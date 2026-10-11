use crate::document::Document;
use crate::error::{Error, InvalidKind, Result, SignedOp};
use crate::ops::{extract_pages, merge, PageRange};

/// Extract `ranges` into one new document (unsigned rewrite).
pub fn extract(doc: &Document, ranges: &[PageRange]) -> Result<Document> {
    if ranges.is_empty() {
        return Err(Error::invalid(InvalidKind::NoPageRanges));
    }
    if ranges.len() == 1 {
        return extract_pages(doc, &ranges[0]);
    }
    let parts: Result<Vec<Document>> = ranges.iter().map(|r| extract_pages(doc, r)).collect();
    merge(&parts?)
}

/// Insert `source` pages (`ranges`) into `target` at 0-based index `at`.
pub fn insert_pages(
    target: &Document,
    at: u32,
    source: &Document,
    ranges: &[PageRange],
) -> Result<Document> {
    if target.is_signed() {
        return Err(Error::signed(SignedOp::Edit));
    }
    if at > target.page_count() {
        return Err(Error::PageOutOfRange {
            page: at,
            count: target.page_count(),
        });
    }
    let incoming = extract(source, ranges)?;
    if at == 0 {
        return merge(&[incoming, target.clone()]);
    }
    if at == target.page_count() {
        return merge(&[target.clone(), incoming]);
    }
    let before = extract_pages(target, &PageRange::new(0, at - 1)?)?;
    let after = extract_pages(target, &PageRange::new(at, target.page_count() - 1)?)?;
    merge(&[before, incoming, after])
}

/// Replace `count` pages of `target` starting at `start` with `source` pages.
pub fn replace_pages(
    target: &Document,
    start: u32,
    count: u32,
    source: &Document,
    ranges: &[PageRange],
) -> Result<Document> {
    if target.is_signed() {
        return Err(Error::signed(SignedOp::Edit));
    }
    if count == 0 {
        return insert_pages(target, start, source, ranges);
    }
    let end = start.saturating_add(count);
    if start >= target.page_count() || end > target.page_count() {
        return Err(Error::PageOutOfRange {
            page: end.saturating_sub(1),
            count: target.page_count(),
        });
    }
    let incoming = extract(source, ranges)?;
    let mut parts = Vec::new();
    if start > 0 {
        parts.push(extract_pages(target, &PageRange::new(0, start - 1)?)?);
    }
    parts.push(incoming);
    if end < target.page_count() {
        parts.push(extract_pages(
            target,
            &PageRange::new(end, target.page_count() - 1)?,
        )?);
    }
    merge(&parts)
}

/// Inclusive crop box in PDF points (`llx, lly, urx, ury`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropBox {
    pub llx: f32,
    pub lly: f32,
    pub urx: f32,
    pub ury: f32,
}

impl CropBox {
    pub fn new(llx: f32, lly: f32, urx: f32, ury: f32) -> Result<Self> {
        if llx >= urx || lly >= ury {
            return Err(Error::invalid(InvalidKind::CropInverted));
        }
        Ok(Self { llx, lly, urx, ury })
    }
}

/// Set `/CropBox` on the selected pages (full rewrite).
pub fn crop_pages(doc: &Document, ranges: &[PageRange], crop: CropBox) -> Result<Document> {
    if doc.is_signed() {
        return Err(Error::signed(SignedOp::Edit));
    }
    let mut inner = doc.assembled_lopdf()?;
    let pages = inner.get_pages();
    let mut targets = Vec::new();
    if ranges.is_empty() {
        targets.extend(0..doc.page_count());
    } else {
        for range in ranges {
            targets.extend(range.iter());
        }
    }
    for i in targets {
        let id = pages.get(&(i + 1)).copied().ok_or(Error::PageOutOfRange {
            page: i,
            count: doc.page_count(),
        })?;
        if let Ok(obj) = inner.get_object_mut(id) {
            if let Ok(dict) = obj.as_dict_mut() {
                dict.set(
                    "CropBox",
                    vec![
                        lopdf::Object::Real(crop.llx),
                        lopdf::Object::Real(crop.lly),
                        lopdf::Object::Real(crop.urx),
                        lopdf::Object::Real(crop.ury),
                    ],
                );
            }
        }
    }
    doc.derive(inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signer::append_stub_signature;
    use crate::test_support::corpus;

    #[test]
    fn insert_extract_replace_and_crop() {
        let two = Document::open(corpus("two_page.pdf")).unwrap();
        let hello = Document::open(corpus("hello.pdf")).unwrap();
        let inserted = insert_pages(&two, 1, &hello, &[PageRange::single(0)]).unwrap();
        assert_eq!(inserted.page_count(), 3);
        let extracted = extract(&inserted, &[PageRange::single(1)]).unwrap();
        assert_eq!(extracted.page_count(), 1);
        let replaced = replace_pages(&two, 0, 1, &hello, &[PageRange::single(0)]).unwrap();
        assert_eq!(replaced.page_count(), 2);
        let cropped = crop_pages(
            &hello,
            &[PageRange::single(0)],
            CropBox::new(10.0, 10.0, 200.0, 200.0).unwrap(),
        )
        .unwrap();
        assert_eq!(cropped.page_count(), 1);
    }

    #[test]
    fn signed_target_refuses_insert_and_crop() {
        let bytes = append_stub_signature(&std::fs::read(corpus("two_page.pdf")).unwrap()).unwrap();
        let signed = Document::from_bytes(bytes, None).unwrap();
        let hello = Document::open(corpus("hello.pdf")).unwrap();
        assert!(matches!(
            insert_pages(&signed, 0, &hello, &[PageRange::single(0)]),
            Err(Error::Signed { op: SignedOp::Edit })
        ));
        assert!(matches!(
            crop_pages(&signed, &[], CropBox::new(0.0, 0.0, 100.0, 100.0).unwrap()),
            Err(Error::Signed { op: SignedOp::Edit })
        ));
        // Extract from a signed file is a new unsigned document.
        let part = extract(&signed, &[PageRange::single(0)]).unwrap();
        assert!(!part.is_signed());
    }
}
