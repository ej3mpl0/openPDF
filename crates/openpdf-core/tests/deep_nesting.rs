//! RUSTSEC-2026-0187: deeply nested objects must not overflow the stack.
//!
//! The PDF is a real, valid one-page document with text. The deep array or
//! dictionary lives in an extra, unreferenced object (5 0 obj), so the file
//! must still load (`Ok`, one page, renders, text extracts) and lopdf 0.44
//! (`MAX_NESTING_DEPTH` = 100) must drop only that object. A test
//! that passes through `EmptyDocument` or any other `Err` proves nothing.
//!
//! Never build and drop a >100-deep `lopdf::Object` tree in memory here:
//! lopdf's `Drop` is recursive and would overflow the stack itself.

use openpdf_core::{Document, Engine};

const DEEP_ID: (u32, u16) = (5, 0);

fn valid_pdf_with_deep_object(deep: &str) -> Vec<u8> {
    let content = "BT /F1 24 Tf 72 700 Td (HELLO DEEP) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /Font << /F1 6 0 R >> >> /Contents 4 0 R >>"
            .to_string(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
        deep.to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
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

fn deep_arrays(depth: usize) -> String {
    format!("{}{}", "[".repeat(depth), "]".repeat(depth))
}

fn deep_dicts(depth: usize) -> String {
    format!("{}{}", "<< /A ".repeat(depth), ">>".repeat(depth))
}

/// Nesting depth of an object, iteratively (no recursion on untrusted data).
fn depth_of(obj: &lopdf::Object) -> usize {
    let mut max = 0;
    let mut stack = vec![(obj, 1usize)];
    while let Some((o, d)) = stack.pop() {
        max = max.max(d);
        match o {
            lopdf::Object::Array(a) => stack.extend(a.iter().map(|c| (c, d + 1))),
            lopdf::Object::Dictionary(dict) => stack.extend(dict.iter().map(|(_, c)| (c, d + 1))),
            _ => {}
        }
    }
    max
}

/// Load on a normal 2 MiB thread (the harness default), so the result does
/// not depend on the main thread's larger stack. A stack overflow aborts the
/// whole process, so reaching the asserts at all is half the test.
fn check(label: &str, deep: String) {
    let pdf = valid_pdf_with_deep_object(&deep);
    let bytes = pdf.clone();
    let label_t = label.to_string();
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let doc = Document::from_bytes(bytes.clone(), None)
                .unwrap_or_else(|e| panic!("{label_t}: valid PDF rejected: {e}"));
            assert_eq!(doc.page_count(), 1, "{label_t}");

            // The deep object itself: gone, or truncated to lopdf's limit.
            let raw = lopdf::Document::load_mem(&bytes).unwrap();
            // lopdf 0.44 drops an object past MAX_NESTING_DEPTH (it does not
            // truncate it), so the id must be absent while the page survives.
            assert!(
                raw.get_object(DEEP_ID).is_err(),
                "{label_t}: deep object 5 0 R was kept"
            );
        })
        .unwrap()
        .join()
        .unwrap_or_else(|_| panic!("{label}: load panicked"));

    // The page is real: it renders and its text extracts.
    let engine = match Engine::new() {
        Ok(e) => e,
        Err(err) => {
            if std::env::var("CI").is_ok() {
                panic!("PDFium required in CI: {err}");
            }
            eprintln!("skipping PDFium part: {err}");
            return;
        }
    };
    let doc = engine.open_bytes(pdf, None).unwrap();
    let text = engine.page_text(doc.id(), 0).unwrap();
    assert!(text.contains("HELLO DEEP"), "{label}: text {text:?}");
    let raster = engine.render_page(doc.id(), 0, 1.0).unwrap();
    assert!(
        raster.rgba.iter().any(|b| *b < 250),
        "{label}: blank render"
    );
    engine.close(doc.id()).unwrap();
}

#[test]
fn nested_arrays_10k_dropped_page_still_loads() {
    check("arrays 10k", deep_arrays(10_000));
}

#[test]
fn nested_arrays_100k_dropped_page_still_loads() {
    check("arrays 100k", deep_arrays(100_000));
}

#[test]
fn nested_dicts_10k_dropped_page_still_loads() {
    check("dicts 10k", deep_dicts(10_000));
}

#[test]
fn nested_dicts_100k_dropped_page_still_loads() {
    check("dicts 100k", deep_dicts(100_000));
}

#[test]
fn shallow_nesting_is_kept() {
    let pdf = valid_pdf_with_deep_object("[[<< /A [1] >>]]");
    let doc = Document::from_bytes(pdf.clone(), None).unwrap();
    assert_eq!(doc.page_count(), 1);
    let raw = lopdf::Document::load_mem(&pdf).unwrap();
    assert_eq!(depth_of(raw.get_object(DEEP_ID).unwrap()), 5);
}
