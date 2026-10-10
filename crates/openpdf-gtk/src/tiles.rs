// SPDX-License-Identifier: MPL-2.0
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::backend::{PageSize, TileBitmap};

pub const DEFAULT_TILE_SIZE: u32 = 256;
pub const DEFAULT_CACHE_BYTES: usize = 16 * 1024 * 1024;

/// Thumbnail sidebar request viewport. Never the full document height:
/// a ~800 px strip shows the measure gate (`MIN_THUMB_TILES` ≈ 3–4).
pub const MIN_THUMB_STRIP_PX: f64 = 800.0;

/// `(view_y, view_h)` for thumbnail tile requests.
///
/// When the strip is not yet allocated, cover [`MIN_THUMB_STRIP_PX`] from
/// the top instead of every page (32 pages would pin 32 surfaces).
pub fn thumb_request_viewport(allocated: f64, scroll_y: f64, content_height: f64) -> (f64, f64) {
    let content_height = content_height.max(1.0);
    let view_h = allocated.max(MIN_THUMB_STRIP_PX).min(content_height);
    let view_y = if allocated < 32.0 { 0.0 } else { scroll_y };
    (view_y, view_h)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub page: u32,
    pub zoom_milli: u32,
    pub tx: u32,
    pub ty: u32,
    pub tile_size: u32,
}

impl TileKey {
    pub fn new(page: u32, zoom: f64, tx: u32, ty: u32, tile_size: u32) -> Self {
        Self {
            page,
            zoom_milli: quantize_zoom(zoom),
            tx,
            ty,
            tile_size,
        }
    }

    pub fn zoom(self) -> f64 {
        f64::from(self.zoom_milli) / 1000.0
    }

    /// Coarser key used for the low-resolution preview pass.
    pub fn preview(self) -> Self {
        Self {
            zoom_milli: (self.zoom_milli / 4).max(50),
            ..self
        }
    }
}

pub fn quantize_zoom(zoom: f64) -> u32 {
    ((zoom.max(0.05) * 1000.0).round() as u32).clamp(50, 16_000)
}

pub fn page_pixel_size(size: PageSize, zoom: f64) -> (u32, u32) {
    let z = zoom.max(0.05);
    (
        ((size.width * z).ceil() as u32).max(1),
        ((size.height * z).ceil() as u32).max(1),
    )
}

pub fn tile_grid(page_w: u32, page_h: u32, tile: u32) -> (u32, u32) {
    let tile = tile.max(1);
    (page_w.div_ceil(tile).max(1), page_h.div_ceil(tile).max(1))
}

#[derive(Debug, Clone, Copy)]
pub struct PageRect {
    pub page: u32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone)]
pub struct PageLayout {
    pub content_width: f64,
    pub content_height: f64,
    pub pages: Vec<PageRect>,
}

pub fn layout_continuous(
    sizes: &[PageSize],
    zoom: f64,
    viewport_width: f64,
    padding: f64,
    gap: f64,
) -> PageLayout {
    let mut pages = Vec::with_capacity(sizes.len());
    let mut y = padding;
    let mut max_w = 0.0f64;
    for (i, size) in sizes.iter().enumerate() {
        let (w, h) = page_pixel_size(*size, zoom);
        let w = f64::from(w);
        let h = f64::from(h);
        max_w = max_w.max(w);
        pages.push(PageRect {
            page: i as u32,
            x: 0.0,
            y,
            width: w,
            height: h,
        });
        y += h + gap;
    }
    if !pages.is_empty() {
        y -= gap;
    }
    y += padding;
    let content_width = max_w + padding * 2.0;
    let content_width = content_width.max(viewport_width);
    for p in &mut pages {
        p.x = ((content_width - p.width) / 2.0).max(padding);
    }
    PageLayout {
        content_width,
        content_height: y.max(padding * 2.0),
        pages,
    }
}

