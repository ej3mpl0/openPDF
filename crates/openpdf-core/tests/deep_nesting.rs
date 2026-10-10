//! RUSTSEC-2026-0187: deeply nested objects must be rejected, not overflow the stack.

use openpdf_core::Document;

/// A one-page PDF whose page tree node carries `/Deep <nested>`, so a parser
/// that survives the nesting still cannot produce a usable document.
fn pdf_with_deep_page_tree(nested: &str) -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [3 0 R] /Count 1 /Deep {nested} >>"),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>".to_string(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .as_bytes(),
    );
    out
}

const LEVELS: usize = 100_000;

#[test]
fn deeply_nested_arrays_are_an_error_not_a_crash() {
    let body = format!("{}{}", "[".repeat(LEVELS), "]".repeat(LEVELS));
    assert!(Document::from_bytes(pdf_with_deep_page_tree(&body), None).is_err());
}

#[test]
fn deeply_nested_dicts_are_an_error_not_a_crash() {
    let body = format!("{}{}", "<< /A ".repeat(LEVELS), ">>".repeat(LEVELS));
    assert!(Document::from_bytes(pdf_with_deep_page_tree(&body), None).is_err());
}

#[test]
fn shallow_nesting_still_loads() {
    let doc = Document::from_bytes(pdf_with_deep_page_tree("[[<< /A [1] >>]]"), None).unwrap();
    assert_eq!(doc.page_count(), 1);
}
