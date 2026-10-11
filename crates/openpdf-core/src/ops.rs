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

    match merge_encryption_source(docs) {
        Some(i) => docs[i].derive_encrypted(out),
        None => Document::from_lopdf(out, None),
    }
}

/// Which input's encryption a [`merge`] output inherits: the first input that
/// was opened with a password (its algorithm, key, permissions and password),
/// or `None` when no input is protected and the output is plaintext.
///
/// This is the single rule `merge` (and multi-range `extract`) apply; UIs can
/// call it to tell the user up front which file's protection the result gets.
pub fn merge_encryption_source(inputs: &[Document]) -> Option<usize> {
    inputs.iter().position(|d| d.password().get().is_some())
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

    doc.derive(src)
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

#[cfg(test)]
mod encryption_tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use lopdf::encryption::crypt_filters::{Aes128CryptFilter, CryptFilter};
    use lopdf::{EncryptionState, EncryptionVersion, Object, Permissions};
    use zeroize::Zeroizing;

    use super::*;
    use crate::optimize::{optimize, OptimizeOptions};
    use crate::protect::{protect, PdfPermissions, ProtectOptions};
    use crate::test_support::corpus;

    const PW: &str = "user-secret";

    fn plain() -> Document {
        Document::open(corpus("two_page.pdf")).unwrap()
    }

    fn reopen(bytes: Vec<u8>) -> Document {
        Document::from_bytes_with_password(bytes, None, Zeroizing::new(PW.into())).unwrap()
    }

    fn perms() -> Permissions {
        Permissions::PRINTABLE | Permissions::COPYABLE_FOR_ACCESSIBILITY
    }

    /// AES-256 (V5/R6) via our own `protect`, with restricted permissions.
    fn aes256() -> Document {
        let mut opts = ProtectOptions::new(PW, "owner").unwrap();
        opts.permissions = PdfPermissions::parse_list("print").unwrap();
        reopen(protect(&plain(), &opts).unwrap().to_bytes().unwrap())
    }

    /// Encrypt with an older lopdf version (RC4 / AES-128), which needs /ID.
    fn legacy(make: impl Fn(&lopdf::Document) -> EncryptionVersion<'_>) -> Document {
        let mut inner = plain().assembled_lopdf().unwrap();
        inner.trailer.set(
            "ID",
            vec![
                Object::string_literal(b"0123456789abcdef".to_vec()),
                Object::string_literal(b"0123456789abcdef".to_vec()),
            ],
        );
        let state = EncryptionState::try_from(make(&inner)).unwrap();
        inner.encrypt(&state).unwrap();
        let mut bytes = Vec::new();
        inner.save_to(&mut bytes).unwrap();
        reopen(bytes)
    }

    fn rc4_128() -> Document {
        legacy(|d| EncryptionVersion::V2 {
            document: d,
            owner_password: "owner",
            user_password: PW,
            key_length: 128,
            permissions: perms(),
        })
    }

    fn aes128() -> Document {
        legacy(|d| {
            let f: Arc<dyn CryptFilter> = Arc::new(Aes128CryptFilter);
            EncryptionVersion::V4 {
                document: d,
                encrypt_metadata: true,
                crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), f)]),
                stream_filter: b"StdCF".to_vec(),
                string_filter: b"StdCF".to_vec(),
                owner_password: "owner",
                user_password: PW,
                permissions: perms(),
            }
        })
    }

    /// The parts of /Encrypt that define algorithm, key and permissions.
    fn encrypt_profile(bytes: &[u8]) -> Vec<(String, String)> {
        let doc = lopdf::Document::load_mem(bytes).unwrap();
        let enc = doc.get_encrypted().expect("output must be encrypted");
        let mut out = Vec::new();
        for key in [
            &b"Filter"[..],
            b"V",
            b"R",
            b"Length",
            b"P",
            b"O",
            b"U",
            b"OE",
            b"UE",
            b"Perms",
            b"StmF",
            b"StrF",
            b"EncryptMetadata",
        ] {
            let v = enc.get(key).map(|o| format!("{o:?}")).unwrap_or_default();
            out.push((String::from_utf8_lossy(key).into_owned(), v));
        }
        if let Ok(cf) = enc.get(b"CF").and_then(Object::as_dict) {
            for (name, f) in cf.iter() {
                let cfm = f
                    .as_dict()
                    .ok()
                    .and_then(|d| d.get(b"CFM").ok())
                    .map(|o| format!("{o:?}"))
                    .unwrap_or_default();
                out.push((format!("CF/{}", String::from_utf8_lossy(name)), cfm));
            }
        }
        out
    }

    fn p_value(bytes: &[u8]) -> i64 {
        lopdf::Document::load_mem(bytes)
            .unwrap()
            .get_encrypted()
            .unwrap()
            .get(b"P")
            .unwrap()
            .as_i64()
            .unwrap()
    }

    #[test]
    fn merge_encryption_source_rule() {
        assert_eq!(merge_encryption_source(&[plain(), plain()]), None);
        assert_eq!(merge_encryption_source(&[plain(), aes256()]), Some(1));
        assert_eq!(merge_encryption_source(&[aes256(), rc4_128()]), Some(0));
        assert_eq!(merge_encryption_source(&[]), None);
    }

    #[test]
    fn merge_uses_the_source_inputs_key() {
        let src = aes128();
        let merged = merge(&[plain(), src.clone()]).unwrap();
        let bytes = merged.to_bytes().unwrap();
        assert_eq!(
            encrypt_profile(&bytes),
            encrypt_profile(src.original_bytes())
        );
        assert_eq!(reopen(bytes).page_count(), 4);
    }

    #[test]
    fn derived_and_merged_output_preserve_algorithm_key_length_and_permissions() {
        for (name, src) in [
            ("aes256", aes256()),
            ("aes128", aes128()),
            ("rc4-128", rc4_128()),
        ] {
            let want = encrypt_profile(src.original_bytes());
            // The source really is restricted (no "modify", bit 4) and /P is
            // the usual negative 32-bit value.
            let p = p_value(src.original_bytes());
            assert!(p < 0 && p & 8 == 0, "{name}: /P {p}");
            let all = parse_page_ranges("1-2", 2).unwrap();
            let two = parse_page_ranges("1,2", 2).unwrap();
            let outputs = [
                (
                    "optimize",
                    optimize(&src, &OptimizeOptions::default()).unwrap(),
                ),
                ("extract 1", crate::pages::extract(&src, &all).unwrap()),
                ("extract 2", crate::pages::extract(&src, &two).unwrap()),
                ("merge", merge(&[plain(), src.clone()]).unwrap()),
            ];
            for (op, out) in outputs {
                let bytes = out.to_bytes().unwrap();
                assert_eq!(encrypt_profile(&bytes), want, "{name} {op}");
                assert!(reopen(bytes).is_encrypted(), "{name} {op}");
            }
        }
    }
}
