//! ByteRange hardening and post-signature modification classification.
//! Every case starts from a real SoftHSM-signed fixture and manipulates it.

use lopdf::{Dictionary, Document as LoDocument, Object};
use openpdf_core::{ByteRangeError, LaterModification, SignatureInfo};

use super::*;
use crate::pades::{
    find_last, get_dict, last_startxref, next_object_id, trailer_root, trailer_size,
    write_lopdf_object, write_xref_and_trailer,
};

fn sign_bytes(pdf: &[u8]) -> Vec<u8> {
    let signer = signer();
    let certs = signer.list_certificates().unwrap();
    let cert = certs.iter().find(|c| can_use(c)).unwrap_or(&certs[0]);
    let request = SignRequest {
        cert_id: cert.id.clone(),
        reason: Some("byterange test".into()),
        location: None,
        contact: None,
        appearance: None,
    };
    signer.sign(pdf, &request).expect("sign")
}

fn signed_fixture() -> Vec<u8> {
    sign_bytes(&fs::read(corpus("hello.pdf")).unwrap())
}

/// Current `[a b c d]` of the (only) signature.
fn byte_range(pdf: &[u8]) -> [i64; 4] {
    let (start, end) = byte_range_span(pdf);
    let text = std::str::from_utf8(&pdf[start + 1..end]).unwrap();
    let v: Vec<i64> = text
        .split_whitespace()
        .map(|t| t.parse().unwrap())
        .collect();
    [v[0], v[1], v[2], v[3]]
}

fn byte_range_span(pdf: &[u8]) -> (usize, usize) {
    let mark = find_last(pdf, b"/ByteRange [").unwrap();
    let open = mark + b"/ByteRange ".len();
    let close = open + pdf[open..].iter().position(|b| *b == b']').unwrap();
    (open, close)
}

/// Rewrite the ByteRange array in place, same length, so no offset moves.
fn with_byte_range(pdf: &[u8], values: &[i64]) -> Vec<u8> {
    let (open, close) = byte_range_span(pdf);
    let inner = values
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let width = close - open - 1;
    assert!(inner.len() <= width, "ByteRange replacement too long");
    let mut out = pdf.to_vec();
    out[open + 1..close].copy_from_slice(format!("{inner:<width$}").as_bytes());
    out
}

fn byte_range_err(pdf: &[u8]) -> ByteRangeError {
    match verify_pdf(pdf) {
        Err(Error::ByteRange(kind)) => kind,
        other => panic!("expected a ByteRange error, got {other:?}"),
    }
}

/// Append one incremental update: `objects` as dictionaries, `raw` as
/// pre-serialized bodies (for streams).
fn append_update(pdf: &[u8], objects: &[(u32, Object)], raw: &[(u32, String)]) -> Vec<u8> {
    let doc = LoDocument::load_mem(pdf).unwrap();
    let root = trailer_root(&doc).unwrap();
    let prev = last_startxref(pdf).unwrap();
    let mut out = pdf.to_vec();
    let mut xref = Vec::new();
    let mut max_id = trailer_size(&doc);
    for (id, body) in raw {
        xref.push((*id, out.len()));
        out.extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
        max_id = max_id.max(id + 1);
    }
    for (id, obj) in objects {
        write_lopdf_object(&mut out, &mut xref, *id, obj).unwrap();
        max_id = max_id.max(id + 1);
    }
    write_xref_and_trailer(&mut out, &xref, max_id, root, prev);
    out
}

