//! Coordinate helpers shared by the platform engines.

use openpdf_core::OcrWord;

/// Pack `stride` RGBA into tightly packed `width * 4` rows.
pub fn pack_rgba(width: u32, height: u32, stride: u32, rgba: &[u8]) -> Vec<u8> {
    let row = (width as usize).saturating_mul(4);
    let stride = stride.max(width.saturating_mul(4)) as usize;
    if stride == row {
        return rgba.to_vec();
    }
    let mut out = Vec::with_capacity(row.saturating_mul(height as usize));
    for y in 0..height as usize {
        let start = y.saturating_mul(stride);
        if start + row <= rgba.len() {
            out.extend_from_slice(&rgba[start..start + row]);
        }
    }
    out
}

/// Vision normalized box (origin bottom-left, 0..1) to page pixels (origin top-left).
pub fn vision_box_to_pixels(
    origin_x: f32,
    origin_y: f32,
    size_w: f32,
    size_h: f32,
    image_w: f32,
    image_h: f32,
) -> (f32, f32, f32, f32) {
    let w = size_w * image_w;
    let h = size_h * image_h;
    let x = origin_x * image_w;
    let y = (1.0 - origin_y - size_h) * image_h;
    (x, y, w, h)
}

/// Windows.Media.Ocr `BoundingRect` is already top-left pixels.
/// Confidence is not reported; callers use 1.0.
pub fn windows_rect_to_word(text: String, x: f32, y: f32, w: f32, h: f32) -> OcrWord {
    OcrWord {
        text,
        x,
        y,
        w,
        h,
        confidence: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vision_flips_origin_to_top_left() {
        // origin (0.1, 0.2), size (0.3, 0.4) on a 100x200 image.
        let (x, y, w, h) = vision_box_to_pixels(0.1, 0.2, 0.3, 0.4, 100.0, 200.0);
        assert!((x - 10.0).abs() < 1e-4);
        assert!((w - 30.0).abs() < 1e-4);
        assert!((h - 80.0).abs() < 1e-4);
        assert!((y - 80.0).abs() < 1e-4, "y={y}");
    }

    #[test]
    fn windows_confidence_is_one() {
        let w = windows_rect_to_word("Hi".into(), 1.0, 2.0, 3.0, 4.0);
        assert_eq!(w.confidence, 1.0);
        assert_eq!(w.text, "Hi");
        assert_eq!((w.x, w.y, w.w, w.h), (1.0, 2.0, 3.0, 4.0));
    }

    #[test]
    fn pack_rgba_respects_stride() {
        let raw = [1, 2, 3, 4, 9, 9, 5, 6, 7, 8, 9, 9];
        let packed = pack_rgba(1, 2, 6, &raw);
        assert_eq!(packed, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }
}
