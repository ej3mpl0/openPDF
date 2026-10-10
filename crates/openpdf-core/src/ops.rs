use std::collections::BTreeMap;
use std::path::Path;

use lopdf::{Dictionary, Document as LopdfDocument, Object, ObjectId};

use crate::document::Document;
use crate::error::{Error, InvalidKind, Result};
use crate::signer::strip_signatures;

/// Inclusive 0-based page range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRange {
    pub start: u32,
    pub end: u32,
}

impl PageRange {
    pub fn new(start: u32, end: u32) -> Result<Self> {
        if start > end {
            return Err(Error::invalid(InvalidKind::RangeInverted { start, end }));
        }
        Ok(Self { start, end })
    }

    pub fn single(page: u32) -> Self {
        Self {
            start: page,
            end: page,
        }
    }

    pub fn iter(self) -> impl Iterator<Item = u32> {
        self.start..=self.end
    }
}

/// Parse a 1-based CLI spec such as `1-3,5,7-8` into 0-based inclusive ranges.
pub fn parse_page_ranges(spec: &str, page_count: u32) -> Result<Vec<PageRange>> {
    if page_count == 0 {
        return Err(Error::EmptyDocument);
    }
    let mut ranges = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (a, b) = if let Some((l, r)) = part.split_once('-') {
            (parse_one(l)?, parse_one(r)?)
        } else {
            let n = parse_one(part)?;
            (n, n)
        };
        if a == 0 || b == 0 {
            return Err(Error::invalid(InvalidKind::PageNumbersOneBased));
        }
        let start = a - 1;
        let end = b - 1;
        if end >= page_count {
            return Err(Error::PageOutOfRange {
                page: end,
                count: page_count,
            });
        }
        ranges.push(PageRange::new(start, end)?);
    }
    if ranges.is_empty() {
        return Err(Error::invalid(InvalidKind::NoPageRanges));
    }
    Ok(ranges)
}

fn parse_one(s: &str) -> Result<u32> {
    s.trim()
        .parse()
        .map_err(|_| Error::invalid(InvalidKind::NotAPageNumber { value: s.to_string() }))
}

/// Concatenate documents in order. Uses lopdf (not PDFium) so gtk/sign stay off this path.
pub fn merge(docs: &[Document]) -> Result<Document> {
    if docs.is_empty() {
        return Err(Error::EmptyDocument);
    }
    if docs.len() == 1 {
        return Ok(docs[0].clone());
    }

    let mut max_id = 1u32;
    let mut pages: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut objects: BTreeMap<ObjectId, Object> = BTreeMap::new();

    for doc in docs {
        let mut part = doc.lopdf_clone();
        part.renumber_objects_with(max_id);
        max_id = part.max_id + 1;
        for (_num, id) in part.get_pages() {
            let obj = part
                .get_object(id)
                .map_err(|e| Error::structure(e.to_string()))?
                .to_owned();
            pages.insert(id, obj);
        }
        objects.extend(part.objects);
    }

    let mut catalog_object: Option<(ObjectId, Object)> = None;
    let mut pages_object: Option<(ObjectId, Object)> = None;

    for (id, object) in objects.iter() {
        match object.type_name().map(|n| n.as_ref()) {
            Ok(b"Catalog") => {
                if catalog_object.is_none() {
                    catalog_object = Some((*id, object.clone()));
                }
            }
            Ok(b"Pages") => {
                if pages_object.is_none() {
                    pages_object = Some((*id, object.clone()));
                }
            }
            _ => {}
        }
    }

    let (catalog_id, catalog) =
        catalog_object.ok_or_else(|| Error::structure("missing Catalog"))?;
    let (pages_id, mut pages_dict) =
        pages_object.ok_or_else(|| Error::structure("missing Pages"))?;

    let dictionary = pages_dict
        .as_dict_mut()
        .map_err(|e| Error::structure(e.to_string()))?;
    dictionary.set(
        "Kids",
        Object::Array(pages.keys().copied().map(Object::Reference).collect()),
    );
    dictionary.set("Count", Object::Integer(pages.len() as i64));

    let mut catalog_dict = catalog;
    catalog_dict
        .as_dict_mut()
        .map_err(|e| Error::structure(e.to_string()))?
        .set("Pages", Object::Reference(pages_id));
    // Merged docs do not share a single outline tree.
    catalog_dict
        .as_dict_mut()
        .ok()
        .map(|d| d.remove(b"Outlines"));

    let mut out = LopdfDocument::with_version("1.5");
    out.objects = objects;
    for (id, obj) in pages {
        out.objects.insert(id, obj);
    }
    out.objects.insert(pages_id, pages_dict);
    out.objects.insert(catalog_id, catalog_dict);
    out.max_id = out.objects.keys().map(|id| id.0).max().unwrap_or(1);
    out.trailer.set("Root", Object::Reference(catalog_id));
    // Merge is a full rewrite; copied signature dictionaries would lock the
    // new file while no longer matching any ByteRange.
    strip_signatures(&mut out);

    Document::from_lopdf(out, None)
}

