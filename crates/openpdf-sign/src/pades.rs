//! Incremental PAdES-B-B PDF update and signature-dictionary verification.

use std::io::Write;

use lopdf::{Dictionary, Document, Object, ObjectId};
use openpdf_core::{
    tr_args, Error, PadesLevel, Result, SignRequest, SignatureInfo, VisibleAppearance,
};
use sha2::{Digest, Sha256};

use crate::cades;

const CONTENTS_LEN: usize = 8192;
const BR_WIDTH: usize = 10;
const CONTENTS_MARK: &[u8] = b"/Contents <";
const BYTE_RANGE_MARK: &[u8] = b"/ByteRange [";

pub(crate) fn sign_incremental(
    pdf: &[u8],
    request: &SignRequest,
    signer_name: &str,
    mut make_cms: impl FnMut(&[u8]) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    ensure_supported_level(request)?;
    if !pdf.starts_with(b"%PDF-") {
        return Err(Error::Sign("not a PDF".into()));
    }
    let doc = Document::load_mem(pdf).map_err(|e| Error::Sign(e.to_string()))?;
    let prev = last_startxref(pdf)?;
    let root = trailer_root(&doc)?;
    let page_id = pick_page(&doc, request.appearance.as_ref())?;
    let next_id = next_object_id(&doc);
    let sig_count = count_signatures(&doc);

    let sig_id = next_id;
    let widget_id = next_id + 1;
    let acro_id = next_id + 2;
    let mut extra_id = next_id + 3;

    let mut out = pdf.to_vec();
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }

    let mut xref: Vec<(u32, usize)> = Vec::new();

    let date = pdf_date_now();
    let sig_body = signature_dict_placeholder(signer_name, request, &date, CONTENTS_LEN, BR_WIDTH);
    write_raw_object(&mut out, &mut xref, sig_id, &sig_body);

    let mut widget = Dictionary::new();
    widget.set("Type", Object::Name(b"Annot".to_vec()));
    widget.set("Subtype", Object::Name(b"Widget".to_vec()));
    widget.set("FT", Object::Name(b"Sig".to_vec()));
    widget.set("V", Object::Reference((sig_id, 0)));
    widget.set(
        "T",
        Object::string_literal(format!("Signature{}", sig_count + 1)),
    );
    widget.set("F", Object::Integer(132));
    widget.set("P", Object::Reference(page_id));
    if let Some(app) = &request.appearance {
        let [llx, lly, urx, ury] = app.rect;
        widget.set(
            "Rect",
            vec![
                Object::Real(llx),
                Object::Real(lly),
                Object::Real(urx),
                Object::Real(ury),
            ],
        );
        let ap_id = extra_id;
        extra_id += 1;
        let font_id = extra_id;
        extra_id += 1;
        let w = (urx - llx).abs();
        let h = (ury - lly).abs();
        write_appearance(&mut out, &mut xref, ap_id, font_id, w, h, signer_name)?;
        let mut ap = Dictionary::new();
        ap.set("N", Object::Reference((ap_id, 0)));
        widget.set("AP", Object::Dictionary(ap));
    } else {
        widget.set("Rect", vec![0.into(), 0.into(), 0.into(), 0.into()]);
    }
    write_lopdf_object(&mut out, &mut xref, widget_id, &Object::Dictionary(widget))?;

    let acro = build_acroform(&doc, (widget_id, 0))?;
    write_lopdf_object(&mut out, &mut xref, acro_id, &Object::Dictionary(acro))?;

    let mut page = get_dict(&doc, page_id)?.clone();
    let mut annots = existing_array(&doc, &page, b"Annots");
    annots.push(Object::Reference((widget_id, 0)));
    page.set("Annots", Object::Array(annots));
    write_lopdf_object(&mut out, &mut xref, page_id.0, &Object::Dictionary(page))?;

    let mut catalog = doc
        .catalog()
        .map_err(|e| Error::Sign(e.to_string()))?
        .clone();
    catalog.set("AcroForm", Object::Reference((acro_id, 0)));
    write_lopdf_object(&mut out, &mut xref, root.0, &Object::Dictionary(catalog))?;

    let old_size = trailer_size(&doc);
    let new_size = extra_id.max(old_size);
    write_xref_and_trailer(&mut out, &xref, new_size, root, prev);

    let (lt, gt) = locate_contents(&out, CONTENTS_LEN)?;
    let rest = out.len() - (gt + 1);
    patch_byte_range(&mut out, lt, gt + 1, rest)?;

    let message = byterange_bytes(&out, &[0, lt as i64, (gt + 1) as i64, rest as i64])?;
    let cms = make_cms(&message)?;
    if cms.len() > CONTENTS_LEN {
        return Err(Error::Sign(
            "CMS signature is too large for the reserved Contents space".into(),
        ));
    }
    let hex = hex_encode_upper(&cms);
    let mut padded = hex.into_bytes();
    padded.resize(CONTENTS_LEN * 2, b'0');
    out[lt + 1..lt + 1 + CONTENTS_LEN * 2].copy_from_slice(&padded);
    Ok(out)
}

