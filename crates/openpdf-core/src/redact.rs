//! True redaction: drop content in the box, then fully rewrite so nothing remains.

use std::collections::BTreeSet;

use image::codecs::jpeg::JpegEncoder;
use image::ExtendedColorType;
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Object, ObjectId, Stream};

use crate::document::Document;
use crate::error::{Error, InvalidKind, Result, SignedOp};

/// Inclusive box in PDF points on a 0-based page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RedactBox {
    pub page: u32,
    pub llx: f32,
    pub lly: f32,
    pub urx: f32,
    pub ury: f32,
}

impl RedactBox {
    pub fn new(page: u32, llx: f32, lly: f32, urx: f32, ury: f32) -> Result<Self> {
        if llx >= urx || lly >= ury {
            return Err(Error::invalid(InvalidKind::RedactionInverted));
        }
        Ok(Self {
            page,
            llx,
            lly,
            urx,
            ury,
        })
    }

    fn intersects(self, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
        let (a0, a1) = minmax(x0, x1);
        let (b0, b1) = minmax(y0, y1);
        a0 < self.urx && a1 > self.llx && b0 < self.ury && b1 > self.lly
    }
}

/// Remove content under `boxes` and rewrite the file (no incremental leftovers).
pub fn redact(doc: &Document, boxes: &[RedactBox]) -> Result<Document> {
    if doc.is_signed() {
        return Err(Error::signed(SignedOp::Rewrite));
    }
    if boxes.is_empty() {
        return Err(Error::invalid(InvalidKind::NoRedactionBoxes));
    }
    for b in boxes {
        if b.page >= doc.page_count() {
            return Err(Error::PageOutOfRange {
                page: b.page,
                count: doc.page_count(),
            });
        }
    }

    let mut inner = doc.assembled_lopdf()?;
    let page_map = inner.get_pages();
    let mut secrets = BTreeSet::new();

    let pages: BTreeSet<u32> = boxes.iter().map(|b| b.page).collect();
    for page in pages {
        let page_id = page_map
            .get(&(page + 1))
            .copied()
            .ok_or(Error::PageOutOfRange {
                page,
                count: doc.page_count(),
            })?;
        let page_boxes: Vec<RedactBox> = boxes.iter().copied().filter(|b| b.page == page).collect();
        redact_page(&mut inner, page_id, &page_boxes, &mut secrets)?;
        drop_thumb(&mut inner, page_id);
        drop_annots_in_boxes(&mut inner, page_id, &page_boxes)?;
    }
    clear_form_values_in_boxes(&mut inner, boxes)?;
    scrub_metadata(&mut inner, &secrets);
    wipe_secrets_everywhere(&mut inner, &secrets);
    inner.prune_objects();
    inner.renumber_objects();
    Document::from_lopdf(inner, None)
}

fn redact_page(
    doc: &mut lopdf::Document,
    page_id: ObjectId,
    boxes: &[RedactBox],
    secrets: &mut BTreeSet<String>,
) -> Result<()> {
    let bytes = doc
        .get_page_content(page_id)
        .map_err(|e| Error::structure(e.to_string()))?;
    let content = Content::decode(&bytes).map_err(|e| Error::structure(e.to_string()))?;
    let resources = page_resources(doc, page_id);
    let filtered = filter_ops(doc, &content.operations, &resources, boxes, secrets, 0)?;
    let encoded = Content {
        operations: filtered,
    }
    .encode()
    .map_err(|e| Error::structure(e.to_string()))?;
    doc.change_page_content(page_id, encoded)
        .map_err(|e| Error::structure(e.to_string()))?;
    Ok(())
}

