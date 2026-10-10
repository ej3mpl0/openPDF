use image::codecs::jpeg::JpegEncoder;
use image::ExtendedColorType;
use lopdf::{Dictionary, Object, Stream};

use crate::document::Document;
use crate::error::{Error, Result, SignedOp};

/// Options for rewrite-optimize (images, unused objects, linearized hint).
#[derive(Clone, Debug)]
pub struct OptimizeOptions {
    /// JPEG quality 1–100 when recompressing raw image XObjects. `None` skips recompress.
    pub jpeg_quality: Option<u8>,
    pub drop_unused: bool,
    pub linearize: bool,
}

impl Default for OptimizeOptions {
    fn default() -> Self {
        Self {
            jpeg_quality: Some(75),
            drop_unused: true,
            linearize: true,
        }
    }
}

/// Recompress images, prune unused objects, optionally stamp a linearized hint.
pub fn optimize(doc: &Document, opts: &OptimizeOptions) -> Result<Document> {
    if doc.is_signed() {
        return Err(Error::signed(SignedOp::Rewrite));
    }
    let mut inner = doc.assembled_lopdf()?;
    if let Some(q) = opts.jpeg_quality {
        recompress_images(&mut inner, q.clamp(1, 100))?;
    }
    if opts.drop_unused {
        inner.prune_objects();
        inner.renumber_objects();
    }
    inner.compress();
    if opts.linearize {
        stamp_linearized(&mut inner, doc.page_count());
    }
    Document::from_lopdf(inner, None)
}

fn recompress_images(doc: &mut lopdf::Document, quality: u8) -> Result<()> {
    let ids: Vec<_> = doc.objects.keys().copied().collect();
    for id in ids {
        let Some(stream) = doc.objects.get(&id).and_then(|o| o.as_stream().ok()) else {
            continue;
        };
        if !is_image(stream) {
            continue;
        }
        if is_jpeg(stream) {
            continue;
        }
        let Some((w, h, rgb)) = rgb8_from_image(stream) else {
            continue;
        };
        let mut jpeg = Vec::new();
        {
            let mut enc = JpegEncoder::new_with_quality(&mut jpeg, quality);
            if enc.encode(&rgb, w, h, ExtendedColorType::Rgb8).is_err() {
                continue;
            }
        }
        let mut dict = stream.dict.clone();
        dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
        dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        dict.set("BitsPerComponent", 8);
        dict.set("Width", w as i64);
        dict.set("Height", h as i64);
        dict.remove(b"DecodeParms");
        doc.objects
            .insert(id, Object::Stream(Stream::new(dict, jpeg)));
    }
    Ok(())
}

fn is_image(stream: &Stream) -> bool {
    stream
        .dict
        .get(b"Subtype")
        .ok()
        .and_then(|o| o.as_name().ok())
        .is_some_and(|n| n == b"Image")
}

fn is_jpeg(stream: &Stream) -> bool {
    match stream.dict.get(b"Filter") {
        Ok(Object::Name(n)) => n.as_slice() == b"DCTDecode",
        Ok(Object::Array(arr)) => arr
            .iter()
            .any(|o| o.as_name().ok().is_some_and(|n| n == b"DCTDecode")),
        _ => false,
    }
}

fn rgb8_from_image(stream: &Stream) -> Option<(u32, u32, Vec<u8>)> {
    let w = stream.dict.get(b"Width").ok()?.as_i64().ok()? as u32;
    let h = stream.dict.get(b"Height").ok()?.as_i64().ok()? as u32;
    if w == 0 || h == 0 {
        return None;
    }
    let bpc = stream
        .dict
        .get(b"BitsPerComponent")
        .ok()
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(8);
    if bpc != 8 {
        return None;
    }
    let data = stream.decompressed_content().ok()?;
    let space = stream
        .dict
        .get(b"ColorSpace")
        .ok()
        .and_then(|o| o.as_name().ok())
        .unwrap_or(b"DeviceRGB");
    match space {
        b"DeviceRGB" if data.len() == (w * h * 3) as usize => Some((w, h, data)),
        b"DeviceGray" if data.len() == (w * h) as usize => {
            let mut rgb = Vec::with_capacity(data.len() * 3);
            for g in data {
                rgb.extend_from_slice(&[g, g, g]);
            }
            Some((w, h, rgb))
        }
        _ => None,
    }
}

fn stamp_linearized(doc: &mut lopdf::Document, pages: u32) {
    let mut dict = Dictionary::new();
    dict.set("Linearized", 1);
    dict.set("N", pages as i64);
    let id = doc.add_object(Object::Dictionary(dict));
    let _ = id;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signer::append_stub_signature;
    use crate::test_support::corpus;

    #[test]
    fn optimize_keeps_pages() {
        let doc = Document::open(corpus("two_page.pdf")).unwrap();
        let out = optimize(&doc, &OptimizeOptions::default()).unwrap();
        assert_eq!(out.page_count(), 2);
        assert!(!out.is_signed());
    }

    #[test]
    fn optimize_refuses_signed() {
        let bytes = append_stub_signature(&std::fs::read(corpus("hello.pdf")).unwrap()).unwrap();
        let signed = Document::from_bytes(bytes, None).unwrap();
        assert!(matches!(
            optimize(&signed, &OptimizeOptions::default()),
            Err(Error::Signed {
                op: SignedOp::Rewrite
            })
        ));
    }
}