/// Only PAdES-B-B is produced. Any other level is a typed error, never a
/// silent downgrade to B-B.
pub(crate) fn ensure_supported_level(request: &SignRequest) -> Result<()> {
    match request.level {
        PadesLevel::BB => Ok(()),
        level => Err(Error::SignLevelUnsupported { level }),
    }
}

pub(crate) fn verify_signatures(pdf: &[u8]) -> Result<Vec<SignatureInfo>> {
    let doc = Document::load_mem(pdf).map_err(|e| Error::Sign(e.to_string()))?;
    let mut out = Vec::new();
    for object in doc.objects.values() {
        let Ok(dict) = object.as_dict() else {
            continue;
        };
        if !is_signature_value(dict) {
            continue;
        }
        out.push(verify_one(pdf, dict));
    }
    Ok(out)
}

fn verify_one(pdf: &[u8], dict: &Dictionary) -> SignatureInfo {
    let reason = dict_text(dict, b"Reason");
    let location = dict_text(dict, b"Location");
    let name = dict_text(dict, b"Name");

    let Some(ranges) = byte_range(dict) else {
        return SignatureInfo {
            name,
            reason,
            location,
            valid: Some(false),
        };
    };
    let Some(contents) = contents_bytes(dict) else {
        return SignatureInfo {
            name,
            reason,
            location,
            valid: Some(false),
        };
    };
    if contents.iter().all(|b| *b == 0) {
        return SignatureInfo {
            name,
            reason,
            location,
            valid: Some(false),
        };
    }
    let Ok(message) = byterange_bytes(pdf, &ranges) else {
        return SignatureInfo {
            name,
            reason,
            location,
            valid: Some(false),
        };
    };
    // Integrity of the claimed ranges: SHA-256 is also pinned inside CMS.
    let _ = Sha256::digest(&message);
    let check = cades::verify_detached(&contents, &message);
    SignatureInfo {
        name: name.or(check.signer),
        reason,
        location,
        valid: Some(check.valid),
    }
}

fn is_signature_value(dict: &Dictionary) -> bool {
    let is_sig = match dict.get(b"Type") {
        Ok(Object::Name(n)) if n.as_slice() == b"Sig" => true,
        _ => false,
    };
    if !is_sig {
        return false;
    }
    dict.get(b"Contents").is_ok() && dict.get(b"ByteRange").is_ok()
}

fn byte_range(dict: &Dictionary) -> Option<Vec<i64>> {
    let arr = dict.get(b"ByteRange").ok()?.as_array().ok()?;
    if arr.len() < 4 || arr.len() % 2 != 0 {
        return None;
    }
    arr.iter()
        .map(|o| match o {
            Object::Integer(i) => Some(*i),
            Object::Real(r) => Some(*r as i64),
            _ => None,
        })
        .collect()
}