fn filter_ops(
    doc: &mut lopdf::Document,
    ops: &[Operation],
    resources: &Dictionary,
    boxes: &[RedactBox],
    secrets: &mut BTreeSet<String>,
    depth: u8,
) -> Result<Vec<Operation>> {
    let mut gfx = Gfx::new();
    let mut out = Vec::new();
    for op in ops {
        match op.operator.as_str() {
            "q" => {
                gfx.push();
                out.push(op.clone());
            }
            "Q" => {
                gfx.pop();
                out.push(op.clone());
            }
            "cm" => {
                if let Some(m) = matrix6(&op.operands) {
                    gfx.concat(m);
                }
                out.push(op.clone());
            }
            "BT" => {
                gfx.in_text = true;
                gfx.text = IDENTITY;
                out.push(op.clone());
            }
            "ET" => {
                gfx.in_text = false;
                out.push(op.clone());
            }
            "Tm" => {
                if let Some(m) = matrix6(&op.operands) {
                    gfx.text = m;
                }
                out.push(op.clone());
            }
            "Td" | "TD" => {
                if op.operands.len() >= 2 {
                    let tx = num(&op.operands[0]).unwrap_or(0.0);
                    let ty = num(&op.operands[1]).unwrap_or(0.0);
                    gfx.text = mul(gfx.text, [1.0, 0.0, 0.0, 1.0, tx, ty]);
                }
                out.push(op.clone());
            }
            "Tf" => {
                if op.operands.len() >= 2 {
                    gfx.font_size = num(&op.operands[1]).unwrap_or(gfx.font_size);
                }
                out.push(op.clone());
            }
            "Tj" | "'" | "\"" => {
                let text = operand_text(op.operands.first());
                if should_drop_text(&gfx, &text, boxes) {
                    record_secret(secrets, &text);
                } else {
                    out.push(op.clone());
                }
            }
            "TJ" => {
                let text = tj_text(&op.operands);
                if should_drop_text(&gfx, &text, boxes) {
                    record_secret(secrets, &text);
                } else {
                    out.push(op.clone());
                }
            }
            "Do" => {
                let name = op
                    .operands
                    .first()
                    .and_then(|o| o.as_name().ok())
                    .map(|n| n.to_vec());
                if let Some(name) = name {
                    handle_do(doc, resources, &name, &gfx, boxes, secrets, depth)?;
                }
                out.push(op.clone());
            }
            _ => out.push(op.clone()),
        }
    }
    Ok(out)
}

