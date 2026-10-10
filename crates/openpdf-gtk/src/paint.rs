// SPDX-License-Identifier: MPL-2.0
//! Single construction point for tile / thumb / preview raster surfaces.
//!
//! GSK ngl/gl uploads cairo image surfaces via `gdk_texture_new_for_surface`.
//! A 0×N or N×0 surface trips `width > 0` and then
//! `gsk_gl_driver_cache_texture: texture_id > 0`. A surface whose width or
//! height exceeds [`MAX_CAIRO_DIM`] (or a context reused after `finish`)
//! leaves `cairo_status != SUCCESS`; GSK then emits
//! `gsk_render_node_draw` CRITICAL. Every caller goes through these helpers.

use gtk::cairo;
use gtk::gdk;
use gtk::graphene;
use gtk::prelude::*;
use gtk4 as gtk;

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

/// Append a texture node only when the bounds have positive size.
pub fn append_texture(
    snapshot: &gtk::Snapshot,
    texture: &gdk::Texture,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    debug_assert!(
        width > 0.0 && height > 0.0,
        "texture node {width}x{height} must be non-zero"
    );
    snapshot.append_texture(texture, &graphene::Rect::new(x, y, width, height));
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

/// Build a snapshot for a widget-sized rect without going through
/// `GtkWidgetPaintable` (GSK ngl SIGSEGVs that path on some xvfb/llvmpipe
/// runners). 0 / oversize is a no-op; a valid size appends one cairo node.
pub fn snapshot_widget(_widget: &impl IsA<gtk::Widget>, width: f64, height: f64) -> gtk::Snapshot {
    let snapshot = gtk::Snapshot::new();
    let _ = append_cairo(&snapshot, 0.0, 0.0, width as f32, height as f32);
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
}