pub fn layout_single(
    sizes: &[PageSize],
    page: u32,
    zoom: f64,
    viewport_width: f64,
    viewport_height: f64,
    padding: f64,
) -> PageLayout {
    if sizes.is_empty() {
        return PageLayout {
            content_width: viewport_width,
            content_height: viewport_height,
            pages: Vec::new(),
        };
    }
    let idx = (page as usize).min(sizes.len() - 1);
    let (w, h) = page_pixel_size(sizes[idx], zoom);
    let w = f64::from(w);
    let h = f64::from(h);
    let content_width = (w + padding * 2.0).max(viewport_width);
    let content_height = (h + padding * 2.0).max(viewport_height);
    PageLayout {
        content_width,
        content_height,
        pages: vec![PageRect {
            page: idx as u32,
            x: ((content_width - w) / 2.0).max(padding),
            y: ((content_height - h) / 2.0).max(padding),
            width: w,
            height: h,
        }],
    }
}

/// Tiles that intersect the viewport, plus a one-tile halo (adjacent).
pub fn visible_tiles(
    layout: &PageLayout,
    view_x: f64,
    view_y: f64,
    view_w: f64,
    view_h: f64,
    zoom: f64,
    tile_size: u32,
    halo: u32,
) -> Vec<TileKey> {
    let view_r = view_x + view_w;
    let view_b = view_y + view_h;
    let mut keys = Vec::new();
    let tile = f64::from(tile_size.max(1));
    for page in &layout.pages {
        if page.x + page.width < view_x || page.x > view_r {
            continue;
        }
        if page.y + page.height < view_y || page.y > view_b {
            continue;
        }
        let left = ((view_x - page.x).max(0.0) / tile).floor() as i32 - halo as i32;
        let top = ((view_y - page.y).max(0.0) / tile).floor() as i32 - halo as i32;
        let right = ((view_r - page.x).min(page.width) / tile).ceil() as i32 + halo as i32;
        let bottom = ((view_b - page.y).min(page.height) / tile).ceil() as i32 + halo as i32;
        let (nx, ny) = tile_grid(page.width as u32, page.height as u32, tile_size);
        let x0 = left.clamp(0, nx as i32) as u32;
        let y0 = top.clamp(0, ny as i32) as u32;
        let x1 = right.clamp(0, nx as i32) as u32;
        let y1 = bottom.clamp(0, ny as i32) as u32;
        for ty in y0..y1 {
            for tx in x0..x1 {
                keys.push(TileKey::new(page.page, zoom, tx, ty, tile_size));
            }
        }
    }
    keys
}

/// LRU tile cache with a hard byte budget so a 1 000-page document stays small.
pub struct TileCache {
    map: HashMap<TileKey, Arc<TileBitmap>>,
    order: VecDeque<TileKey>,
    bytes: usize,
    max_bytes: usize,
}

impl TileCache {
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

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Change the byte budget. Evicts LRU tiles when shrinking.
    pub fn set_max_bytes(&mut self, max_bytes: usize) {
        self.max_bytes = max_bytes.max(1);
        while self.bytes > self.max_bytes {
            if let Some(old_key) = self.order.pop_front() {
                if let Some(old) = self.map.remove(&old_key) {
                    self.bytes = self.bytes.saturating_sub(old.byte_len());
                }
            } else {
                break;
            }
        }
    }

    pub fn get(&mut self, key: &TileKey) -> Option<Arc<TileBitmap>> {
        let tile = self.map.get(key).cloned()?;
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            self.order.remove(pos);
            self.order.push_back(*key);
        }
        Some(tile)
    }

    pub fn insert(&mut self, key: TileKey, tile: Arc<TileBitmap>) {
        if let Some(old) = self.map.remove(&key) {
            self.bytes = self.bytes.saturating_sub(old.byte_len());
            if let Some(pos) = self.order.iter().position(|k| k == &key) {
                self.order.remove(pos);
            }
        }
        self.bytes += tile.byte_len();
        self.map.insert(key, tile);
        self.order.push_back(key);
        while self.bytes > self.max_bytes {
            if let Some(old_key) = self.order.pop_front() {
                if let Some(old) = self.map.remove(&old_key) {
                    self.bytes = self.bytes.saturating_sub(old.byte_len());
                }
            } else {
                break;
            }
        }
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
        self.bytes = 0;
    }
}