fn handle_do(
    doc: &mut lopdf::Document,
    resources: &Dictionary,
    name: &[u8],
    gfx: &Gfx,
    boxes: &[RedactBox],
    secrets: &mut BTreeSet<String>,
    depth: u8,
) -> Result<()> {
    let xobjs = match resources.get(b"XObject") {
        Ok(Object::Dictionary(d)) => d.clone(),
        Ok(Object::Reference(id)) => doc
            .get_object(*id)
            .ok()
            .and_then(|o| o.as_dict().ok())
            .cloned()
            .unwrap_or_default(),
        _ => return Ok(()),
    };
    let Some(Object::Reference(id)) = xobjs.get(name).ok() else {
        return Ok(());
    };
    let id = *id;
    let subtype = doc
        .get_object(id)
        .ok()
        .and_then(|o| o.as_stream().ok())
        .and_then(|s| s.dict.get(b"Subtype").ok()?.as_name().ok().map(|n| n.to_vec()));
    match subtype.as_deref() {
        Some(b"Image") => {
            let (x0, y0) = apply(gfx.ctm, 0.0, 0.0);
            let (x1, y1) = apply(gfx.ctm, 1.0, 1.0);
            if boxes.iter().any(|b| b.intersects(x0, y0, x1, y1)) {
                punch_image(doc, id, gfx.ctm, boxes);
            }
        }
        Some(b"Form") if depth < 6 => {
            let (data, form_res) = {
                let Ok(stream) = doc.get_object(id).and_then(|o| o.as_stream()) else {
                    return Ok(());
                };
                let data = stream
                    .decompressed_content()
                    .unwrap_or_else(|_| stream.content.clone());
                let form_res = stream
                    .dict
                    .get(b"Resources")
                    .ok()
                    .and_then(|o| match o {
                        Object::Dictionary(d) => Some(d.clone()),
                        Object::Reference(rid) => doc
                            .get_object(*rid)
                            .ok()
                            .and_then(|r| r.as_dict().ok())
                            .cloned(),
                        _ => None,
                    })
                    .unwrap_or_else(|| resources.clone());
                (data, form_res)
            };
            if let Ok(inner) = Content::decode(&data) {
                let filtered = filter_ops(doc, &inner.operations, &form_res, boxes, secrets, depth + 1)?;
                let encoded = Content {
                    operations: filtered,
                }
                .encode()
                .map_err(|e| Error::structure(e.to_string()))?;
                if let Ok(Object::Stream(s)) = doc.get_object_mut(id) {
                    s.set_plain_content(encoded);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn punch_image(doc: &mut lopdf::Document, id: ObjectId, ctm: [f32; 6], boxes: &[RedactBox]) {
    let Some(stream) = doc.get_object(id).ok().and_then(|o| o.as_stream().ok()) else {
        return;
    };
    let Some((w, h, mut rgb)) = decode_image(stream) else {
        replace_image_with_black(doc, id);
        return;
    };
    let Some(inv) = invert(ctm) else {
        rgb.fill(0);
        write_image(doc, id, w, h, rgb);
        return;
    };
    for box_ in boxes {
        for &(px, py) in &[
            (box_.llx, box_.lly),
            (box_.llx, box_.ury),
            (box_.urx, box_.lly),
            (box_.urx, box_.ury),
        ] {
            let _ = (px, py);
        }
        let corners = [
            apply(inv, box_.llx, box_.lly),
            apply(inv, box_.llx, box_.ury),
            apply(inv, box_.urx, box_.lly),
            apply(inv, box_.urx, box_.ury),
        ];
        let u0 = corners.iter().map(|c| c.0).fold(f32::MAX, f32::min).clamp(0.0, 1.0);
        let u1 = corners.iter().map(|c| c.0).fold(f32::MIN, f32::max).clamp(0.0, 1.0);
        let v0 = corners.iter().map(|c| c.1).fold(f32::MAX, f32::min).clamp(0.0, 1.0);
        let v1 = corners.iter().map(|c| c.1).fold(f32::MIN, f32::max).clamp(0.0, 1.0);
        let x0 = (u0 * w as f32).floor() as u32;
        let x1 = (u1 * w as f32).ceil() as u32;
        let y0 = ((1.0 - v1) * h as f32).floor() as u32;
        let y1 = ((1.0 - v0) * h as f32).ceil() as u32;
        for y in y0..y1.min(h) {
            for x in x0..x1.min(w) {
                let i = ((y * w + x) * 3) as usize;
                if i + 2 < rgb.len() {
                    rgb[i] = 0;
                    rgb[i + 1] = 0;
                    rgb[i + 2] = 0;
                }
            }
        }
    }
    write_image(doc, id, w, h, rgb);
}

fn decode_image(stream: &Stream) -> Option<(u32, u32, Vec<u8>)> {
    let w = stream.dict.get(b"Width").ok()?.as_i64().ok()? as u32;
    let h = stream.dict.get(b"Height").ok()?.as_i64().ok()? as u32;
    if w == 0 || h == 0 {
        return None;
    }
    if is_jpeg(stream) {
        let data = stream.content.clone();
        let img = image::load_from_memory(&data).ok()?.to_rgb8();
        return Some((img.width(), img.height(), img.into_raw()));
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

fn is_jpeg(stream: &Stream) -> bool {
    match stream.dict.get(b"Filter") {
        Ok(Object::Name(n)) => n.as_slice() == b"DCTDecode",
        Ok(Object::Array(arr)) => arr
            .iter()
            .any(|o| o.as_name().ok().is_some_and(|n| n == b"DCTDecode")),
        _ => false,
    }
}

fn write_image(doc: &mut lopdf::Document, id: ObjectId, w: u32, h: u32, rgb: Vec<u8>) {
    let mut jpeg = Vec::new();
    if JpegEncoder::new_with_quality(&mut jpeg, 70)
        .encode(&rgb, w, h, ExtendedColorType::Rgb8)
        .is_err()
    {
        replace_image_with_black(doc, id);
        return;
    }
    if let Ok(Object::Stream(stream)) = doc.get_object_mut(id) {
        stream.dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        stream.dict.set("BitsPerComponent", 8);
        stream.dict.set("Width", w as i64);
        stream.dict.set("Height", h as i64);
        stream.set_plain_content(jpeg);
        stream.dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
        stream.dict.set("Length", stream.content.len() as i64);
    }
}

fn replace_image_with_black(doc: &mut lopdf::Document, id: ObjectId) {
    let mut jpeg = Vec::new();
    let rgb = [0u8, 0, 0];
    if JpegEncoder::new_with_quality(&mut jpeg, 50)
        .encode(&rgb, 1, 1, ExtendedColorType::Rgb8)
        .is_err()
    {
        return;
    }
    if let Ok(Object::Stream(stream)) = doc.get_object_mut(id) {
        stream.dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
        stream.dict.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        stream.dict.set("Width", 1);
        stream.dict.set("Height", 1);
        stream.dict.set("BitsPerComponent", 8);
        stream.set_plain_content(jpeg);
    }
}

fn drop_thumb(doc: &mut lopdf::Document, page_id: ObjectId) {
    let Ok(page) = doc.get_object_mut(page_id) else {
        return;
    };
    let Ok(dict) = page.as_dict_mut() else {
        return;
    };
    if let Ok(Object::Reference(id)) = dict.get(b"Thumb").map(|o| o.to_owned()) {
        dict.remove(b"Thumb");
        doc.objects.remove(&id);
    } else {
        dict.remove(b"Thumb");
    }
}

fn drop_annots_in_boxes(
    doc: &mut lopdf::Document,
    page_id: ObjectId,
    boxes: &[RedactBox],
) -> Result<()> {
    let annots = {
        let Ok(page) = doc.get_object(page_id) else {
            return Ok(());
        };
        let Ok(dict) = page.as_dict() else {
            return Ok(());
        };
        match dict.get(b"Annots") {
            Ok(Object::Array(a)) => a.clone(),
            Ok(Object::Reference(id)) => match doc.get_object(*id) {
                Ok(Object::Array(a)) => a.clone(),
                _ => return Ok(()),
            },
            _ => return Ok(()),
        }
    };
    let mut keep = Vec::new();
    let mut drop_ids = Vec::new();
    for item in annots {
        let Object::Reference(id) = item else {
            keep.push(item);
            continue;
        };
        if annot_intersects(doc, id, boxes) {
            clear_field_value(doc, id);
            drop_ids.push(id);
        } else {
            keep.push(Object::Reference(id));
        }
    }
    if let Ok(Object::Dictionary(dict)) = doc.get_object_mut(page_id) {
        if keep.is_empty() {
            dict.remove(b"Annots");
        } else {
            dict.set("Annots", Object::Array(keep));
        }
    }
    for id in drop_ids {
        doc.objects.remove(&id);
    }
    Ok(())
}

fn annot_intersects(doc: &lopdf::Document, id: ObjectId, boxes: &[RedactBox]) -> bool {
    let Ok(Object::Dictionary(dict)) = doc.get_object(id) else {
        return false;
    };
    let Ok(Object::Array(rect)) = dict.get(b"Rect") else {
        return false;
    };
    if rect.len() < 4 {
        return false;
    }
    let nums: Vec<f32> = rect.iter().filter_map(num_obj).collect();
    if nums.len() < 4 {
        return false;
    }
    boxes
        .iter()
        .any(|b| b.intersects(nums[0], nums[1], nums[2], nums[3]))
}

fn clear_form_values_in_boxes(doc: &mut lopdf::Document, boxes: &[RedactBox]) -> Result<()> {
    let catalog = match doc.catalog() {
        Ok(c) => c.clone(),
        Err(_) => return Ok(()),
    };
    let Ok(Object::Reference(acro_id)) = catalog.get(b"AcroForm") else {
        return Ok(());
    };
    let fields = match doc.get_object(*acro_id) {
        Ok(Object::Dictionary(d)) => match d.get(b"Fields") {
            Ok(Object::Array(a)) => a.clone(),
            _ => return Ok(()),
        },
        _ => return Ok(()),
    };
    let mut stack: Vec<ObjectId> = fields
        .iter()
        .filter_map(|o| o.as_reference().ok())
        .collect();
    while let Some(id) = stack.pop() {
        if annot_intersects(doc, id, boxes) {
            clear_field_value(doc, id);
        }
        if let Ok(Object::Dictionary(d)) = doc.get_object(id) {
            if let Ok(Object::Array(kids)) = d.get(b"Kids") {
                for kid in kids {
                    if let Ok(kid_id) = kid.as_reference() {
                        stack.push(kid_id);
                    }
                }
            }
        }
    }
    Ok(())
}

fn clear_field_value(doc: &mut lopdf::Document, id: ObjectId) {
    let Ok(Object::Dictionary(dict)) = doc.get_object_mut(id) else {
        return;
    };
    dict.set("V", Object::string_literal(""));
    dict.remove(b"AP");
    dict.remove(b"AS");
}

fn scrub_metadata(doc: &mut lopdf::Document, secrets: &BTreeSet<String>) {
    if secrets.is_empty() {
        return;
    }
    if let Ok(Object::Reference(id)) = doc.trailer.get(b"Info").map(|o| o.to_owned()) {
        if let Ok(Object::Dictionary(info)) = doc.get_object_mut(id) {
            let keys: Vec<Vec<u8>> = info.iter().map(|(k, _)| k.clone()).collect();
            for key in keys {
                if let Ok(Object::String(bytes, fmt)) = info.get(key.as_slice()) {
                    let cleaned = scrub_bytes(bytes, secrets);
                    if cleaned != *bytes {
                        info.set(key, Object::String(cleaned, *fmt));
                    }
                }
            }
        }
    }
    let meta_id = doc.catalog().ok().and_then(|c| match c.get(b"Metadata") {
        Ok(Object::Reference(id)) => Some(*id),
        _ => None,
    });
    if let Some(id) = meta_id {
        if let Ok(Object::Stream(stream)) = doc.get_object_mut(id) {
            let data = stream
                .get_plain_content()
                .unwrap_or_else(|_| stream.content.clone());
            let cleaned = scrub_bytes(&data, secrets);
            if cleaned != data {
                stream.set_plain_content(cleaned);
            }
        }
    }
}

fn wipe_secrets_everywhere(doc: &mut lopdf::Document, secrets: &BTreeSet<String>) {
    if secrets.is_empty() {
        return;
    }
    let ids: Vec<ObjectId> = doc.objects.keys().copied().collect();
    for id in ids {
        if let Ok(obj) = doc.get_object_mut(id) {
            wipe_object(obj, secrets);
        }
    }
    if let Ok(info) = doc.trailer.get_mut(b"Info") {
        wipe_object(info, secrets);
    }
}

fn wipe_object(obj: &mut Object, secrets: &BTreeSet<String>) {
    match obj {
        Object::String(bytes, _) => *bytes = scrub_bytes(bytes, secrets),
        Object::Array(items) => {
            for item in items {
                wipe_object(item, secrets);
            }
        }
        Object::Dictionary(dict) => {
            let keys: Vec<Vec<u8>> = dict.iter().map(|(k, _)| k.clone()).collect();
            for key in keys {
                if let Ok(value) = dict.get_mut(key.as_slice()) {
                    wipe_object(value, secrets);
                }
            }
        }
        Object::Stream(stream) => {
            let data = stream
                .get_plain_content()
                .unwrap_or_else(|_| stream.content.clone());
            stream.set_plain_content(scrub_bytes(&data, secrets));
            let keys: Vec<Vec<u8>> = stream.dict.iter().map(|(k, _)| k.clone()).collect();
            for key in keys {
                if let Ok(value) = stream.dict.get_mut(key.as_slice()) {
                    wipe_object(value, secrets);
                }
            }
        }
        _ => {}
    }
}

fn scrub_bytes(data: &[u8], secrets: &BTreeSet<String>) -> Vec<u8> {
    let mut out = data.to_vec();
    for secret in secrets {
        for enc in secret_encodings(secret) {
            out = remove_slice(&out, &enc);
        }
    }
    out
}

fn secret_encodings(secret: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let utf8 = secret.as_bytes();
    if utf8.is_empty() {
        return out;
    }
    out.push(utf8.to_vec());
    out.push(utf16_be(secret, true));
    out.push(utf16_be(secret, false));
    out.push(utf16_le(secret, true));
    out.push(utf16_le(secret, false));
    out.push(to_hex(utf8, false));
    out.push(to_hex(utf8, true));
    let be = utf16_be(secret, false);
    out.push(to_hex(&be, false));
    out.push(to_hex(&be, true));
    out
}

fn utf16_be(s: &str, bom: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if bom {
        out.extend_from_slice(&[0xFE, 0xFF]);
    }
    for unit in s.encode_utf16() {
        out.extend_from_slice(&unit.to_be_bytes());
    }
    out
}

fn utf16_le(s: &str, bom: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if bom {
        out.extend_from_slice(&[0xFF, 0xFE]);
    }
    for unit in s.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

fn to_hex(bytes: &[u8], upper: bool) -> Vec<u8> {
    const LOW: &[u8; 16] = b"0123456789abcdef";
    const UP: &[u8; 16] = b"0123456789ABCDEF";
    let table = if upper { UP } else { LOW };
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(table[(b >> 4) as usize]);
        out.push(table[(b & 0x0F) as usize]);
    }
    out
}

fn remove_slice(hay: &[u8], needle: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return hay.to_vec();
    }
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(needle) {
            i += needle.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

fn record_secret(secrets: &mut BTreeSet<String>, text: &str) {
    let t = text.trim();
    if t.len() >= 3 {
        secrets.insert(t.to_string());
    }
}

fn should_drop_text(gfx: &Gfx, text: &str, boxes: &[RedactBox]) -> bool {
    if text.is_empty() {
        return false;
    }
    let tm = mul(gfx.ctm, gfx.text);
    let (x, y) = apply(tm, 0.0, 0.0);
    let w = text.chars().count() as f32 * gfx.font_size * 0.5;
    let h = gfx.font_size;
    boxes.iter().any(|b| b.intersects(x, y, x + w, y + h))
}

fn operand_text(obj: Option<&Object>) -> String {
    match obj {
        Some(Object::String(b, _)) => pdf_string_text(b),
        _ => String::new(),
    }
}

fn pdf_string_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .chunks(2)
            .filter_map(|c| (c.len() == 2).then_some(u16::from_be_bytes([c[0], c[1]])))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..]
            .chunks(2)
            .filter_map(|c| (c.len() == 2).then_some(u16::from_le_bytes([c[0], c[1]])))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn tj_text(operands: &[Object]) -> String {
    let Some(Object::Array(items)) = operands.first() else {
        return String::new();
    };
    let mut s = String::new();
    for item in items {
        if let Object::String(b, _) = item {
            s.push_str(&pdf_string_text(b));
        }
    }
    s
}

fn page_resources(doc: &lopdf::Document, mut id: ObjectId) -> Dictionary {
    for _ in 0..16 {
        let Ok(obj) = doc.get_object(id) else {
            break;
        };
        let Ok(dict) = obj.as_dict() else {
            break;
        };
        if let Ok(res) = dict.get(b"Resources") {
            return match res {
                Object::Dictionary(d) => d.clone(),
                Object::Reference(rid) => doc
                    .get_object(*rid)
                    .ok()
                    .and_then(|o| o.as_dict().ok())
                    .cloned()
                    .unwrap_or_default(),
                _ => Dictionary::new(),
            };
        }
        let Ok(parent) = dict.get(b"Parent").and_then(|o| o.as_reference()) else {
            break;
        };
        id = parent;
    }
    Dictionary::new()
}

const IDENTITY: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

struct Gfx {
    ctm: [f32; 6],
    stack: Vec<[f32; 6]>,
    in_text: bool,
    text: [f32; 6],
    font_size: f32,
}

impl Gfx {
    fn new() -> Self {
        Self {
            ctm: IDENTITY,
            stack: Vec::new(),
            in_text: false,
            text: IDENTITY,
            font_size: 12.0,
        }
    }

    fn push(&mut self) {
        self.stack.push(self.ctm);
    }

    fn pop(&mut self) {
        if let Some(m) = self.stack.pop() {
            self.ctm = m;
        }
    }

    fn concat(&mut self, m: [f32; 6]) {
        self.ctm = mul(self.ctm, m);
    }
}

fn mul(m: [f32; 6], n: [f32; 6]) -> [f32; 6] {
    [
        m[0] * n[0] + m[2] * n[1],
        m[1] * n[0] + m[3] * n[1],
        m[0] * n[2] + m[2] * n[3],
        m[1] * n[2] + m[3] * n[3],
        m[0] * n[4] + m[2] * n[5] + m[4],
        m[1] * n[4] + m[3] * n[5] + m[5],
    ]
}

fn apply(m: [f32; 6], x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn invert(m: [f32; 6]) -> Option<[f32; 6]> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-8 {
        return None;
    }
    Some([
        m[3] / det,
        -m[1] / det,
        -m[2] / det,
        m[0] / det,
        (m[2] * m[5] - m[3] * m[4]) / det,
        (m[1] * m[4] - m[0] * m[5]) / det,
    ])
}

fn matrix6(ops: &[Object]) -> Option<[f32; 6]> {
    if ops.len() < 6 {
        return None;
    }
    Some([
        num(&ops[0])?,
        num(&ops[1])?,
        num(&ops[2])?,
        num(&ops[3])?,
        num(&ops[4])?,
        num(&ops[5])?,
    ])
}

fn num(obj: &Object) -> Option<f32> {
    num_obj(obj)
}

fn num_obj(obj: &Object) -> Option<f32> {
    match obj {
        Object::Integer(v) => Some(*v as f32),
        Object::Real(v) => Some(*v),
        _ => None,
    }
}

fn minmax(a: f32, b: f32) -> (f32, f32) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signer::append_stub_signature;
    use crate::test_support::corpus;
    use lopdf::content::{Content, Operation};
    use lopdf::dictionary;
    use lopdf::StringFormat;

    const SECRET: &str = "SECRET42";

    #[derive(Clone, Copy)]
    enum SecretDraw {
        LiteralTj,
        TjKerning,
        HexTj,
    }

    fn secret_pdf(draw: SecretDraw, compress: bool) -> Document {
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
        let show = match draw {
            SecretDraw::LiteralTj => Operation::new("Tj", vec![Object::string_literal(SECRET)]),
            SecretDraw::TjKerning => Operation::new(
                "TJ",
                vec![Object::Array(vec![
                    Object::string_literal("SEC"),
                    Object::Integer(-20),
                    Object::string_literal("RET42"),
                ])],
            ),
            SecretDraw::HexTj => Operation::new(
                "Tj",
                vec![Object::String(
                    SECRET.as_bytes().to_vec(),
                    StringFormat::Hexadecimal,
                )],
            ),
        };
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 18.into()]),
                Operation::new("Td", vec![72.into(), 700.into()]),
                show,
                Operation::new("ET", vec![]),
            ],
        };
        let mut content_stream = Stream::new(Dictionary::new(), content.encode().unwrap());
        if compress {
            let _ = content_stream.compress();
        }
        let content_id = doc.add_object(content_stream);
        let mut thumb = Stream::new(Dictionary::new(), SECRET.as_bytes().to_vec());
        if compress {
            let _ = thumb.compress();
        }
        let thumb_id = doc.add_object(thumb);
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Thumb" => thumb_id,
        });
        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
            "Resources" => resources_id,
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let info_id = doc.add_object(dictionary! {
            "Title" => Object::string_literal(SECRET),
            "Subject" => Object::string_literal(format!("about {SECRET}")),
        });
        let xmp = format!("<x:xmpmeta>{SECRET} hidden</x:xmpmeta>");
        let mut meta = Stream::new(Dictionary::new(), xmp.into_bytes());
        if compress {
            let _ = meta.compress();
        }
        let meta_id = doc.add_object(meta);
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
            "Metadata" => meta_id,
        });
        doc.trailer.set("Root", catalog_id);
        doc.trailer.set("Info", info_id);
        Document::from_lopdf(doc, None).unwrap()
    }

    fn assert_secret_present(bytes: &[u8]) {
        assert!(
            secret_in_pdf(bytes, SECRET),
            "fixture must contain {SECRET} after decompressing streams and checking encodings"
        );
    }

    fn assert_secret_absent(bytes: &[u8]) {
        if let Some(where_) = secret_in_pdf_where(bytes, SECRET) {
            panic!("secret still present in rewritten PDF ({where_})");
        }
        let reloaded = lopdf::Document::load_mem(bytes).unwrap();
        let extracted = reloaded.extract_text(&[1]).unwrap_or_default();
        assert!(
            !extracted.contains(SECRET),
            "extract_text still has secret: {extracted}"
        );
        if let Some(id) = reloaded.get_pages().get(&1) {
            if let Ok(page) = reloaded.get_object(*id).and_then(|o| o.as_dict()) {
                assert!(page.get(b"Thumb").is_err(), "Thumb must be removed");
            }
        }
    }

    fn secret_in_pdf(bytes: &[u8], secret: &str) -> bool {
        secret_in_pdf_where(bytes, secret).is_some()
    }

    fn secret_in_pdf_where(bytes: &[u8], secret: &str) -> Option<&'static str> {
        if encodings_contain(bytes, secret) {
            return Some("raw file encodings");
        }
        let doc = lopdf::Document::load_mem(bytes).ok()?;
        for obj in doc.objects.values() {
            if let Some(w) = object_has_secret(obj, secret) {
                return Some(w);
            }
        }
        if let Ok(text) = doc.extract_text(&[1]) {
            if text.contains(secret) {
                return Some("extract_text");
            }
        }
        None
    }

    fn object_has_secret(obj: &Object, secret: &str) -> Option<&'static str> {
        match obj {
            Object::String(b, _) => {
                if encodings_contain(b, secret) || pdf_string_text(b).contains(secret) {
                    Some("string object")
                } else {
                    None
                }
            }
            Object::Array(items) => items.iter().find_map(|o| object_has_secret(o, secret)),
            Object::Dictionary(dict) => dict.iter().find_map(|(_, v)| object_has_secret(v, secret)),
            Object::Stream(stream) => {
                if encodings_contain(&stream.content, secret) {
                    return Some("compressed stream");
                }
                let plain = stream
                    .get_plain_content()
                    .or_else(|_| stream.decompressed_content())
                    .unwrap_or_else(|_| stream.content.clone());
                if encodings_contain(&plain, secret) {
                    return Some("decompressed stream");
                }
                if content_strings_joined(&plain).contains(secret) {
                    return Some("content-stream strings");
                }
                stream
                    .dict
                    .iter()
                    .find_map(|(_, v)| object_has_secret(v, secret))
            }
            _ => None,
        }
    }

    fn content_strings_joined(data: &[u8]) -> String {
        let Ok(content) = Content::decode(data) else {
            return String::new();
        };
        let mut out = String::new();
        for op in content.operations {
            collect_pdf_strings(&Object::Array(op.operands), &mut out);
        }
        out
    }

    fn collect_pdf_strings(obj: &Object, out: &mut String) {
        match obj {
            Object::String(b, _) => out.push_str(&pdf_string_text(b)),
            Object::Array(items) => {
                for item in items {
                    collect_pdf_strings(item, out);
                }
            }
            Object::Dictionary(dict) => {
                for (_, v) in dict.iter() {
                    collect_pdf_strings(v, out);
                }
            }
            _ => {}
        }
    }

    fn encodings_contain(hay: &[u8], secret: &str) -> bool {
        secret_encodings(secret)
            .iter()
            .any(|enc| hay.windows(enc.len()).any(|w| w == enc.as_slice()))
    }

    fn redact_box() -> RedactBox {
        RedactBox::new(0, 50.0, 680.0, 300.0, 740.0).unwrap()
    }

    #[test]
    fn redact_refuses_signed() {
        let bytes = append_stub_signature(&std::fs::read(corpus("hello.pdf")).unwrap()).unwrap();
        let signed = Document::from_bytes(bytes, None).unwrap();
        assert!(matches!(
            redact(&signed, &[RedactBox::new(0, 0.0, 0.0, 100.0, 100.0).unwrap()]),
            Err(Error::Signed {
                op: SignedOp::Rewrite
            })
        ));
    }

    #[test]
    fn redacted_literal_tj_not_extractable() {
        let doc = secret_pdf(SecretDraw::LiteralTj, true);
        assert_secret_present(&doc.to_bytes().unwrap());
        let out = redact(&doc, &[redact_box()]).unwrap();
        assert_secret_absent(&out.to_bytes().unwrap());
    }

    #[test]
    fn redacted_tj_kerning_split_not_extractable() {
        let doc = secret_pdf(SecretDraw::TjKerning, true);
        assert_secret_present(&doc.to_bytes().unwrap());
        let out = redact(&doc, &[redact_box()]).unwrap();
        assert_secret_absent(&out.to_bytes().unwrap());
    }

    #[test]
    fn redacted_hex_string_not_extractable() {
        let doc = secret_pdf(SecretDraw::HexTj, true);
        assert_secret_present(&doc.to_bytes().unwrap());
        let out = redact(&doc, &[redact_box()]).unwrap();
        assert_secret_absent(&out.to_bytes().unwrap());
    }

    #[test]
    fn empty_boxes_are_invalid() {
        let doc = Document::open(corpus("hello.pdf")).unwrap();
        assert!(matches!(
            redact(&doc, &[]),
            Err(Error::Invalid(InvalidKind::NoRedactionBoxes))
        ));
    }
}