pub fn merge_paths<P: AsRef<Path>>(paths: &[P]) -> Result<Document> {
    let docs: Result<Vec<Document>> = paths.iter().map(|p| Document::open(p.as_ref())).collect();
    merge(&docs?)
}

/// Extract each range into its own document (full rewrite via lopdf).
pub fn split(doc: &Document, ranges: &[PageRange]) -> Result<Vec<Document>> {
    if ranges.is_empty() {
        return split_all(doc);
    }
    let mut out = Vec::with_capacity(ranges.len());
    for range in ranges {
        out.push(extract_pages(doc, range)?);
    }
    Ok(out)
}

pub fn split_all(doc: &Document) -> Result<Vec<Document>> {
    let n = doc.page_count();
    if n == 0 {
        return Err(Error::EmptyDocument);
    }
    let ranges: Vec<PageRange> = (0..n).map(PageRange::single).collect();
    split(doc, &ranges)
}

pub(crate) fn extract_pages(doc: &Document, range: &PageRange) -> Result<Document> {
    let count = doc.page_count();
    if range.end >= count {
        return Err(Error::PageOutOfRange {
            page: range.end,
            count,
        });
    }
    let mut src = doc.lopdf_clone();
    let pages = src.get_pages();
    let keep: Vec<ObjectId> = range
        .iter()
        .map(|i| {
            pages
                .get(&(i + 1))
                .copied()
                .ok_or(Error::PageOutOfRange { page: i, count })
        })
        .collect::<Result<Vec<_>>>()?;

    let pages_id = src
        .catalog()?
        .get(b"Pages")?
        .as_reference()
        .map_err(|e| Error::structure(e.to_string()))?;
    {
        let pages_obj = src.get_object_mut(pages_id)?;
        let dict: &mut Dictionary = pages_obj
            .as_dict_mut()
            .map_err(|e| Error::structure(e.to_string()))?;
        dict.set(
            "Kids",
            Object::Array(keep.iter().copied().map(Object::Reference).collect()),
        );
        dict.set("Count", Object::Integer(keep.len() as i64));
    }
    for id in &keep {
        if let Ok(Object::Dictionary(dict)) = src.get_object_mut(*id) {
            dict.set("Parent", Object::Reference(pages_id));
        }
    }
    strip_signatures(&mut src);

    Document::from_lopdf(src, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::corpus;

    #[test]
    fn parse_ranges() {
        let r = parse_page_ranges("1-2,4", 5).unwrap();
        assert_eq!(
            r,
            vec![
                PageRange { start: 0, end: 1 },
                PageRange { start: 3, end: 3 }
            ]
        );
        assert!(matches!(
            parse_page_ranges("0-1", 5),
            Err(Error::Invalid(InvalidKind::PageNumbersOneBased))
        ));
        assert!(matches!(
            parse_page_ranges("foo", 5),
            Err(Error::Invalid(InvalidKind::NotAPageNumber { .. }))
        ));
    }

    #[test]
    fn merge_and_split_corpus() {
        let a = Document::open(corpus("two_page.pdf")).unwrap();
        let b = Document::open(corpus("hello.pdf")).unwrap();
        let merged = merge(&[a, b]).unwrap();
        assert_eq!(merged.page_count(), 3);
        let parts = split_all(&merged).unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].page_count(), 1);
        let first_two = split(
            &merged,
            &[PageRange { start: 0, end: 1 }],
        )
        .unwrap();
        assert_eq!(first_two[0].page_count(), 2);
    }

    #[test]
    fn merge_of_signed_source_is_unsigned_and_editable() {
        let signed_bytes =
            crate::signer::append_stub_signature(&std::fs::read(corpus("hello.pdf")).unwrap())
                .unwrap();
        let signed = Document::from_bytes(signed_bytes, None).unwrap();
        assert!(signed.is_signed());
        let other = Document::open(corpus("two_page.pdf")).unwrap();
        let mut merged = merge(&[signed, other]).unwrap();
        assert!(!merged.is_signed());
        assert!(!merged.is_read_only());
        merged.rotate_page(0, 90).unwrap();
        merged.save(std::env::temp_dir().join(format!(
            "openpdf-merge-signed-{}.pdf",
            std::process::id()
        )))
        .unwrap();
    }
}
