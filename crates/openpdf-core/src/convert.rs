use std::io::Cursor;
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::ExtendedColorType;
use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Dictionary, Object, Stream};

use crate::document::Document;
use crate::error::{Error, InvalidKind, Result};
use crate::render::{Engine, Raster};

/// Build a PDF with `page_count` blank letter pages. Used by tests and benches.
pub fn blank_document(page_count: u32) -> Result<Document> {
    if page_count == 0 {
        return Err(Error::EmptyDocument);
    }
    let mut doc = lopdf::Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });
    let mut kids = Vec::new();
    for i in 0..page_count {
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 18.into()]),
                Operation::new("Td", vec![72.into(), 720.into()]),
                Operation::new(
                    "Tj",
                    vec![Object::string_literal(format!("Page {}", i + 1))],
                ),
                Operation::new("ET", vec![]),
            ],
        };
        let encoded = content
            .encode()
            .map_err(|e| Error::structure(e.to_string()))?;
        let content_id = doc.add_object(Stream::new(Dictionary::new(), encoded));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        kids.push(page_id.into());
    }
    let pages = dictionary! {
        "Type" => "Pages",
        "Kids" => kids,
        "Count" => page_count as i64,
        "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    };
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    Document::from_lopdf(doc, None)
}

/// One 72×72 pt page filled with DeviceRGB red (`1 0 0 rg`). For colour tests.
pub fn red_rect_document() -> Result<Document> {
    let mut doc = lopdf::Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let content = Content {
        operations: vec![
            Operation::new("rg", vec![1.into(), 0.into(), 0.into()]),
            Operation::new("re", vec![0.into(), 0.into(), 72.into(), 72.into()]),
            Operation::new("f", vec![]),
        ],
    };
    let encoded = content
        .encode()
        .map_err(|e| Error::structure(e.to_string()))?;
    let content_id = doc.add_object(Stream::new(Dictionary::new(), encoded));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "MediaBox" => vec![0.into(), 0.into(), 72.into(), 72.into()],
    });
    let pages = dictionary! {
        "Type" => "Pages",
        "Kids" => vec![page_id.into()],
        "Count" => 1,
        "MediaBox" => vec![0.into(), 0.into(), 72.into(), 72.into()],
    };
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    Document::from_lopdf(doc, None)
}

/// One page per image, scaled to fit US Letter.
pub fn images_to_pdf<P: AsRef<Path>>(paths: &[P]) -> Result<Document> {
    if paths.is_empty() {
        return Err(Error::invalid(InvalidKind::NoInputImages));
    }
    let mut doc = lopdf::Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();
    const PW: f32 = 612.0;
    const PH: f32 = 792.0;
    const MARGIN: f32 = 36.0;

    for path in paths {
        let bytes = std::fs::read(path.as_ref())?;
        let dyn_img =
            image::load_from_memory(&bytes).map_err(|e| Error::structure(e.to_string()))?;
        let rgb = dyn_img.to_rgb8();
        let (iw, ih) = rgb.dimensions();
        if iw == 0 || ih == 0 {
            return Err(Error::structure("image has zero size"));
        }
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 85)
            .encode(rgb.as_raw(), iw, ih, ExtendedColorType::Rgb8)
            .map_err(|e| Error::structure(e.to_string()))?;
        let mut img_dict = Dictionary::new();
        img_dict.set("Type", Object::Name(b"XObject".to_vec()));
        img_dict.set("Subtype", Object::Name(b"Image".to_vec()));
        img_dict.set("Width", iw as i64);
        img_dict.set("Height", ih as i64);
        img_dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        img_dict.set("BitsPerComponent", 8);
        img_dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
        let img_id = doc.add_object(Stream::new(img_dict, jpeg));

        let max_w = PW - 2.0 * MARGIN;
        let max_h = PH - 2.0 * MARGIN;
        let scale = (max_w / iw as f32).min(max_h / ih as f32).min(1.0);
        let dw = iw as f32 * scale;
        let dh = ih as f32 * scale;
        let x = (PW - dw) / 2.0;
        let y = (PH - dh) / 2.0;
        let content = Content {
            operations: vec![
                Operation::new("q", vec![]),
                Operation::new(
                    "cm",
                    vec![dw.into(), 0.into(), 0.into(), dh.into(), x.into(), y.into()],
                ),
                Operation::new("Do", vec!["Im0".into()]),
                Operation::new("Q", vec![]),
            ],
        };
        let encoded = content
            .encode()
            .map_err(|e| Error::structure(e.to_string()))?;
        let content_id = doc.add_object(Stream::new(Dictionary::new(), encoded));
        let resources = dictionary! {
            "XObject" => dictionary! { "Im0" => img_id },
        };
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        });
        kids.push(page_id.into());
    }

    let pages = dictionary! {
        "Type" => "Pages",
        "Kids" => kids,
        "Count" => paths.len() as i64,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    };
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    Document::from_lopdf(doc, None)
}

