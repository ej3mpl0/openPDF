//! Single-threaded PDFium renderer, priority queue, and LRU tile cache.

mod cache;
mod engine;
mod pdfium;
mod queue;

pub use cache::TileCache;
pub use engine::Engine;

use crate::document::DocumentId;

/// Tile edge in pixels.
pub const TILE_SIZE: u32 = 256;

/// Default tile-cache memory cap (16 MiB of raw RGBA).
///
/// Override with `OPENPDF_TILE_CACHE_BYTES` (bytes). 16 visible 256² tiles
/// are ~4 MiB; the rest is thumbs and a short-lived adjacent preview.
pub const DEFAULT_CACHE_BYTES: usize = 16 * 1024 * 1024;

/// Lower clamp for [`viewport_cache_bytes`] (same as [`DEFAULT_CACHE_BYTES`]).
pub const MIN_CACHE_BYTES: usize = DEFAULT_CACHE_BYTES;

/// Upper clamp for [`viewport_cache_bytes`].
pub const MAX_CACHE_BYTES: usize = 128 * 1024 * 1024;

/// [`DEFAULT_CACHE_BYTES`], or a positive `OPENPDF_TILE_CACHE_BYTES`.
pub fn tile_cache_bytes() -> usize {
    parse_tile_cache_bytes(std::env::var("OPENPDF_TILE_CACHE_BYTES").ok().as_deref())
}

pub fn parse_tile_cache_bytes(raw: Option<&str>) -> usize {
    raw.and_then(|s| s.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_CACHE_BYTES)
}

/// Tile-cache budget from the visible GTK viewport.
///
/// `device_px = (logical_width * scale) * (logical_height * scale)` using
/// the widget's logical size and `scale_factor` exactly once. Budget is
/// `2 * device_px * 4` bytes (two frames of RGBA), clamped to
/// [[`MIN_CACHE_BYTES`], [`MAX_CACHE_BYTES`]].
///
/// 1920×1080 @ scale 2 matches 3840×2160 @ scale 1 (66 355 200 ≈ 63.3 MiB),
/// not 4× that figure.
pub fn viewport_cache_bytes(logical_width: u32, logical_height: u32, scale_factor: f64) -> usize {
    let scale = scale_factor.max(0.0);
    let device_w = f64::from(logical_width) * scale;
    let device_h = f64::from(logical_height) * scale;
    let device_px = device_w * device_h;
    let raw = 2.0 * device_px * 4.0;
    let bytes = if raw.is_finite() && raw > 0.0 {
        raw.round() as usize
    } else {
        MIN_CACHE_BYTES
    };
    bytes.clamp(MIN_CACHE_BYTES, MAX_CACHE_BYTES)
}

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

    #[test]
    fn tile_cache_bytes_reads_env_or_default() {
        assert_eq!(parse_tile_cache_bytes(None), DEFAULT_CACHE_BYTES);
        assert_eq!(parse_tile_cache_bytes(Some("0")), DEFAULT_CACHE_BYTES);
        assert_eq!(parse_tile_cache_bytes(Some("nope")), DEFAULT_CACHE_BYTES);
        assert_eq!(parse_tile_cache_bytes(Some("8388608")), 8 * 1024 * 1024);
        assert_eq!(DEFAULT_CACHE_BYTES, 16 * 1024 * 1024);
    }

    #[test]
    fn hidpi_1920x1080_scale_2_matches_3840x2160_scale_1_not_4x() {
        const EXPECTED: usize = 2 * 3840 * 2160 * 4; // 66_355_200 ≈ 63.3 MiB
        assert_eq!(EXPECTED, 66_355_200);
        let hidpi = viewport_cache_bytes(1920, 1080, 2.0);
        let uhd = viewport_cache_bytes(3840, 2160, 1.0);
        assert_eq!(hidpi, uhd);
        assert_eq!(hidpi, EXPECTED);
        let once = 2 * 1920 * 1080 * 2 * 4; // scale applied to area once
        assert_ne!(hidpi, once, "scale must apply to each logical edge");
        let four_x = EXPECTED.saturating_mul(4);
        assert_ne!(hidpi, four_x.clamp(MIN_CACHE_BYTES, MAX_CACHE_BYTES));
        assert_eq!(
            viewport_cache_bytes(1280, 800, 1.0),
            MIN_CACHE_BYTES,
            "1280×800 @ 1 is 8 MiB raw → clamp 16 MiB"
        );
        assert_eq!(viewport_cache_bytes(7680, 4320, 2.0), MAX_CACHE_BYTES);
    }
}
