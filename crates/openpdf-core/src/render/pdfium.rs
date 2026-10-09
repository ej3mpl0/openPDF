use std::env;
use std::path::{Path, PathBuf};

use pdfium_render::prelude::*;

use crate::error::{Error, Result};

/// Resolve and load `libpdfium` from a bundled path, `OPENPDF_PDFIUM`, or the system.
pub fn bind_pdfium(bundled: Option<&Path>) -> Result<Pdfium> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(p) = env::var("OPENPDF_PDFIUM") {
        if !p.is_empty() {
            candidates.push(PathBuf::from(p));
        }
    }
    if let Some(p) = bundled {
        candidates.push(p.to_path_buf());
    }
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.to_path_buf());
            candidates.push(dir.join("libpdfium.so"));
        }
    }
    candidates.push(PathBuf::from("."));
    candidates.push(PathBuf::from("libpdfium.so"));
    candidates.push(PathBuf::from("/usr/local/lib"));
    candidates.push(PathBuf::from("/usr/local/lib/libpdfium.so"));
    candidates.push(PathBuf::from("/usr/lib"));
    candidates.push(PathBuf::from("/usr/lib/libpdfium.so"));
    candidates.push(PathBuf::from("/usr/lib/x86_64-linux-gnu"));
    candidates.push(PathBuf::from("/usr/lib/x86_64-linux-gnu/libpdfium.so"));

    let mut last = None;
    for c in candidates {
        let lib = if c.is_dir() {
            Pdfium::pdfium_platform_library_name_at_path(&c)
        } else {
            c
        };
        match Pdfium::bind_to_library(&lib) {
            Ok(bindings) => return Ok(Pdfium::new(bindings)),
            Err(e) => last = Some(e.to_string()),
        }
    }

    match Pdfium::bind_to_system_library() {
        Ok(bindings) => Ok(Pdfium::new(bindings)),
        Err(e) => Err(Error::pdfium_load(
            last.unwrap_or_else(|| "none".into()),
            e.to_string(),
        )),
    }
}

pub fn render_tile(
    document: &PdfDocument<'_>,
    page_index: u32,
    zoom: f32,
    tile_x: u32,
    tile_y: u32,
    tile_size: u32,
) -> Result<(u32, u32, Vec<u8>)> {
    let pages = document.pages();
    if page_index >= pages.len() as u32 {
        return Err(Error::PageOutOfRange {
            page: page_index,
            count: pages.len() as u32,
        });
    }
    let page = pages
        .get(page_index as i32)
        .map_err(|e| Error::pdfium(e.to_string()))?;
    let page_w = page.width().value;
    let page_h = page.height().value;
    let (full_w, full_h) = super::page_pixel_size(page_w, page_h, zoom);
    let (tw, th, x0, y0) = super::tile_rect(full_w, full_h, tile_x, tile_y, tile_size);

    let config = PdfRenderConfig::new()
        .scale_page_by_factor(zoom)
        .set_origin(-(x0 as i32), -(y0 as i32))
        .set_reverse_byte_order(true)
        .render_annotations(true);

    let mut bitmap = PdfBitmap::empty(tw as i32, th as i32, PdfBitmapFormat::BGRA)
        .map_err(|e| Error::pdfium(e.to_string()))?;
    match page.render_into_bitmap_with_config(&mut bitmap, &config) {
        Ok(()) => {
            let rgba = bitmap.as_rgba_bytes();
            Ok((tw, th, force_rgba(rgba, tw, th)))
        }
        Err(_) => {
            // Fallback: render the full page and crop. Used if origin+small bitmap is rejected.
            let full = page
                .render_with_config(
                    &PdfRenderConfig::new()
                        .scale_page_by_factor(zoom)
                        .set_reverse_byte_order(true)
                        .render_annotations(true),
                )
                .map_err(|e| Error::pdfium(e.to_string()))?;
            let raw = force_rgba(full.as_rgba_bytes(), full_w, full_h);
            Ok((tw, th, crop_rgba(&raw, full_w, full_h, x0, y0, tw, th)))
        }
    }
}

pub fn render_page(document: &PdfDocument<'_>, page_index: u32, zoom: f32) -> Result<(u32, u32, Vec<u8>)> {
    let pages = document.pages();
    if page_index >= pages.len() as u32 {
        return Err(Error::PageOutOfRange {
            page: page_index,
            count: pages.len() as u32,
        });
    }
    let page = pages
        .get(page_index as i32)
        .map_err(|e| Error::pdfium(e.to_string()))?;
    let bitmap = page
        .render_with_config(
            &PdfRenderConfig::new()
                .scale_page_by_factor(zoom)
                .set_reverse_byte_order(true)
                .render_annotations(true),
        )
        .map_err(|e| Error::pdfium(e.to_string()))?;
    let w = bitmap.width() as u32;
    let h = bitmap.height() as u32;
    Ok((w, h, force_rgba(bitmap.as_rgba_bytes(), w, h)))
}

pub fn page_text(document: &PdfDocument<'_>, page_index: u32) -> Result<String> {
    let pages = document.pages();
    if page_index >= pages.len() as u32 {
        return Err(Error::PageOutOfRange {
            page: page_index,
            count: pages.len() as u32,
        });
    }
    let page = pages
        .get(page_index as i32)
        .map_err(|e| Error::pdfium(e.to_string()))?;
    let text = page.text().map_err(|e| Error::pdfium(e.to_string()))?;
    Ok(text.all())
}

fn force_rgba(mut bytes: Vec<u8>, w: u32, h: u32) -> Vec<u8> {
    let expected = (w as usize) * (h as usize) * 4;
    if bytes.len() == expected {
        return bytes;
    }
    if bytes.len() > expected {
        bytes.truncate(expected);
        return bytes;
    }
    bytes.resize(expected, 255);
    bytes
}

fn crop_rgba(src: &[u8], src_w: u32, src_h: u32, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
    for row in 0..h {
        let sy = y + row;
        if sy >= src_h {
            break;
        }
        let src_off = ((sy * src_w + x) as usize) * 4;
        let dst_off = (row as usize) * (w as usize) * 4;
        let count = ((w.min(src_w.saturating_sub(x))) as usize) * 4;
        if src_off + count <= src.len() {
            out[dst_off..dst_off + count].copy_from_slice(&src[src_off..src_off + count]);
        }
    }
    out
}