/// Render every page through PDFium.
pub fn pdf_to_rasters(engine: &Engine, doc: &Document, zoom: f32) -> Result<Vec<Raster>> {
    engine.bind(doc)?;
    let mut out = Vec::with_capacity(doc.page_count() as usize);
    for i in 0..doc.page_count() {
        out.push(engine.render_page(doc.id(), i, zoom)?);
    }
    Ok(out)
}

/// Encode a raster as PNG bytes.
pub fn raster_to_png(raster: &Raster) -> Result<Vec<u8>> {
    let img = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(
        raster.width,
        raster.height,
        raster.rgba.clone(),
    )
    .ok_or_else(|| Error::structure("RGBA buffer size"))?;
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| Error::structure(e.to_string()))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::corpus;

    #[test]
    fn blank_and_images_roundtrip() {
        let doc = blank_document(3).unwrap();
        assert_eq!(doc.page_count(), 3);
        let dir = std::env::temp_dir().join(format!("openpdf-img-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let png = dir.join("dot.png");
        // 2x2 red PNG
        let img = image::RgbImage::from_pixel(2, 2, image::Rgb([200, 10, 10]));
        img.save(&png).unwrap();
        let pdf = images_to_pdf(&[png]).unwrap();
        assert_eq!(pdf.page_count(), 1);
    }

    #[test]
    fn corpus_stays_openable() {
        assert_eq!(Document::open(corpus("hello.pdf")).unwrap().page_count(), 1);
    }

    #[test]
    fn red_rect_document_is_one_page() {
        let doc = red_rect_document().unwrap();
        assert_eq!(doc.page_count(), 1);
        let page = doc.page(0).unwrap();
        assert!((page.width_pt - 72.0).abs() < 0.01);
        assert!((page.height_pt - 72.0).abs() < 0.01);
    }

    /// `set_reverse_byte_order(true)` stores RGBA; white fill ⇒ alpha 255.
    /// A DeviceRGB red rect must stay red (R high, B low), not swapped.
    #[test]
    fn red_rect_tiles_are_opaque_rgba_red() {
        let Ok(engine) = Engine::new() else {
            eprintln!("red_rect colour: Engine::new skipped (no PDFium)");
            return;
        };
        let doc = red_rect_document().unwrap();
        engine.bind(&doc).unwrap();
        let raster = engine.render_page(doc.id(), 0, 1.0).unwrap();
        assert!(raster.width > 0 && raster.height > 0);
        assert!(
            raster.rgba.chunks_exact(4).all(|p| p[3] == 255),
            "R8g8b8x8 requires opaque tiles (alpha 255)"
        );
        let cx = (raster.width / 2) as usize;
        let cy = (raster.height / 2) as usize;
        let i = (cy * raster.width as usize + cx) * 4;
        let (r, g, b, a) = (
            raster.rgba[i],
            raster.rgba[i + 1],
            raster.rgba[i + 2],
            raster.rgba[i + 3],
        );
        assert_eq!(a, 255);
        assert!(
            r > 200 && b < 40,
            "center pixel must be red not blue (R={r} G={g} B={b})"
        );
    }
}
