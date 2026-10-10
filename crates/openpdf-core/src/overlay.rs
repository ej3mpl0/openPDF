use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Object, ObjectId};

use crate::document::Document;
use crate::error::{Error, InvalidKind, Result, SignedOp};
use crate::ops::PageRange;

/// Where to place overlay text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayKind {
    Watermark,
    Header,
    Footer,
    PageNumber,
}

#[derive(Clone, Debug)]
pub struct Overlay {
    pub text: String,
    pub kind: OverlayKind,
    pub pages: Vec<PageRange>,
}

impl Overlay {
    pub fn new(kind: OverlayKind, text: impl Into<String>) -> Result<Self> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(Error::invalid(InvalidKind::EmptyOverlayText));
        }
        Ok(Self {
            text,
            kind,
            pages: Vec::new(),
        })
    }
}

/// Stamp text (watermark / header / footer / page numbers). `{page}` and `{total}` expand.
pub fn apply_overlay(doc: &Document, overlay: &Overlay) -> Result<Document> {
    if doc.is_signed() {
        return Err(Error::signed(SignedOp::Edit));
    }
    let mut inner = doc.assembled_lopdf()?;
    let page_map = inner.get_pages();
    let total = doc.page_count();
    let mut indices: Vec<u32> = if overlay.pages.is_empty() {
        (0..total).collect()
    } else {
        overlay.pages.iter().flat_map(|r| r.iter()).collect()
    };
    indices.sort_unstable();
    indices.dedup();

    let font_id = helvetica_id(&mut inner);

    for i in indices {
        let page_id = page_map
            .get(&(i + 1))
            .copied()
            .ok_or(Error::PageOutOfRange {
                page: i,
                count: total,
            })?;
        ensure_font(&mut inner, page_id, font_id)?;
        let (w, h) = page_size(&inner, page_id).unwrap_or((612.0, 792.0));
        let text = overlay
            .text
            .replace("{page}", &(i + 1).to_string())
            .replace("{total}", &total.to_string());
        let (x, y, size, gray) = match overlay.kind {
            OverlayKind::Watermark => (w * 0.22, h * 0.48, 36.0, 0.72),
            OverlayKind::Header => (w * 0.12, h - 28.0, 10.0, 0.15),
            OverlayKind::Footer | OverlayKind::PageNumber => (w * 0.12, 18.0, 10.0, 0.15),
        };
        let content = Content {
            operations: vec![
                Operation::new("q", vec![]),
                Operation::new("rg", vec![gray.into(), gray.into(), gray.into()]),
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["Helv".into(), size.into()]),
                Operation::new("Td", vec![x.into(), y.into()]),
                Operation::new("Tj", vec![Object::string_literal(text.as_str())]),
                Operation::new("ET", vec![]),
                Operation::new("Q", vec![]),
            ],
        };
        let bytes = content
            .encode()
            .map_err(|e| Error::structure(e.to_string()))?;
        inner
            .add_page_contents(page_id, bytes)
            .map_err(|e| Error::structure(e.to_string()))?;
    }
    Document::from_lopdf(inner, None)
}

fn helvetica_id(doc: &mut lopdf::Document) -> ObjectId {
    let mut dict = Dictionary::new();
    dict.set("Type", Object::Name(b"Font".to_vec()));
    dict.set("Subtype", Object::Name(b"Type1".to_vec()));
    dict.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
    doc.add_object(Object::Dictionary(dict))
}

fn ensure_font(doc: &mut lopdf::Document, page_id: ObjectId, font_id: ObjectId) -> Result<()> {
    let page = doc
        .get_object_mut(page_id)
        .map_err(|e| Error::structure(e.to_string()))?;
    let dict = page
        .as_dict_mut()
        .map_err(|e| Error::structure(e.to_string()))?;
    let mut resources = match dict.get(b"Resources") {
        Ok(Object::Dictionary(d)) => d.clone(),
        Ok(Object::Reference(id)) => {
            let id = *id;
            return ensure_font_in_resources(doc, id, font_id);
        }
        _ => Dictionary::new(),
    };
    let mut fonts = match resources.get(b"Font") {
        Ok(Object::Dictionary(d)) => d.clone(),
        _ => Dictionary::new(),
    };
    fonts.set("Helv", Object::Reference(font_id));
    resources.set("Font", Object::Dictionary(fonts));
    dict.set("Resources", Object::Dictionary(resources));
    Ok(())
}

fn ensure_font_in_resources(
    doc: &mut lopdf::Document,
    resources_id: ObjectId,
    font_id: ObjectId,
) -> Result<()> {
    let obj = doc
        .get_object_mut(resources_id)
        .map_err(|e| Error::structure(e.to_string()))?;
    let resources = obj
        .as_dict_mut()
        .map_err(|e| Error::structure(e.to_string()))?;
    let mut fonts = match resources.get(b"Font") {
        Ok(Object::Dictionary(d)) => d.clone(),
        _ => Dictionary::new(),
    };
    fonts.set("Helv", Object::Reference(font_id));
    resources.set("Font", Object::Dictionary(fonts));
    Ok(())
}

fn page_size(doc: &lopdf::Document, mut id: ObjectId) -> Option<(f32, f32)> {
    for _ in 0..16 {
        let dict = doc.get_object(id).ok()?.as_dict().ok()?;
        if let Ok(mb) = dict.get(b"MediaBox") {
            let arr = mb.as_array().ok()?;
            if arr.len() < 4 {
                return None;
            }
            let n = |i: usize| match arr[i] {
                Object::Integer(v) => Some(v as f32),
                Object::Real(v) => Some(v),
                _ => None,
            };
            let w = (n(2)? - n(0)?).abs();
            let h = (n(3)? - n(1)?).abs();
            return Some((w, h));
        }
        id = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signer::append_stub_signature;
    use crate::test_support::corpus;

    #[test]
    fn watermark_and_numbers() {
        let doc = Document::open(corpus("two_page.pdf")).unwrap();
        let marked = apply_overlay(
            &doc,
            &Overlay::new(OverlayKind::Watermark, "DRAFT").unwrap(),
        )
        .unwrap();
        assert_eq!(marked.page_count(), 2);
        let numbered = apply_overlay(
            &marked,
            &Overlay::new(OverlayKind::PageNumber, "{page} / {total}").unwrap(),
        )
        .unwrap();
        assert_eq!(numbered.page_count(), 2);
    }

    #[test]
    fn overlay_refuses_signed() {
        let bytes = append_stub_signature(&std::fs::read(corpus("hello.pdf")).unwrap()).unwrap();
        let signed = Document::from_bytes(bytes, None).unwrap();
        assert!(matches!(
            apply_overlay(&signed, &Overlay::new(OverlayKind::Header, "x").unwrap()),
            Err(Error::Signed { op: SignedOp::Edit })
        ));
    }
}