/// Add `annot` to page 1 in a new incremental update. With `opaque_ap`, the
/// annotation gets a Form XObject that paints a filled white box.
fn append_annotation(pdf: &[u8], mut annot: Dictionary, opaque_ap: bool) -> Vec<u8> {
    let doc = LoDocument::load_mem(pdf).unwrap();
    let page_id = *doc.get_pages().get(&1).unwrap();
    let annot_id = next_object_id(&doc);
    let ap_id = annot_id + 1;
    let mut raw = Vec::new();
    if opaque_ap {
        let content = "1 1 1 rg 0 0 400 400 re f";
        raw.push((
            ap_id,
            format!(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 400 400] /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
        ));
        let mut ap = Dictionary::new();
        ap.set("N", Object::Reference((ap_id, 0)));
        annot.set("AP", Object::Dictionary(ap));
    }
    let mut page = get_dict(&doc, page_id).unwrap().clone();
    let mut annots = match page.get(b"Annots") {
        Ok(Object::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    annots.push(Object::Reference((annot_id, 0)));
    page.set("Annots", Object::Array(annots));
    append_update(
        pdf,
        &[
            (annot_id, Object::Dictionary(annot)),
            (page_id.0, Object::Dictionary(page)),
        ],
        &raw,
    )
}

fn annot(subtype: &str) -> Dictionary {
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"Annot".to_vec()));
    d.set("Subtype", Object::Name(subtype.as_bytes().to_vec()));
    d.set("Rect", vec![0.into(), 0.into(), 400.into(), 400.into()]);
    d.set("Contents", Object::string_literal("later"));
    d
}

fn only_info(pdf: &[u8]) -> SignatureInfo {
    let infos = verify_pdf(pdf).expect("verify");
    assert_eq!(infos.len(), 1);
    infos.into_iter().next().unwrap()
}

#[test]
fn well_formed_signature_covers_whole_file() {
    let Some(()) = with_softhsm(|| {
        let info = only_info(&signed_fixture());
        assert_eq!(info.valid, Some(true));
        assert_eq!(info.modified_after, None);
    }) else {
        return;
    };
}

#[test]
fn byte_range_odd_count_rejected() {
    let Some(()) = with_softhsm(|| {
        let pdf = signed_fixture();
        let [a, b, c, _] = byte_range(&pdf);
        assert_eq!(
            byte_range_err(&with_byte_range(&pdf, &[a, b, c])),
            ByteRangeError::NotFourIntegers { count: 3 }
        );
    }) else {
        return;
    };
}

#[test]
fn byte_range_six_values_rejected() {
    let Some(()) = with_softhsm(|| {
        let pdf = signed_fixture();
        let [a, b, c, d] = byte_range(&pdf);
        assert_eq!(
            byte_range_err(&with_byte_range(&pdf, &[a, b, c, d, 0, 0])),
            ByteRangeError::NotFourIntegers { count: 6 }
        );
    }) else {
        return;
    };
}

#[test]
fn byte_range_negative_rejected() {
    let Some(()) = with_softhsm(|| {
        let pdf = signed_fixture();
        let [a, b, c, d] = byte_range(&pdf);
        assert_eq!(
            byte_range_err(&with_byte_range(&pdf, &[a, -b, c, d])),
            ByteRangeError::Negative
        );
    }) else {
        return;
    };
}

#[test]
fn byte_range_start_not_zero_rejected() {
    let Some(()) = with_softhsm(|| {
        let pdf = signed_fixture();
        let [_, b, c, d] = byte_range(&pdf);
        assert_eq!(
            byte_range_err(&with_byte_range(&pdf, &[1, b - 1, c, d])),
            ByteRangeError::FirstOffsetNotZero { offset: 1 }
        );
    }) else {
        return;
    };
}

#[test]
fn byte_range_gap_shifted_rejected() {
    let Some(()) = with_softhsm(|| {
        let pdf = signed_fixture();
        let [a, b, c, d] = byte_range(&pdf);
        assert_eq!(
            byte_range_err(&with_byte_range(&pdf, &[a, b + 1, c + 1, d - 1])),
            ByteRangeError::GapNotContents
        );
        assert_eq!(
            byte_range_err(&with_byte_range(&pdf, &[a, b - 1, c - 1, d + 1])),
            ByteRangeError::GapNotContents
        );
    }) else {
        return;
    };
}