fn contents_bytes(dict: &Dictionary) -> Option<Vec<u8>> {
    match dict.get(b"Contents").ok()? {
        Object::String(bytes, _) => strip_contents_padding(bytes).map(<[u8]>::to_vec),
        _ => None,
    }
}

/// `/Contents` is the CMS DER followed by zero padding. Cut it at the DER
/// length, not at the last non-zero byte: a CMS that happens to end in 0x00
/// (about 1 in 256 RSA signatures) would otherwise be truncated and reported
/// invalid. If the DER length can't be read or runs past the data, the CMS is
/// malformed: return `None` (signature invalid), never guess with the old
/// trailing-zero trim.
fn strip_contents_padding(bytes: &[u8]) -> Option<&[u8]> {
    let len = der_tlv_len(bytes)?;
    bytes.get(..len)
}

/// Total length (header + content) of the DER SEQUENCE at the start of `b`.
fn der_tlv_len(b: &[u8]) -> Option<usize> {
    if b.len() < 2 || b[0] != 0x30 {
        return None;
    }
    let first = b[1] as usize;
    if first < 0x80 {
        return Some(2 + first);
    }
    let n = first & 0x7f;
    if n == 0 || n > 4 || b.len() < 2 + n {
        return None;
    }
    let len = b[2..2 + n]
        .iter()
        .fold(0usize, |acc, x| (acc << 8) | *x as usize);
    Some(2 + n + len)
}

pub(crate) fn byterange_bytes(pdf: &[u8], ranges: &[i64]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for chunk in ranges.chunks_exact(2) {
        let off =
            usize::try_from(chunk[0]).map_err(|_| Error::Sign("ByteRange is invalid".into()))?;
        let len =
            usize::try_from(chunk[1]).map_err(|_| Error::Sign("ByteRange is invalid".into()))?;
        let end = off
            .checked_add(len)
            .ok_or_else(|| Error::Sign("ByteRange is invalid".into()))?;
        if end > pdf.len() {
            return Err(Error::Sign("ByteRange is invalid".into()));
        }
        out.extend_from_slice(&pdf[off..end]);
    }
    Ok(out)
}

fn dict_text(dict: &Dictionary, key: &[u8]) -> Option<String> {
    match dict.get(key).ok()? {
        Object::String(bytes, _) => decode_pdf_text(bytes),
        Object::Name(n) => String::from_utf8(n.clone()).ok(),
        _ => None,
    }
}

fn decode_pdf_text(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .chunks(2)
            .filter_map(|c| (c.len() == 2).then(|| u16::from_be_bytes([c[0], c[1]])))
            .collect();
        String::from_utf16(&units).ok()
    } else if let Ok(s) = std::str::from_utf8(bytes) {
        Some(s.to_string())
    } else {
        Some(bytes.iter().map(|&b| b as char).collect())
    }
}

fn signature_dict_placeholder(
    name: &str,
    request: &SignRequest,
    date: &str,
    contents_len: usize,
    br_w: usize,
) -> Vec<u8> {
    let zeros = "0".repeat(contents_len * 2);
    let br = format!("[0 {v:>w$} {v:>w$} {v:>w$}]", v = 0, w = br_w);
    let mut s = format!(
        "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /ETSI.CAdES.detached \
         /ByteRange {br} /Contents <{zeros}> /M {}\n/Name {}\n",
        pdf_text(date),
        pdf_text(name)
    );
    if let Some(r) = &request.reason {
        s.push_str(&format!("/Reason {}\n", pdf_text(r)));
    }
    if let Some(l) = &request.location {
        s.push_str(&format!("/Location {}\n", pdf_text(l)));
    }
    if let Some(c) = &request.contact {
        s.push_str(&format!("/ContactInfo {}\n", pdf_text(c)));
    }
    s.push_str(">>\n");
    s.into_bytes()
}

