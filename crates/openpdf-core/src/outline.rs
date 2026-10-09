use lopdf::{Dictionary, Document as LopdfDocument, Object, ObjectId};

use crate::error::{Error, Result};

/// Bookmark / outline entry. `page` is 0-based when it resolves to a document page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineItem {
    pub title: String,
    pub page: Option<u32>,
    pub children: Vec<OutlineItem>,
}

pub(crate) fn load_outline(doc: &LopdfDocument) -> Result<Vec<OutlineItem>> {
    let catalog = match doc.catalog() {
        Ok(c) => c,
        Err(_) => return Ok(Vec::new()),
    };
    let outlines = match catalog.get(b"Outlines") {
        Ok(Object::Reference(id)) => match doc.get_object(*id) {
            Ok(Object::Dictionary(dict)) => dict,
            _ => return Ok(Vec::new()),
        },
        Ok(Object::Dictionary(dict)) => dict,
        _ => return Ok(Vec::new()),
    };
    let Some(first) = dict_ref(outlines, b"First") else {
        return Ok(Vec::new());
    };
    let page_map = page_object_map(doc);
    walk_siblings(doc, first, &page_map, 0)
}

fn walk_siblings(
    doc: &LopdfDocument,
    start: ObjectId,
    page_map: &std::collections::HashMap<ObjectId, u32>,
    depth: usize,
) -> Result<Vec<OutlineItem>> {
    if depth > 64 {
        return Err(Error::structure("outline nest too deep"));
    }
    let mut items = Vec::new();
    let mut current = Some(start);
    let mut guard = 0;
    while let Some(id) = current {
        guard += 1;
        if guard > 10_000 {
            return Err(Error::structure("outline sibling loop"));
        }
        let dict = match doc.get_object(id).ok().and_then(|o| o.as_dict().ok()) {
            Some(d) => d,
            None => break,
        };
        let title = dict
            .get(b"Title")
            .ok()
            .map(object_string)
            .unwrap_or_else(|| "(untitled)".into());
        let page = dest_page(doc, dict, page_map);
        let children = dict_ref(dict, b"First")
            .map(|child| walk_siblings(doc, child, page_map, depth + 1))
            .transpose()?
            .unwrap_or_default();
        items.push(OutlineItem {
            title,
            page,
            children,
        });
        current = dict_ref(dict, b"Next");
    }
    Ok(items)
}

fn dest_page(
    doc: &LopdfDocument,
    dict: &Dictionary,
    page_map: &std::collections::HashMap<ObjectId, u32>,
) -> Option<u32> {
    let dest = dict.get(b"Dest").ok().or_else(|| {
        dict.get(b"A")
            .ok()
            .and_then(|a| a.as_dict().ok())
            .and_then(|a| a.get(b"D").ok())
    })?;
    dest_object_page(doc, dest, page_map)
}

fn dest_object_page(
    doc: &LopdfDocument,
    dest: &Object,
    page_map: &std::collections::HashMap<ObjectId, u32>,
) -> Option<u32> {
    match dest {
        Object::Array(arr) => match arr.first()? {
            Object::Reference(id) => page_map.get(id).copied(),
            Object::Integer(i) if *i >= 0 => Some(*i as u32),
            _ => None,
        },
        Object::Reference(id) => {
            let obj = doc.get_object(*id).ok()?;
            dest_object_page(doc, obj, page_map)
        }
        _ => None,
    }
}

fn page_object_map(doc: &LopdfDocument) -> std::collections::HashMap<ObjectId, u32> {
    doc.get_pages()
        .into_iter()
        .map(|(num, id)| (id, num.saturating_sub(1)))
        .collect()
}

fn dict_ref(dict: &Dictionary, key: &[u8]) -> Option<ObjectId> {
    dict.get(key).ok().and_then(|o| o.as_reference().ok())
}

fn object_string(obj: &Object) -> String {
    match obj {
        Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        _ => String::new(),
    }
}
