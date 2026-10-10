use std::io::Write;
use std::path::Path;

use lopdf::{IncrementalDocument, Object};

use crate::document::{Document, PageSlot};
use crate::error::{Error, Result, SignedOp};

impl Document {
    /// Rewrite the PDF (optimized "Save as"). Existing signatures are not preserved.
    ///
    /// Signed documents must use [`Document::save_incremental`] instead.
    pub fn save<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let bytes = self.to_bytes()?;
        std::fs::write(path, &bytes)?;
        self.reload_from_saved(bytes)
    }

    /// Append an incremental update so the original bytes stay intact (signatures survive).
    pub fn save_incremental<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let bytes = self.to_incremental_bytes()?;
        std::fs::write(path, &bytes)?;
        self.reload_from_saved(bytes)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        if self.is_signed() {
            return Err(Error::signed(SignedOp::Rewrite));
        }
        let mut doc = self.assembled_lopdf()?;
        let mut buf = Vec::new();
        doc.save_to(&mut buf)?;
        Ok(buf)
    }

    /// Original file bytes plus an incremental update describing the current page list.
    pub fn to_incremental_bytes(&self) -> Result<Vec<u8>> {
        if !self.is_dirty() {
            return Ok(self.original_slice().to_vec());
        }
        let prev = lopdf::Document::load_mem(self.original_slice())?;
        let mut inc = IncrementalDocument::create_from(self.original_slice().to_vec(), prev);
        apply_slots_incremental(&mut inc, self.slots())?;
        let mut buf = Vec::new();
        inc.save_to(&mut buf)?;
        if !buf.starts_with(self.original_slice()) {
            return Err(Error::structure(
                "incremental save did not preserve original bytes",
            ));
        }
        Ok(buf)
    }

    pub fn write_to<W: Write>(&self, mut w: W) -> Result<()> {
        if self.is_signed() {
            w.write_all(&self.to_incremental_bytes()?)?;
        } else {
            w.write_all(&self.to_bytes()?)?;
        }
        Ok(())
    }
}

fn apply_slots_incremental(inc: &mut IncrementalDocument, slots: &[PageSlot]) -> Result<()> {
    let pages_id = inc
        .get_prev_documents()
        .catalog()?
        .get(b"Pages")?
        .as_reference()
        .map_err(|e| Error::structure(e.to_string()))?;
    inc.opt_clone_object_to_new_document(pages_id)?;
    for slot in slots {
        inc.opt_clone_object_to_new_document(slot.obj_id)?;
        if let Ok(Object::Dictionary(dict)) = inc.new_document.get_object_mut(slot.obj_id) {
            if slot.rotation == 0 {
                dict.remove(b"Rotate");
            } else {
                dict.set("Rotate", slot.rotation as i64);
            }
            dict.set("Parent", pages_id);
        }
    }
    if let Ok(Object::Dictionary(dict)) = inc.new_document.get_object_mut(pages_id) {
        dict.set(
            "Kids",
            Object::Array(
                slots
                    .iter()
                    .map(|s| Object::Reference(s.obj_id))
                    .collect(),
            ),
        );
        dict.set("Count", Object::Integer(slots.len() as i64));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::document::Document;
    use crate::test_support::corpus;

    #[test]
    fn incremental_save_keeps_prefix() {
        let mut doc = Document::open(corpus("hello.pdf")).unwrap();
        let original = doc.original_slice().to_vec();
        doc.rotate_page(0, 90).unwrap();
        let saved = doc.to_incremental_bytes().unwrap();
        assert!(
            saved.starts_with(&original),
            "incremental output must start with the original file"
        );
        assert!(saved.len() > original.len());
        let reloaded = Document::from_bytes(saved, None).unwrap();
        assert_eq!(reloaded.page(0).unwrap().rotation, 90);
    }
}
