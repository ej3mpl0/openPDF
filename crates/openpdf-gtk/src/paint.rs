// SPDX-License-Identifier: MPL-2.0
//! Single construction point for tile / thumb / preview rasters.
//!
//! Tiles become [`gdk::MemoryTexture`] from core RGBA via `glib::Bytes::from_owned`
//! (no pixel copy). The GTK tile cache stores those textures. Snapshot paints
//! with `append_texture` / `append_scaled_texture` — no cairo surface per tile.
//!
//! 0×N / N×0 / &gt;32767 still return None so GSK ngl never sees a 0-wide
//! texture or an INVALID_SIZE cairo node.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use gtk::cairo;
use gtk::gdk;
use gtk::gdk::prelude::{PaintableExt, TextureExt};
use gtk::glib;
use gtk::graphene;
use gtk::prelude::*;
use gtk4 as gtk;

use crate::tiles::TileKey;

/// cairo `CAIRO_MAX_IMAGE_SIZE`. Device pixels above this yield INVALID_SIZE.
pub const MAX_CAIRO_DIM: i32 = 32767;

/// Largest widget-space edge that stays ≤ [`MAX_CAIRO_DIM`] after `scale`.
pub fn max_widget_dim(scale: i32) -> f64 {
    let s = f64::from(scale.max(1));
    (f64::from(MAX_CAIRO_DIM) / s).floor().max(1.0)
}

/// True when a widget-space size is safe to hand to cairo at `scale`.
pub fn valid_cairo_size(width: i32, height: i32, scale: i32) -> bool {
    if width <= 0 || height <= 0 {
        return false;
    }
    let s = i64::from(scale.max(1));
    i64::from(width) * s <= i64::from(MAX_CAIRO_DIM)
        && i64::from(height) * s <= i64::from(MAX_CAIRO_DIM)
}

/// Clip a paint rect so the device-pixel size is in `(0, MAX_CAIRO_DIM]`.
pub fn clamp_cairo_rect(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    scale: i32,
) -> Option<(f64, f64, f64, f64)> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let max = max_widget_dim(scale);
    let width = width.min(max);
    let height = height.min(max);
    if width < 1.0 || height < 1.0 {
        return None;
    }
    Some((x, y, width, height))
}

pub fn context_ok(cr: &cairo::Context) -> bool {
    cr.status().is_ok()
}

pub fn surface_ok(surface: &cairo::ImageSurface) -> bool {
    surface.status().is_ok() && surface.width() > 0 && surface.height() > 0
}