fn pdf_text(s: &str) -> String {
    if s.is_ascii() {
        let escaped = s
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");
        format!("({escaped})")
    } else {
        let mut raw = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            raw.extend_from_slice(&u.to_be_bytes());
        }
        format!("<{}>", hex_encode_upper(&raw))
    }
}

fn write_appearance(
    out: &mut Vec<u8>,
    xref: &mut Vec<(u32, usize)>,
    ap_id: u32,
    font_id: u32,
    w: f32,
    h: f32,
    name: &str,
) -> Result<()> {
    let mut font = Dictionary::new();
    font.set("Type", Object::Name(b"Font".to_vec()));
    font.set("Subtype", Object::Name(b"Type1".to_vec()));
    font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
    write_lopdf_object(out, xref, font_id, &Object::Dictionary(font))?;

    let label = tr_args("Signed by {name}", &[("name", name)]);
    let content = format!(
        "q 0.95 0.95 0.9 rg 0 0 {w:.2} {h:.2} re f 0 0 0 RG 0.5 w 0 0 {w:.2} {h:.2} re S \
         BT /Hev 8 Tf 4 6 Td {} Tj ET Q\n",
        pdf_text(&label)
    );
    let mut stream_dict = Dictionary::new();
    stream_dict.set("Type", Object::Name(b"XObject".to_vec()));
    stream_dict.set("Subtype", Object::Name(b"Form".to_vec()));
    stream_dict.set(
        "BBox",
        vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(w),
            Object::Real(h),
        ],
    );
    let mut resources = Dictionary::new();
    let mut fonts = Dictionary::new();
    fonts.set("Hev", Object::Reference((font_id, 0)));
    resources.set("Font", Object::Dictionary(fonts));
    stream_dict.set("Resources", Object::Dictionary(resources));
    stream_dict.set("Length", Object::Integer(content.len() as i64));

    let off = out.len();
    xref.push((ap_id, off));
    write!(out, "{ap_id} 0 obj\n").map_err(|e| Error::Sign(e.to_string()))?;
    encode_object(&Object::Dictionary(stream_dict), out)?;
    out.extend_from_slice(b"\nstream\n");
    out.extend_from_slice(content.as_bytes());
    out.extend_from_slice(b"endstream\nendobj\n");
    Ok(())
}

fn build_acroform(doc: &Document, widget: ObjectId) -> Result<Dictionary> {
    let mut fields = Vec::new();
    if let Ok(catalog) = doc.catalog() {
        if let Ok(obj) = catalog.get(b"AcroForm") {
            match obj {
                Object::Reference(id) => {
                    if let Ok(dict) = get_dict(doc, *id) {
                        fields = existing_array(doc, dict, b"Fields");
                    }
                }
                Object::Dictionary(dict) => {
                    fields = existing_array(doc, dict, b"Fields");
                }
                _ => {}
            }
        }
    }
    fields.push(Object::Reference(widget));
    let mut acro = Dictionary::new();
    acro.set("Fields", Object::Array(fields));
    acro.set("SigFlags", Object::Integer(3));
    Ok(acro)
}

