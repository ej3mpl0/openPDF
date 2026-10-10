// SPDX-License-Identifier: MPL-2.0
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::backend::{PageSize, TileBitmap};

pub const DEFAULT_TILE_SIZE: u32 = 256;
pub const DEFAULT_CACHE_BYTES: usize = 64 * 1024 * 1024;

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
#[allow(clippy::too_many_arguments)]
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
    fn preview_key_is_coarser() {
        let k = TileKey::new(0, 2.0, 1, 1, 256);
        let p = k.preview();
        assert!(p.zoom_milli < k.zoom_milli);
        assert_eq!(p.tx, 1);
    }
}