/// Build a cairo image surface for a decoded RGBA tile.
///
/// Returns `None` when size is 0, exceeds [`MAX_CAIRO_DIM`], or cairo
/// reports an error. Never hands a bad surface to cairo/GSK.
pub fn image_surface_from_rgba(
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Option<cairo::ImageSurface> {
    if width == 0 || height == 0 {
        return None;
    }
    if width > MAX_CAIRO_DIM as u32 || height > MAX_CAIRO_DIM as u32 {
        return None;
    }
    debug_assert!(
        valid_cairo_size(width as i32, height as i32, 1),
        "tile texture {width}x{height} must be a valid cairo size"
    );
    let (data, stride) = rgba_to_cairo_argb(rgba, width, height)?;
    let surface = cairo::ImageSurface::create_for_data(
        data,
        cairo::Format::ARgb32,
        width as i32,
        height as i32,
        stride,
    )
    .ok()?;
    if !surface_ok(&surface) {
        return None;
    }
    Some(surface)
}

/// Host bytes currently held by GTK [`TextureCache`] instances (w×h×4).
static TEXTURE_CACHE_BYTES: AtomicUsize = AtomicUsize::new(0);
static TEXTURE_CREATES: AtomicU64 = AtomicU64::new(0);

pub fn texture_cache_bytes() -> usize {
    TEXTURE_CACHE_BYTES.load(Ordering::Relaxed)
}

pub fn texture_create_count() -> u64 {
    TEXTURE_CREATES.load(Ordering::Relaxed)
}

pub fn reset_texture_create_count() {
    TEXTURE_CREATES.store(0, Ordering::Relaxed);
}

fn publish_texture_bytes(bytes: usize) {
    TEXTURE_CACHE_BYTES.store(bytes, Ordering::Relaxed);
}

/// Core sets `FPDF_REVERSE_BYTE_ORDER`, so tiles are RGBA, opaque white fill.
/// Never BGRA / Premultiplied — those force a CPU convert or swap red/blue.
pub const TILE_MEMORY_FORMAT: gdk::MemoryFormat = gdk::MemoryFormat::R8g8b8x8;

/// RGBA bytes → [`gdk::MemoryTexture`] with no extra pixel copy.
///
/// `pixels` is moved into `glib::Bytes::from_owned`. 0 / oversize / short
/// buffers return `None`. Call when the tile arrives, never from snapshot.
pub fn memory_texture_from_owned(
    width: u32,
    height: u32,
    pixels: impl AsRef<[u8]> + Send + 'static,
) -> Option<gdk::MemoryTexture> {
    if width == 0 || height == 0 {
        return None;
    }
    if width > MAX_CAIRO_DIM as u32 || height > MAX_CAIRO_DIM as u32 {
        return None;
    }
    if !gtk::is_initialized_main_thread() {
        return None;
    }
    let stride = (width as usize).checked_mul(4)?;
    let need = stride.checked_mul(height as usize)?;
    if pixels.as_ref().len() < need {
        return None;
    }
    debug_assert!(
        valid_cairo_size(width as i32, height as i32, 1),
        "tile texture {width}x{height} must be a valid size"
    );
    debug_assert!(
        pixels.as_ref()[..need].chunks_exact(4).all(|p| p[3] == 255),
        "R8g8b8x8 requires opaque tiles (alpha 255); core fills white"
    );
    let bytes = glib::Bytes::from_owned(pixels);
    TEXTURE_CREATES.fetch_add(1, Ordering::Relaxed);
    Some(gdk::MemoryTexture::new(
        width as i32,
        height as i32,
        TILE_MEMORY_FORMAT,
        &bytes,
        stride,
    ))
}

/// LRU of [`gdk::Texture`]. Budget is texture bytes (`width * height * 4`).
pub struct TextureCache {
    map: HashMap<TileKey, gdk::Texture>,
    order: VecDeque<TileKey>,
    bytes: usize,
    max_bytes: usize,
}

impl TextureCache {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            max_bytes: max_bytes.max(1),
        }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn zoom_millis(&self) -> std::collections::BTreeSet<u32> {
        self.map.keys().map(|k| k.zoom_milli).collect()
    }

    pub fn bytes_for_zooms(&self, keep: &[u32]) -> usize {
        self.map
            .iter()
            .filter(|(k, _)| keep.contains(&k.zoom_milli))
            .map(|(_, t)| texture_byte_len(t))
            .sum()
    }

    pub fn set_max_bytes(&mut self, max_bytes: usize) {
        self.max_bytes = max_bytes.max(1);
        self.evict();
        publish_texture_bytes(self.bytes);
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
        self.bytes = 0;
        publish_texture_bytes(0);
    }

    pub fn peek(&self, key: &TileKey) -> Option<gdk::Texture> {
        self.map.get(key).cloned()
    }

    pub fn get(&mut self, key: &TileKey) -> Option<gdk::Texture> {
        let tile = self.map.get(key).cloned()?;
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
            self.order.push_back(*key);
        }
        Some(tile)
    }

    pub fn insert(&mut self, key: TileKey, texture: gdk::Texture) {
        let add = texture_byte_len(&texture);
        if let Some(old) = self.map.remove(&key) {
            self.bytes = self.bytes.saturating_sub(texture_byte_len(&old));
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
            }
        }
        self.bytes += add;
        self.map.insert(key, texture);
        self.order.push_back(key);
        self.evict();
        publish_texture_bytes(self.bytes);
    }

    fn evict(&mut self) {
        while self.bytes > self.max_bytes {
            if let Some(old_key) = self.order.pop_front() {
                if let Some(old) = self.map.remove(&old_key) {
                    self.bytes = self.bytes.saturating_sub(texture_byte_len(&old));
                }
            } else {
                break;
            }
        }
    }
}

fn texture_byte_len(texture: &gdk::Texture) -> usize {
    (texture.width().max(0) as usize)
        .saturating_mul(texture.height().max(0) as usize)
        .saturating_mul(4)
}

/// Append a texture node only when the bounds have a valid size.
pub fn append_texture(
    snapshot: &gtk::Snapshot,
    texture: &gdk::Texture,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) {
    if !valid_texture_bounds(width, height) {
        return;
    }
    snapshot.append_texture(texture, &graphene::Rect::new(x, y, width, height));
}