fn existing_array(doc: &Document, dict: &Dictionary, key: &[u8]) -> Vec<Object> {
    match dict.get(key) {
        Ok(Object::Array(a)) => a.clone(),
        Ok(Object::Reference(id)) => match doc.get_object(*id) {
            Ok(Object::Array(a)) => a.clone(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn write_raw_object(out: &mut Vec<u8>, xref: &mut Vec<(u32, usize)>, id: u32, body: &[u8]) {
    let off = out.len();
    xref.push((id, off));
    let _ = write!(out, "{id} 0 obj\n");
    out.extend_from_slice(body);
    if !body.ends_with(b"\n") {
        out.push(b'\n');
    }
    out.extend_from_slice(b"endobj\n");
}

fn write_lopdf_object(
    out: &mut Vec<u8>,
    xref: &mut Vec<(u32, usize)>,
    id: u32,
    obj: &Object,
) -> Result<()> {
    let off = out.len();
    xref.push((id, off));
    write!(out, "{id} 0 obj\n").map_err(|e| Error::Sign(e.to_string()))?;
    encode_object(obj, out)?;
    out.extend_from_slice(b"\nendobj\n");
    Ok(())
}

fn encode_object(obj: &Object, out: &mut Vec<u8>) -> Result<()> {
    match obj {
        Object::Null => out.extend_from_slice(b"null"),
        Object::Boolean(true) => out.extend_from_slice(b"true"),
        Object::Boolean(false) => out.extend_from_slice(b"false"),
        Object::Integer(i) => write!(out, "{i}").map_err(|e| Error::Sign(e.to_string()))?,
        Object::Real(r) => write!(out, "{r}").map_err(|e| Error::Sign(e.to_string()))?,
        Object::Name(n) => {
            out.push(b'/');
            out.extend_from_slice(n);
        }
        Object::String(bytes, lopdf::StringFormat::Literal) => {
            out.push(b'(');
            for &b in bytes {
                match b {
                    b'(' | b')' | b'\\' => {
                        out.push(b'\\');
                        out.push(b);
                    }
                    _ => out.push(b),
                }
            }
            out.push(b')');
        }
        Object::String(bytes, lopdf::StringFormat::Hexadecimal) => {
            out.push(b'<');
            out.extend_from_slice(hex_encode_upper(bytes).as_bytes());
            out.push(b'>');
        }
        Object::Array(arr) => {
            out.push(b'[');
            for (i, item) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                encode_object(item, out)?;
            }
            out.push(b']');
        }
        Object::Dictionary(dict) => {
            out.extend_from_slice(b"<<");
            for (k, v) in dict.iter() {
                out.push(b'/');
                out.extend_from_slice(k);
                out.push(b' ');
                encode_object(v, out)?;
            }
            out.extend_from_slice(b">>");
        }
        Object::Reference((id, gen)) => {
            write!(out, "{id} {gen} R").map_err(|e| Error::Sign(e.to_string()))?;
        }
        Object::Stream(_) => {
            return Err(Error::Sign(
                "unexpected stream in incremental object".into(),
            ));
        }
    }
    Ok(())
}

fn write_xref_and_trailer(
    out: &mut Vec<u8>,
    xref: &[(u32, usize)],
    size: u32,
    root: ObjectId,
    prev: usize,
) {
    let mut entries = xref.to_vec();
    entries.sort_by_key(|(id, _)| *id);
    let xref_off = out.len();
    out.extend_from_slice(b"xref\n");
    let mut i = 0;
    while i < entries.len() {
        let start = entries[i].0;
        let mut j = i + 1;
        while j < entries.len() && entries[j].0 == entries[j - 1].0 + 1 {
            j += 1;
        }
        let _ = writeln!(out, "{start} {}", j - i);
        for k in i..j {
            let _ = write!(out, "{:010} {:05} n \n", entries[k].1, 0u16);
        }
        i = j;
    }
    let _ = write!(
        out,
        "trailer\n<< /Size {size} /Root {} {} R /Prev {prev} >>\nstartxref\n{xref_off}\n%%EOF\n",
        root.0, root.1
    );
}

fn locate_contents(buf: &[u8], contents_len: usize) -> Result<(usize, usize)> {
    let expected_hex = contents_len * 2;
    let mut search = 0;
    while let Some(rel) = find_subslice(&buf[search..], CONTENTS_MARK) {
        let lt = search + rel + CONTENTS_MARK.len() - 1; // '<'
        let hex_start = lt + 1;
        let gt = hex_start + expected_hex;
        if gt < buf.len() && buf[gt] == b'>' {
            if buf[hex_start..gt].iter().all(|b| *b == b'0') {
                return Ok((lt, gt));
            }
        }
        search += rel + 1;
    }
    Err(Error::Sign("Contents placeholder not found".into()))
}

fn patch_byte_range(
    buf: &mut [u8],
    contents_lt: usize,
    r2_start: usize,
    r2_len: usize,
) -> Result<()> {
    let search = &buf[..contents_lt];
    let rel = find_last(search, BYTE_RANGE_MARK)
        .ok_or_else(|| Error::Sign("ByteRange not found".into()))?;
    let start = rel + BYTE_RANGE_MARK.len() - 1; // '['
    let actual = format!(
        "[0 {:>w$} {:>w$} {:>w$}]",
        contents_lt,
        r2_start,
        r2_len,
        w = BR_WIDTH
    );
    if start + actual.len() > buf.len() {
        return Err(Error::Sign("ByteRange is out of range".into()));
    }
    buf[start..start + actual.len()].copy_from_slice(actual.as_bytes());
    Ok(())
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn last_startxref(pdf: &[u8]) -> Result<usize> {
    let pos = find_last(pdf, b"startxref")
        .ok_or_else(|| Error::Sign("this file has no startxref".into()))?;
    let rest = &pdf[pos + 9..];
    let digits = rest
        .iter()
        .skip_while(|b| b.is_ascii_whitespace())
        .take_while(|b| b.is_ascii_digit())
        .copied()
        .collect::<Vec<_>>();
    let s = std::str::from_utf8(&digits).map_err(|e| Error::Sign(e.to_string()))?;
    s.parse()
        .map_err(|e: std::num::ParseIntError| Error::Sign(e.to_string()))
}

fn find_last(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

fn trailer_root(doc: &Document) -> Result<ObjectId> {
    doc.trailer
        .get(b"Root")
        .map_err(|e| Error::Sign(e.to_string()))?
        .as_reference()
        .map_err(|e| Error::Sign(e.to_string()))
}

fn trailer_size(doc: &Document) -> u32 {
    match doc.trailer.get(b"Size") {
        Ok(Object::Integer(i)) if *i > 0 => *i as u32,
        _ => doc.max_id + 1,
    }
}

fn next_object_id(doc: &Document) -> u32 {
    let from_objects = doc.objects.keys().map(|(id, _)| *id).max().unwrap_or(0);
    from_objects.max(doc.max_id) + 1
}

fn pick_page(doc: &Document, appearance: Option<&VisibleAppearance>) -> Result<ObjectId> {
    let pages = doc.get_pages();
    let index = appearance.map(|a| a.page).unwrap_or(0);
    pages.get(&(index + 1)).copied().ok_or_else(|| {
        Error::Sign(tr_args(
            "page {page} not found",
            &[("page", &index.to_string())],
        ))
    })
}

fn get_dict(doc: &Document, id: ObjectId) -> Result<&Dictionary> {
    doc.get_object(id)
        .map_err(|e| Error::Sign(e.to_string()))?
        .as_dict()
        .map_err(|e| Error::Sign(e.to_string()))
}

fn count_signatures(doc: &Document) -> usize {
    doc.objects
        .values()
        .filter(|o| o.as_dict().is_ok_and(is_signature_value))
        .count()
}

fn pdf_date_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d, h, min, s) = civil_utc(secs);
    format!("D:{y:04}{m:02}{d:02}{h:02}{min:02}{s:02}+00'00'")
}

pub(crate) fn civil_utc(unix: u64) -> (i32, u32, u32, u32, u32, u32) {
    let z = unix / 86400;
    let tod = unix % 86400;
    let h = (tod / 3600) as u32;
    let min = ((tod % 3600) / 60) as u32;
    let s = (tod % 60) as u32;
    let z = z as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y } as i32;
    (y, m, d, h, min, s)
}

pub(crate) fn hex_encode_upper(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

pub(crate) fn hex_encode_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

#[cfg(test)]
pub(crate) fn hex_decode(s: &str) -> Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return Err(Error::Sign("odd hex length".into()));
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for i in (0..bytes.len()).step_by(2) {
        let hi = hex_val(bytes[i])?;
        let lo = hex_val(bytes[i + 1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

#[cfg(test)]
fn hex_val(b: u8) -> Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(Error::Sign("invalid hex".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let v = vec![0xde, 0xad, 0xbe, 0xef];
        assert_eq!(hex_decode(&hex_encode_lower(&v)).unwrap(), v);
        assert_eq!(hex_decode(&hex_encode_upper(&v)).unwrap(), v);
    }

    #[test]
    fn verify_unsigned_is_empty() {
        let pdf = std::fs::read(corpus("hello.pdf")).unwrap();
        assert!(verify_signatures(&pdf).unwrap().is_empty());
    }

    #[test]
    fn contents_padding_keeps_trailing_zero_of_der() {
        // SEQUENCE { OCTET STRING 01 00 } followed by padding.
        let der = [0x30, 0x04, 0x04, 0x02, 0x01, 0x00];
        let mut padded = der.to_vec();
        padded.extend_from_slice(&[0; 16]);
        assert_eq!(strip_contents_padding(&padded), Some(&der[..]));
        let mut long = vec![0x30, 0x82, 0x01, 0x00];
        long.extend(std::iter::repeat_n(0xAB, 255));
        long.push(0x00);
        let full = long.clone();
        long.extend_from_slice(&[0; 8]);
        assert_eq!(strip_contents_padding(&long), Some(&full[..]));
    }

    #[test]
    fn contents_with_unreadable_der_length_is_rejected() {
        // Not a SEQUENCE.
        assert_eq!(strip_contents_padding(&[0x04, 0x01, 0xAB, 0, 0]), None);
        // Length runs past the data.
        assert_eq!(strip_contents_padding(&[0x30, 0x10, 0x01, 0x02]), None);
        assert_eq!(
            strip_contents_padding(&[0x30, 0x84, 0x7F, 0xFF, 0xFF, 0xFF, 1, 0]),
            None
        );
        // Indefinite / too many length octets / header cut short.
        assert_eq!(strip_contents_padding(&[0x30, 0x80, 1, 0, 0]), None);
        assert_eq!(strip_contents_padding(&[0x30, 0x85, 1, 1, 1, 1, 1]), None);
        assert_eq!(strip_contents_padding(&[0x30, 0x82, 0x01]), None);
        assert_eq!(strip_contents_padding(&[0x30]), None);
    }

    /// A /Contents whose DER length runs past the data is reported invalid
    /// (not valid, not `Err`), and an earlier good signature in the same PDF
    /// still verifies.
    #[test]
    fn truncated_der_contents_is_invalid_and_other_signature_verifies() {
        use rsa::pkcs1v15::SigningKey;
        use rsa::pkcs8::DecodePrivateKey;
        use rsa::signature::{SignatureEncoding, Signer as _};
        use rsa::RsaPrivateKey;

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
        let cert = std::fs::read(dir.join("trailing-zero-cert.der")).unwrap();
        let key = SigningKey::<Sha256>::new(
            RsaPrivateKey::from_pkcs8_der(
                &std::fs::read(dir.join("trailing-zero-key.p8.der")).unwrap(),
            )
            .unwrap(),
        );
        let request = SignRequest::new(String::new()).with_reason("good");
        let pdf = std::fs::read(corpus("hello.pdf")).unwrap();
        let once = sign_incremental(&pdf, &request, "Good", |m| {
            cades::sign_detached(m, &cert, |tbs| Ok(key.sign(tbs).to_vec()))
        })
        .expect("first sign");
        let twice = sign_incremental(&once, &request, "Truncated", |m| {
            let mut cms = cades::sign_detached(m, &cert, |tbs| Ok(key.sign(tbs).to_vec()))?;
            // Claim a DER length far beyond the reserved /Contents space.
            assert_eq!(cms[0], 0x30);
            let body = cms.split_off(2 + (cms[1] & 0x7f) as usize);
            cms.truncate(1);
            cms.extend_from_slice(&[0x84, 0x7F, 0xFF, 0xFF, 0xFF]);
            cms.extend_from_slice(&body);
            Ok(cms)
        })
        .expect("second sign");

        let infos = verify_signatures(&twice).expect("verify must not fail");
        assert_eq!(infos.len(), 2);
        let good = infos.iter().filter(|i| i.valid == Some(true)).count();
        let bad = infos.iter().filter(|i| i.valid == Some(false)).count();
        assert_eq!((good, bad), (1, 1), "{infos:?}");
    }

    /// Regression: a real signed PDF whose CMS DER ends in 0x00 must verify.
    /// Deterministic: a fixed software key, and an unsigned attribute whose
    /// value is the single byte 0x00 is placed last in the SignerInfo, so the
    /// CMS always ends in 0x00 whatever the signature value is.
    #[test]
    fn cms_ending_in_zero_byte_verifies() {
        use cms::content_info::ContentInfo;
        use cms::signed_data::SignedData;
        use der::asn1::{OctetString, SetOfVec};
        use der::{Any, Decode, Encode};
        use rsa::pkcs1v15::SigningKey;
        use rsa::pkcs8::DecodePrivateKey;
        use rsa::signature::{SignatureEncoding, Signer as _};
        use rsa::RsaPrivateKey;

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
        let cert = std::fs::read(dir.join("trailing-zero-cert.der")).unwrap();
        let key = RsaPrivateKey::from_pkcs8_der(
            &std::fs::read(dir.join("trailing-zero-key.p8.der")).unwrap(),
        )
        .unwrap();
        let key = SigningKey::<Sha256>::new(key);

        let pdf = std::fs::read(corpus("hello.pdf")).unwrap();
        let request = SignRequest::new(String::new()).with_reason("trailing zero");
        let mut produced = Vec::new();
        let signed = sign_incremental(&pdf, &request, "Trailing Zero", |message| {
            let cms = cades::sign_detached(message, &cert, |tbs| Ok(key.sign(tbs).to_vec()))?;
            let ci = ContentInfo::from_der(&cms).unwrap();
            let mut sd = ci.content.decode_as::<SignedData>().unwrap();
            let mut infos: Vec<_> = sd.signer_infos.0.into_vec();
            let zero = OctetString::new(vec![0x00]).unwrap().to_der().unwrap();
            infos[0].unsigned_attrs = Some(
                SetOfVec::try_from(vec![x509_cert::attr::Attribute {
                    // id-data: arbitrary, ignored by verification.
                    oid: const_oid::db::rfc5911::ID_DATA,
                    values: SetOfVec::try_from(vec![Any::from_der(&zero).unwrap()]).unwrap(),
                }])
                .unwrap(),
            );
            sd.signer_infos = cms::signed_data::SignerInfos(SetOfVec::try_from(infos).unwrap());
            let out = ContentInfo {
                content_type: ci.content_type,
                content: Any::from_der(&sd.to_der().unwrap()).unwrap(),
            }
            .to_der()
            .unwrap();
            produced = out.clone();
            Ok(out)
        })
        .expect("sign");
        assert_eq!(produced.last(), Some(&0x00), "CMS must end in 0x00");

        let infos = verify_signatures(&signed).unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].valid, Some(true));

        // The old last-non-zero trim really does lose that byte.
        let end = produced.iter().rposition(|b| *b != 0).unwrap() + 1;
        assert!(end < produced.len());
    }

    fn corpus(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus")
            .join(name)
    }
}