#[test]
fn byte_range_not_to_revision_end_rejected() {
    let Some(()) = with_softhsm(|| {
        let pdf = signed_fixture();
        let [a, b, c, d] = byte_range(&pdf);
        assert_eq!(
            byte_range_err(&with_byte_range(&pdf, &[a, b, c, d - 7])),
            ByteRangeError::NotRevisionEnd
        );
    }) else {
        return;
    };
}

#[test]
fn byte_range_overlapping_rejected() {
    let Some(()) = with_softhsm(|| {
        let pdf = signed_fixture();
        let [a, b, c, d] = byte_range(&pdf);
        let end = c + d;
        assert_eq!(
            byte_range_err(&with_byte_range(&pdf, &[a, b, b - 10, end - (b - 10)])),
            ByteRangeError::Overlapping
        );
    }) else {
        return;
    };
}

#[test]
fn byte_range_out_of_bounds_rejected() {
    let Some(()) = with_softhsm(|| {
        let pdf = signed_fixture();
        let [a, b, c, d] = byte_range(&pdf);
        assert_eq!(
            byte_range_err(&with_byte_range(&pdf, &[a, b, c, d + 1000])),
            ByteRangeError::OutOfBounds
        );
    }) else {
        return;
    };
}

#[test]
fn byte_range_error_is_translated() {
    let err = Error::ByteRange(ByteRangeError::Overlapping);
    with_locale("en", || {
        assert_eq!(err.to_string(), "signature ByteRange ranges overlap");
    });
    with_locale("es", || {
        assert_ne!(err.to_string(), "signature ByteRange ranges overlap");
    });
}

#[test]
fn later_plain_comment_is_annotations() {
    let Some(()) = with_softhsm(|| {
        let pdf = append_annotation(&signed_fixture(), annot("Text"), false);
        let info = only_info(&pdf);
        assert_eq!(info.valid, Some(true), "signed revision is still intact");
        assert_eq!(info.modified_after, Some(LaterModification::Annotations));
    }) else {
        return;
    };
}

#[test]
fn later_opaque_stamp_is_other() {
    let Some(()) = with_softhsm(|| {
        let pdf = append_annotation(&signed_fixture(), annot("Stamp"), true);
        let info = only_info(&pdf);
        assert_eq!(info.valid, Some(true));
        assert_eq!(info.modified_after, Some(LaterModification::Other));
    }) else {
        return;
    };
}

#[test]
fn later_opaque_freetext_is_other() {
    let Some(()) = with_softhsm(|| {
        let mut ft = annot("FreeText");
        ft.set("DA", Object::string_literal("/Helv 12 Tf 0 g"));
        let pdf = append_annotation(&signed_fixture(), ft, true);
        assert_eq!(
            only_info(&pdf).modified_after,
            Some(LaterModification::Other)
        );
    }) else {
        return;
    };
}

#[test]
fn later_comment_with_visible_appearance_is_other() {
    let Some(()) = with_softhsm(|| {
        let pdf = append_annotation(&signed_fixture(), annot("Text"), true);
        assert_eq!(
            only_info(&pdf).modified_after,
            Some(LaterModification::Other)
        );
    }) else {
        return;
    };
}

#[test]
fn second_signature_is_later_signature() {
    let Some(()) = with_softhsm(|| {
        let first = signed_fixture();
        let second = sign_bytes(&first);
        let infos = verify_pdf(&second).expect("verify");
        assert_eq!(infos.len(), 2);
        assert!(infos.iter().all(|i| i.valid == Some(true)));
        let mut mods: Vec<_> = infos.iter().map(|i| i.modified_after).collect();
        mods.sort_by_key(|m| m.is_some());
        assert_eq!(mods, vec![None, Some(LaterModification::LaterSignature)]);
    }) else {
        return;
    };
}