/// Scale a (usually preview) texture into `width`×`height`.
pub fn append_scaled_texture(
    snapshot: &gtk::Snapshot,
    texture: &gdk::Texture,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) {
    if !valid_texture_bounds(width, height) {
        return;
    }
    snapshot.append_scaled_texture(
        texture,
        gtk::gsk::ScalingFilter::Linear,
        &graphene::Rect::new(x, y, width, height),
    );
}

fn valid_texture_bounds(width: f32, height: f32) -> bool {
    if width <= 0.0 || height <= 0.0 {
        return false;
    }
    valid_cairo_size(width.ceil() as i32, height.ceil() as i32, 1)
}

/// `gtk_snapshot_append_cairo` only when bounds are a valid cairo size.
/// A 0-wide or >32767 rect is what GSK turns into an error-status context
/// (`gsk_render_node_draw` CRITICAL) and then a 0-width texture abort.
pub fn append_cairo(
    snapshot: &gtk::Snapshot,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> Option<cairo::Context> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let w = width.ceil() as i32;
    let h = height.ceil() as i32;
    if !valid_cairo_size(w, h, 1) {
        return None;
    }
    debug_assert!(
        width > 0.0 && height > 0.0,
        "cairo node {width}x{height} must be non-zero"
    );
    let cr = snapshot.append_cairo(&graphene::Rect::new(x, y, width, height));
    if !context_ok(&cr) {
        return None;
    }
    Some(cr)
}

/// Convert unpremultiplied RGBA8 to cairo ARgb32 (BGRA on little-endian).
fn rgba_to_cairo_argb(rgba: &[u8], width: u32, height: u32) -> Option<(Vec<u8>, i32)> {
    let w = width as i32;
    let h = height as i32;
    if w <= 0 || h <= 0 || w > MAX_CAIRO_DIM || h > MAX_CAIRO_DIM {
        return None;
    }
    let stride = cairo::Format::ARgb32.stride_for_width(width).ok()?;
    if stride <= 0 {
        return None;
    }
    let mut data = vec![0u8; stride as usize * h as usize];
    let src_stride = width as usize * 4;
    if src_stride == 0 {
        return None;
    }
    for (dest_row, src_row) in data
        .chunks_mut(stride as usize)
        .zip(rgba.chunks_exact(src_stride))
        .take(height as usize)
    {
        let (dest_px, _) = dest_row.as_chunks_mut::<4>();
        let (src_px, _) = src_row.as_chunks::<4>();
        for (dest, src) in dest_px.iter_mut().zip(src_px) {
            dest[0] = src[2];
            dest[1] = src[1];
            dest[2] = src[0];
            dest[3] = src[3];
        }
    }
    Some((data, stride))
}

/// Snapshot a widget. 0 / oversize is a no-op. Positive sizes go through
/// `GtkWidgetPaintable` so GSK ngl is exercised (the crash under test).
pub fn snapshot_widget(widget: &impl IsA<gtk::Widget>, width: f64, height: f64) -> gtk::Snapshot {
    let snapshot = gtk::Snapshot::new();
    if width <= 0.0 || height <= 0.0 {
        return snapshot;
    }
    if width.ceil() as i32 > MAX_CAIRO_DIM || height.ceil() as i32 > MAX_CAIRO_DIM {
        return snapshot;
    }
    if gtk::is_initialized_main_thread() {
        let paintable = gtk::WidgetPaintable::new(Some(widget));
        PaintableExt::snapshot(&paintable, &snapshot, width, height);
    }
    snapshot
}