/// Place a tile onto a page-sized RGBA buffer at the same origin the GTK
/// draw func uses: `tx * tile_size`, `ty * tile_size`.
pub fn place_tile_rgba(
    dest: &mut [u8],
    dest_w: u32,
    dest_h: u32,
    src: &[u8],
    src_w: u32,
    src_h: u32,
    tx: u32,
    ty: u32,
    tile_size: u32,
) {
    let x0 = (tx.saturating_mul(tile_size)) as usize;
    let y0 = (ty.saturating_mul(tile_size)) as usize;
    let dw = dest_w as usize;
    let dh = dest_h as usize;
    let sw = src_w as usize;
    let sh = src_h as usize;
    if dw == 0 || dest.len() < dw * dh * 4 || src.len() < sw * sh * 4 {
        return;
    }
    for row in 0..sh {
        let dy = y0 + row;
        if dy >= dh {
            break;
        }
        let dest_off = (dy * dw + x0) * 4;
        let src_off = row * sw * 4;
        let cols = sw.min(dw.saturating_sub(x0));
        dest[dest_off..dest_off + cols * 4].copy_from_slice(&src[src_off..src_off + cols * 4]);
    }
}

/// Largest per-channel absolute difference. Used by the tiles-vs-full test.
pub fn max_channel_delta(a: &[u8], b: &[u8]) -> u8 {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

/// Fraction of pixels (RGBA tuples) whose any-channel delta exceeds `tol`.
pub fn pixel_mismatch_ratio(a: &[u8], b: &[u8], tol: u8) -> f64 {
    let n = a.len().min(b.len()) / 4;
    if n == 0 {
        return 1.0;
    }
    let mut bad = 0usize;
    for i in 0..n {
        let o = i * 4;
        if a[o].abs_diff(b[o]) > tol
            || a[o + 1].abs_diff(b[o + 1]) > tol
            || a[o + 2].abs_diff(b[o + 2]) > tol
            || a[o + 3].abs_diff(b[o + 3]) > tol
        {
            bad += 1;
        }
    }
    bad as f64 / n as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a4() -> PageSize {
        PageSize::A4
    }

    #[test]
    fn continuous_layout_stacks_pages() {
        let layout = layout_continuous(&[a4(), a4(), a4()], 1.0, 400.0, 16.0, 12.0);
        assert_eq!(layout.pages.len(), 3);
        assert!(layout.pages[1].y > layout.pages[0].y);
        assert!(layout.content_height > 842.0 * 3.0);
        assert!(layout.pages[0].x > 0.0);
    }

    #[test]
    fn single_layout_only_one_page() {
        let layout = layout_single(&[a4(), a4()], 1, 1.0, 800.0, 600.0, 16.0);
        assert_eq!(layout.pages.len(), 1);
        assert_eq!(layout.pages[0].page, 1);
    }

    #[test]
    fn visible_tiles_cover_viewport_only() {
        let layout = layout_continuous(&[a4()], 1.0, 600.0, 0.0, 0.0);
        let keys = visible_tiles(&layout, 0.0, 0.0, 256.0, 256.0, 1.0, 256, 0);
        assert!(!keys.is_empty());
        assert!(keys.iter().all(|k| k.page == 0 && k.tx <= 1 && k.ty <= 1));
        let all = visible_tiles(&layout, 0.0, 0.0, 2000.0, 2000.0, 1.0, 256, 0);
        assert!(all.len() > keys.len());
    }

    #[test]
    fn cache_evicts_to_budget() {
        let mut cache = TileCache::new(64);
        for i in 0..8 {
            let bmp = Arc::new(TileBitmap {
                width: 2,
                height: 2,
                pixels: vec![1; 16],
            });
            cache.insert(TileKey::new(i, 1.0, 0, 0, 256), bmp);
        }
        assert!(cache.bytes() <= 64);
        assert!(cache.len() <= 4);
        assert!(cache.get(&TileKey::new(7, 1.0, 0, 0, 256)).is_some());
        assert!(cache.get(&TileKey::new(0, 1.0, 0, 0, 256)).is_none());
    }

    #[test]
    fn cache_set_max_bytes_evicts_when_shrinking() {
        let mut cache = TileCache::new(64);
        for i in 0..4 {
            let bmp = Arc::new(TileBitmap {
                width: 2,
                height: 2,
                pixels: vec![1; 16],
            });
            cache.insert(TileKey::new(i, 1.0, 0, 0, 256), bmp);
        }
        assert_eq!(cache.len(), 4);
        cache.set_max_bytes(32);
        assert!(cache.bytes() <= 32);
        assert_eq!(cache.len(), 2);
        assert!(cache.get(&TileKey::new(0, 1.0, 0, 0, 256)).is_none());
        assert!(cache.get(&TileKey::new(3, 1.0, 0, 0, 256)).is_some());
    }

    #[test]
    fn preview_key_is_coarser() {
        let k = TileKey::new(0, 2.0, 1, 1, 256);
        let p = k.preview();
        assert!(p.zoom_milli < k.zoom_milli);
        assert_eq!(p.tx, 1);
    }

    #[test]
    fn place_tile_rgba_uses_tx_ty_offset() {
        let mut dest = vec![0u8; 8 * 4 * 4];
        let tile = vec![
            255u8, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
        ];
        place_tile_rgba(&mut dest, 8, 4, &tile, 2, 2, 1, 1, 2);
        // Tile (1,1) at tile_size 2 starts at (2, 2).
        let i = (2 * 8 + 2) * 4;
        assert_eq!(&dest[i..i + 4], &[255, 0, 0, 255]);
        assert_eq!(&dest[0..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn thumb_request_viewport_caps_unallocated_strip() {
        let content = 32.0 * 200.0;
        let (y, h) = thumb_request_viewport(0.0, 40.0, content);
        assert_eq!(y, 0.0);
        assert_eq!(h, MIN_THUMB_STRIP_PX);
        assert!(h < content, "must not request all 32 thumbnail pages");
        let (y2, h2) = thumb_request_viewport(480.0, 120.0, content);
        assert_eq!(y2, 120.0);
        assert_eq!(h2, MIN_THUMB_STRIP_PX);
        let (y3, h3) = thumb_request_viewport(900.0, 0.0, content);
        assert_eq!((y3, h3), (0.0, 900.0));
    }

    #[test]
    fn tiny_thumb_viewport_misses_later_pages() {
        let sizes = vec![a4(); 8];
        let zoom = 132.0 / 595.0;
        let layout = layout_continuous(&sizes, zoom, 156.0, 10.0, 10.0);
        assert_eq!(layout.pages.len(), 8);
        let tiny = visible_tiles(&layout, 0.0, 0.0, 156.0, 1.0, zoom, 256, 0);
        let all = visible_tiles(
            &layout,
            0.0,
            0.0,
            156.0,
            layout.content_height,
            zoom,
            256,
            0,
        );
        assert!(
            tiny.iter().map(|k| k.page).max().unwrap_or(0) < 7,
            "a 1 px viewport must not be used for thumbs or only page 1 appears"
        );
        assert_eq!(all.iter().map(|k| k.page).max(), Some(7));
        assert_eq!(all.len(), 8);
    }

    #[test]
    fn mock_tiles_at_147_percent_are_distinct_and_offset() {
        use crate::backend::{DocumentBackend, MockDocument, TileRequest};
        let zoom = 1.47;
        let tile = 256u32;
        let doc = MockDocument::sample(1);
        let size = doc.page_size(0).unwrap();
        let (pw, ph) = page_pixel_size(size, zoom);
        let (cols, rows) = tile_grid(pw, ph, tile);
        assert!(cols >= 2 && rows >= 2, "147% page must need several tiles");
        let origin = doc
            .render_tile(TileRequest {
                page: 0,
                zoom,
                tx: 0,
                ty: 0,
                tile_size: tile,
            })
            .unwrap();
        let right = doc
            .render_tile(TileRequest {
                page: 0,
                zoom,
                tx: 1,
                ty: 0,
                tile_size: tile,
            })
            .unwrap();
        assert_ne!(
            origin.pixels, right.pixels,
            "column 1 must not reprint the page origin (tile x offset applied)"
        );
        let mut dest = vec![0u8; pw as usize * ph as usize * 4];
        place_tile_rgba(
            &mut dest,
            pw,
            ph,
            &right.pixels,
            right.width,
            right.height,
            1,
            0,
            tile,
        );
        let i = tile as usize * 4;
        assert_eq!(&dest[i..i + 4], &right.pixels[..4]);
        assert_eq!(&dest[0..4], &[0, 0, 0, 0]);
    }
}
