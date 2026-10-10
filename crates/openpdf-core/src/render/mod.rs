//! Single-threaded PDFium renderer, priority queue, and LRU tile cache.

mod cache;
mod engine;
mod pdfium;
mod queue;

pub use cache::TileCache;
pub use engine::Engine;
pub use pdfium::pdfium_library_name;

use crate::document::DocumentId;

/// Tile edge in pixels.
pub const TILE_SIZE: u32 = 256;

/// Default tile-cache memory cap (64 MiB of raw RGBA).
pub const DEFAULT_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// Quantized zoom used in [`TileKey`]. Stored as `round(zoom * 1000)`.
pub fn quantize_zoom(zoom: f32) -> u32 {
    (zoom.max(0.01) * 1000.0).round() as u32
}

pub fn zoom_from_q(q: u32) -> f32 {
    q as f32 / 1000.0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub doc: DocumentId,
    pub page: u32,
    pub zoom_q: u32,
    pub x: u32,
    pub y: u32,
}

impl TileKey {
    pub fn new(doc: DocumentId, page: u32, zoom: f32, x: u32, y: u32) -> Self {
        Self {
            doc,
            page,
            zoom_q: quantize_zoom(zoom),
            x,
            y,
        }
    }

    pub fn zoom(&self) -> f32 {
        zoom_from_q(self.zoom_q)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Visible = 0,
    Adjacent = 1,
    Thumbnail = 2,
}

#[derive(Clone, Debug)]
pub struct TileRequest {
    pub key: TileKey,
    pub priority: Priority,
}

impl TileRequest {
    pub fn visible(key: TileKey) -> Self {
        Self {
            key,
            priority: Priority::Visible,
        }
    }
}

/// Raw RGBA8 raster (unpremultiplied, row-major, no padding).
#[derive(Clone, Debug)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Raster {
    pub fn byte_len(&self) -> usize {
        self.rgba.len()
    }
}

#[derive(Clone, Debug)]
pub struct Tile {
    pub key: TileKey,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Tile {
    pub fn byte_len(&self) -> usize {
        self.rgba.len()
    }
}

#[derive(Clone, Debug)]
pub enum TileEvent {
    Ready(std::sync::Arc<Tile>),
    Cancelled(TileKey),
    Failed { key: TileKey, message: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub page: u32,
    pub start: usize,
    pub end: usize,
    pub context: String,
}

pub fn page_pixel_size(width_pt: f32, height_pt: f32, zoom: f32) -> (u32, u32) {
    let w = (width_pt * zoom).round().max(1.0) as u32;
    let h = (height_pt * zoom).round().max(1.0) as u32;
    (w, h)
}

pub fn tile_grid(pixel_w: u32, pixel_h: u32, tile: u32) -> (u32, u32) {
    let t = tile.max(1);
    (pixel_w.div_ceil(t), pixel_h.div_ceil(t))
}

pub fn tile_rect(full_w: u32, full_h: u32, x: u32, y: u32, tile: u32) -> (u32, u32, u32, u32) {
    let x0 = x * tile;
    let y0 = y * tile;
    let tw = tile.min(full_w.saturating_sub(x0)).max(1);
    let th = tile.min(full_h.saturating_sub(y0)).max(1);
    (tw, th, x0, y0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_and_edge_tile() {
        let (cols, rows) = tile_grid(600, 800, 256);
        assert_eq!((cols, rows), (3, 4));
        let (tw, th, x0, y0) = tile_rect(600, 800, 2, 3, 256);
        assert_eq!((x0, y0), (512, 768));
        assert_eq!((tw, th), (88, 32));
    }
}