/// Paint RGBA onto `cr` only when both the context and the source are valid.
/// Resets the source afterwards so dropping the surface cannot finish() a
/// surface that is still the context's pattern.
pub fn paint_rgba_on(
    cr: &cairo::Context,
    x: f64,
    y: f64,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> bool {
    if !context_ok(cr) {
        return false;
    }
    let Some(surface) = image_surface_from_rgba(width, height, rgba) else {
        return false;
    };
    if cr.set_source_surface(&surface, x, y).is_err() || !context_ok(cr) {
        return false;
    }
    let painted = cr.paint().is_ok() && context_ok(cr);
    // Detach the image surface before it is dropped (Drop decref / finish).
    cr.set_source_rgb(0.0, 0.0, 0.0);
    painted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_surface_rejects_zero_size() {
        assert!(image_surface_from_rgba(0, 16, &[0; 4]).is_none());
        assert!(image_surface_from_rgba(16, 0, &[0; 4]).is_none());
        assert!(image_surface_from_rgba(0, 0, &[]).is_none());
        let pix = vec![255u8; 4];
        let surf = image_surface_from_rgba(1, 1, &pix).expect("1x1");
        assert_eq!(surf.width(), 1);
        assert_eq!(surf.height(), 1);
        assert!(surface_ok(&surf));
    }

    #[test]
    fn image_surface_rejects_oversize() {
        assert!(!valid_cairo_size(0, 16, 1));
        assert!(!valid_cairo_size(16, 0, 1));
        assert!(!valid_cairo_size(MAX_CAIRO_DIM + 1, 16, 1));
        assert!(!valid_cairo_size(16, MAX_CAIRO_DIM + 1, 1));
        assert!(valid_cairo_size(MAX_CAIRO_DIM, 1, 1));
        assert!(!valid_cairo_size(20_000, 20_000, 2));
        assert!(clamp_cairo_rect(0.0, 0.0, 0.0, 100.0, 1).is_none());
        let (_, _, w, h) = clamp_cairo_rect(0.0, 0.0, 80_000.0, 80_000.0, 1).unwrap();
        assert!(w <= f64::from(MAX_CAIRO_DIM));
        assert!(h <= f64::from(MAX_CAIRO_DIM));
        assert!(image_surface_from_rgba(MAX_CAIRO_DIM as u32 + 1, 8, &[0; 4]).is_none());
    }

    #[test]
    fn memory_texture_rejects_zero_and_oversize() {
        let Some(()) = crate::test_gtk::on_gtk_thread(|| {
            assert!(memory_texture_from_owned(0, 16, vec![0u8; 4]).is_none());
            assert!(memory_texture_from_owned(16, 0, vec![0u8; 4]).is_none());
            assert!(memory_texture_from_owned(MAX_CAIRO_DIM as u32 + 1, 8, vec![0u8; 4]).is_none());
            let pix = vec![255u8; 4];
            let tex = memory_texture_from_owned(1, 1, pix).expect("1x1 texture");
            assert_eq!(tex.width(), 1);
            assert_eq!(tex.height(), 1);
            assert_eq!(TILE_MEMORY_FORMAT, gdk::MemoryFormat::R8g8b8x8);
        }) else {
            eprintln!("memory_texture sizes: skipped (no GTK display)");
            return;
        };
    }

    /// Core RGBA red + `R8g8b8x8` through GSK must stay red (not blue).
    #[test]
    fn gsk_red_rect_stays_red_not_blue() {
        let Ok(engine) = openpdf_core::Engine::new() else {
            eprintln!("gsk red rect: Engine::new skipped (no PDFium)");
            return;
        };
        let doc = openpdf_core::red_rect_document().unwrap();
        engine.bind(&doc).unwrap();
        let raster = engine.render_page(doc.id(), 0, 2.0).unwrap();
        assert!(
            raster.rgba.chunks_exact(4).all(|p| p[3] == 255),
            "tile alpha must be 255 for R8g8b8x8"
        );
        let cx = (raster.width / 2) as usize;
        let cy = (raster.height / 2) as usize;
        let i = (cy * raster.width as usize + cx) * 4;
        assert!(
            raster.rgba[i] > 200 && raster.rgba[i + 2] < 40,
            "core bytes must be RGBA red, got R={} B={}",
            raster.rgba[i],
            raster.rgba[i + 2]
        );

        let Some(()) = crate::test_gtk::on_gtk_thread(move || {
            let w = raster.width;
            let h = raster.height;
            let tex = memory_texture_from_owned(w, h, raster.rgba).expect("red texture");
            let snapshot = gtk::Snapshot::new();
            append_texture(&snapshot, tex.upcast_ref(), 0.0, 0.0, w as f32, h as f32);
            let node = snapshot.to_node().expect("gsk node");
            let mut surf = cairo::ImageSurface::create(cairo::Format::ARgb32, w as i32, h as i32)
                .expect("cairo target");
            {
                let cr = cairo::Context::new(&surf).unwrap();
                cr.set_source_rgb(0.0, 0.0, 0.0);
                let _ = cr.paint();
                node.draw(&cr);
            }
            surf.flush();
            let stride = surf.stride() as usize;
            let data = surf.data().expect("cairo pixels");
            let off = cy * stride + cx * 4;
            let (b, g, r, a) = (data[off], data[off + 1], data[off + 2], data[off + 3]);
            assert_eq!(a, 255);
            assert!(
                r > 200 && b < 40,
                "GSK pixel must stay red not blue (cairo ARgb32 B={b} G={g} R={r})"
            );
        }) else {
            eprintln!("gsk red rect: skipped (no GTK display)");
            return;
        };
    }
}
