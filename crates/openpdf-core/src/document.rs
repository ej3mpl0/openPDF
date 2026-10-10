use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use lopdf::{Dictionary, Document as LopdfDocument, LoadOptions, Object, ObjectId};
use zeroize::Zeroizing;

use crate::command::{normalize_rotation, Applied, Command, History};
use crate::error::{Error, Result, SignedOp};
use crate::outline::{load_outline, OutlineItem};
use crate::signer::contains_signature;

static NEXT_DOC_ID: AtomicU64 = AtomicU64::new(1);

/// Stable id used as a cache / render-queue key. Assigned when the document is opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocumentId(pub u64);

impl DocumentId {
    pub fn next() -> Self {
        Self(NEXT_DOC_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// Page metrics in PDF points (1/72 in). `index` is 0-based.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageInfo {
    pub index: u32,
    pub width_pt: f32,
    pub height_pt: f32,
    pub rotation: i32,
}

#[derive(Clone, Debug)]
pub(crate) struct PageSlot {
    pub obj_id: ObjectId,
    pub rotation: i32,
}

/// Password of an encrypted document, wiped from memory on drop.
///
/// Deliberately has no `Clone`, `Debug` or `Display`: there is exactly one
/// copy per opened document (shared via `Arc` by clones of that `Document`),
/// and it can never be printed. It is dropped when the last `Document` clone
/// is dropped (closed).
pub(crate) struct DocSecret(Zeroizing<String>);

impl DocSecret {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Optional [`DocSecret`] holder with a redacted `Debug`.
#[derive(Clone, Default)]
pub(crate) struct PasswordSlot(Option<Arc<DocSecret>>);

impl PasswordSlot {
    pub(crate) fn new(password: Zeroizing<String>) -> Self {
        Self(Some(Arc::new(DocSecret(password))))
    }

    pub(crate) fn get(&self) -> Option<&Arc<DocSecret>> {
        self.0.as_ref()
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        self.0.as_deref().map(DocSecret::as_str)
    }
}

impl fmt::Debug for PasswordSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() {
            "PasswordSlot(<redacted>)"
        } else {
            "PasswordSlot(None)"
        })
    }
}

/// Parse `bytes` with lopdf, decrypting with `password` when given.
///
/// An encrypted file that cannot be decrypted is `PasswordRequired` (no
/// password given) or `WrongPassword`, never `EmptyDocument`.
pub(crate) fn load_lopdf(bytes: &[u8], password: Option<&str>) -> Result<LopdfDocument> {
    let loaded = match password {
        // lopdf's LoadOptions takes a plain String; that copy is lopdf's and
        // short-lived, but it is not zeroized.
        Some(pw) => LopdfDocument::load_mem_with_options(bytes, LoadOptions::with_password(pw)),
        None => LopdfDocument::load_mem(bytes),
    };
    let doc = loaded.map_err(|e| match e {
        lopdf::Error::InvalidPassword => Error::WrongPassword,
        e => Error::from(e),
    })?;
    if doc.is_encrypted() {
        return Err(if password.is_some() {
            Error::WrongPassword
        } else {
            Error::PasswordRequired
        });
    }
    Ok(doc)
}

/// Re-apply the document's encryption before writing, so a document opened
/// with a password is never written out in plaintext by accident.
pub(crate) fn reencrypt_for_write(doc: &mut LopdfDocument) -> Result<()> {
    if let Some(state) = doc.encryption_state.take() {
        doc.encrypt(&state)?;
    }
    Ok(())
}

/// Structure-level PDF document. Rendering and text go through [`crate::Engine`].
#[derive(Clone, Debug)]
pub struct Document {
    id: DocumentId,
    path: Option<PathBuf>,
    original: Vec<u8>,
    inner: LopdfDocument,
    slots: Vec<PageSlot>,
    history: History,
    dirty: bool,
    signed: bool,
    password: PasswordSlot,
}

impl Document {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)?;
        Self::from_bytes(bytes, Some(path.to_path_buf()))
    }

    /// Open an unencrypted PDF (or one with an empty user password).
    ///
    /// Encrypted PDFs return [`Error::PasswordRequired`]; use
    /// [`Document::from_bytes_with_password`].
    pub fn from_bytes(bytes: Vec<u8>, path: Option<PathBuf>) -> Result<Self> {
        let inner = load_lopdf(&bytes, None)?;
        Self::from_lopdf_inner(inner, bytes, path, PasswordSlot::default())
    }

    /// Open a PDF with its user or owner password.
    ///
    /// A wrong password is [`Error::WrongPassword`]. The password is kept
    /// (zeroized on drop, never logged) so saves can re-encrypt and the render
    /// engine can open the file. Unencrypted files ignore it.
    pub fn from_bytes_with_password(
        bytes: Vec<u8>,
        path: Option<PathBuf>,
        password: Zeroizing<String>,
    ) -> Result<Self> {
        let inner = load_lopdf(&bytes, Some(&password))?;
        let slot = if inner.was_encrypted() {
            PasswordSlot::new(password)
        } else {
            PasswordSlot::default()
        };
        Self::from_lopdf_inner(inner, bytes, path, slot)
    }

    /// File variant of [`Document::from_bytes_with_password`].
    pub fn open_with_password(path: impl AsRef<Path>, password: Zeroizing<String>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)?;
        Self::from_bytes_with_password(bytes, Some(path.to_path_buf()), password)
    }

    pub(crate) fn from_lopdf(inner: LopdfDocument, path: Option<PathBuf>) -> Result<Self> {
        Self::from_lopdf_with(inner, path, PasswordSlot::default())
    }

    /// Like [`Document::from_lopdf`], keeping this document's password so an
    /// encrypted source yields an encrypted result.
    pub(crate) fn derive(&self, inner: LopdfDocument) -> Result<Self> {
        Self::from_lopdf_with(inner, None, self.password.clone())
    }

    /// [`Document::derive`] for a freshly built lopdf document (e.g. merge
    /// output) that carries no encryption state of its own: it is encrypted
    /// with this document's key and reopened with this document's password.
    pub(crate) fn derive_encrypted(&self, mut inner: LopdfDocument) -> Result<Self> {
        inner.encryption_state = self.inner.encryption_state.clone();
        // RC4 / AES-128 (V < 5) keys and the /U check are derived from the
        // first file ID, so the output must carry the source's /ID.
        if let Ok(id) = self.inner.trailer.get(b"ID") {
            inner.trailer.set("ID", id.clone());
        }
        self.derive(inner)
    }

    pub(crate) fn from_lopdf_with(
        inner: LopdfDocument,
        path: Option<PathBuf>,
        password: PasswordSlot,
    ) -> Result<Self> {
        let mut tmp = inner;
        reencrypt_for_write(&mut tmp)?;
        let mut bytes = Vec::new();
        tmp.save_to(&mut bytes)?;
        let inner = load_lopdf(&bytes, password.as_str())?;
        let password = if inner.was_encrypted() {
            password
        } else {
            PasswordSlot::default()
        };
        Self::from_lopdf_inner(inner, bytes, path, password)
    }

    fn from_lopdf_inner(
        inner: LopdfDocument,
        original: Vec<u8>,
        path: Option<PathBuf>,
        password: PasswordSlot,
    ) -> Result<Self> {
        let slots = slots_from(&inner)?;
        let signed = contains_signature(&inner);
        Ok(Self {
            id: DocumentId::next(),
            path,
            original,
            inner,
            slots,
            history: History::new(),
            dirty: false,
            signed,
            password,
        })
    }

    pub fn id(&self) -> DocumentId {
        self.id
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn page_count(&self) -> u32 {
        self.slots.len() as u32
    }

    pub fn pages(&self) -> Vec<PageInfo> {
        (0..self.slots.len())
            .filter_map(|i| self.page(i as u32).ok())
            .collect()
    }

    pub fn page(&self, index: u32) -> Result<PageInfo> {
        let slot = self
            .slots
            .get(index as usize)
            .ok_or(Error::PageOutOfRange {
                page: index,
                count: self.page_count(),
            })?;
        let (w, h) = media_box(&self.inner, slot.obj_id)?;
        let (width_pt, height_pt) = if slot.rotation == 90 || slot.rotation == 270 {
            (h, w)
        } else {
            (w, h)
        };
        Ok(PageInfo {
            index,
            width_pt,
            height_pt,
            rotation: slot.rotation,
        })
    }

    pub fn outline(&self) -> Result<Vec<OutlineItem>> {
        load_outline(&self.inner)
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// True when the PDF contains at least one signature dictionary (`/Type /Sig` or `/FT /Sig`).
    pub fn is_signed(&self) -> bool {
        self.signed
    }

    /// True when the file is encrypted (whether or not it was opened with a password).
    pub fn is_encrypted(&self) -> bool {
        self.inner.is_encrypted() || self.inner.was_encrypted()
    }

    /// Password this document was opened with, for the render engine and saves.
    pub(crate) fn password(&self) -> &PasswordSlot {
        &self.password
    }

    /// Signed documents are locked against rotate/delete/move and full rewrites.
    /// The UI should treat this as read-only except for incremental (append-only) save.
    pub fn is_read_only(&self) -> bool {
        self.signed
    }

    /// Error if rotate/delete/move would break a signature. The UI can call this
    /// before enabling those tools.
    pub fn ensure_editable(&self) -> Result<()> {
        if self.signed {
            Err(Error::signed(SignedOp::Edit))
        } else {
            Ok(())
        }
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    pub fn execute(&mut self, command: Command) -> Result<()> {
        match command {
            Command::Rotate { index, delta } => self.rotate_page(index as u32, delta),
            Command::Delete { index } => self.delete_page(index as u32),
            Command::Move { from, to } => self.move_page(from as u32, to as u32),
        }
    }

    pub fn rotate_page(&mut self, index: u32, degrees: i32) -> Result<()> {
        let i = self.check_index(index)?;
        let delta = normalize_rotation(degrees);
        if delta == 0 {
            return Ok(());
        }
        if self.signed {
            return Err(Error::signed(SignedOp::Rotate));
        }
        self.slots[i].rotation = normalize_rotation(self.slots[i].rotation + delta);
        self.dirty = true;
        self.history.push(Applied::Rotate { index: i, delta });
        Ok(())
    }

    pub fn delete_page(&mut self, index: u32) -> Result<()> {
        if self.signed {
            return Err(Error::signed(SignedOp::Delete));
        }
        if self.slots.len() <= 1 {
            return Err(Error::EmptyDocument);
        }
        let i = self.check_index(index)?;
        let slot = self.slots.remove(i);
        self.dirty = true;
        self.history.push(Applied::Delete { index: i, slot });
        Ok(())
    }

    pub fn move_page(&mut self, from: u32, to: u32) -> Result<()> {
        let f = self.check_index(from)?;
        if to as usize > self.slots.len() {
            return Err(Error::PageOutOfRange {
                page: to,
                count: self.page_count(),
            });
        }
        let t = to as usize;
        if f == t || f + 1 == t {
            return Ok(());
        }
        if self.signed {
            return Err(Error::signed(SignedOp::Move));
        }
        let slot = self.slots.remove(f);
        let insert_at = if t > f { t - 1 } else { t };
        self.slots.insert(insert_at, slot);
        self.dirty = true;
        self.history.push(Applied::Move { from: f, to: t });
        Ok(())
    }

    pub fn undo(&mut self) -> Result<bool> {
        if !self.history.can_undo() {
            return Ok(false);
        }
        if self.signed {
            return Err(Error::signed(SignedOp::Undo));
        }
        let Some(applied) = self.history.pop_undo() else {
            return Ok(false);
        };
        let redo = self.apply_inverse(applied)?;
        self.history.record_redo(redo);
        self.dirty = true;
        Ok(true)
    }

    pub fn redo(&mut self) -> Result<bool> {
        if !self.history.can_redo() {
            return Ok(false);
        }
        if self.signed {
            return Err(Error::signed(SignedOp::Redo));
        }
        let Some(applied) = self.history.pop_redo() else {
            return Ok(false);
        };
        let undo = self.apply_forward(applied)?;
        self.history.record_undo(undo);
        self.dirty = true;
        Ok(true)
    }

    fn apply_inverse(&mut self, applied: Applied) -> Result<Applied> {
        match applied {
            Applied::Rotate { index, delta } => {
                self.slots[index].rotation =
                    normalize_rotation(self.slots[index].rotation - delta);
                Ok(Applied::Rotate { index, delta })
            }
            Applied::Delete { index, slot } => {
                self.slots.insert(index, slot);
                Ok(Applied::Insert { index })
            }
            Applied::Insert { index } => {
                let slot = self.slots.remove(index);
                Ok(Applied::Delete { index, slot })
            }
            Applied::Move { from, to } => {
                self.move_slot_raw(to, from)?;
                Ok(Applied::Move { from, to })
            }
        }
    }

    fn apply_forward(&mut self, applied: Applied) -> Result<Applied> {
        match applied {
            Applied::Rotate { index, delta } => {
                self.slots[index].rotation =
                    normalize_rotation(self.slots[index].rotation + delta);
                Ok(Applied::Rotate { index, delta })
            }
            Applied::Delete { index, slot: _ } => {
                let slot = self.slots.remove(index);
                Ok(Applied::Delete { index, slot })
            }
            Applied::Insert { index } => {
                // Redo of undo-delete: the inverse of Insert is Delete; the slot
                // was restored on undo, so redo removes it again.
                let slot = self.slots.remove(index);
                Ok(Applied::Delete { index, slot })
            }
            Applied::Move { from, to } => {
                self.move_slot_raw(from, to)?;
                Ok(Applied::Move { from, to })
            }
        }
    }

    fn move_slot_raw(&mut self, from: usize, to: usize) -> Result<()> {
        if from >= self.slots.len() {
            return Err(Error::PageOutOfRange {
                page: from as u32,
                count: self.page_count(),
            });
        }
        let slot = self.slots.remove(from);
        let insert_at = if to > from { to.saturating_sub(1) } else { to };
        let insert_at = insert_at.min(self.slots.len());
        self.slots.insert(insert_at, slot);
        Ok(())
    }

    fn check_index(&self, index: u32) -> Result<usize> {
        if (index as usize) < self.slots.len() {
            Ok(index as usize)
        } else {
            Err(Error::PageOutOfRange {
                page: index,
                count: self.page_count(),
            })
        }
    }

    pub(crate) fn original_bytes(&self) -> &[u8] {
        &self.original
    }

    pub(crate) fn slots(&self) -> &[PageSlot] {
        &self.slots
    }

    pub(crate) fn lopdf_clone(&self) -> LopdfDocument {
        self.assembled_lopdf().unwrap_or_else(|_| self.inner.clone())
    }

    pub(crate) fn assembled_lopdf(&self) -> Result<LopdfDocument> {
        let mut doc = self.inner.clone();
        apply_slots(&mut doc, &self.slots)?;
        Ok(doc)
    }

    pub(crate) fn reload_from_saved(&mut self, bytes: Vec<u8>) -> Result<()> {
        let inner = load_lopdf(&bytes, self.password.as_str())?;
        self.slots = slots_from(&inner)?;
        self.signed = contains_signature(&inner);
        self.inner = inner;
        self.original = bytes;
        self.dirty = false;
        Ok(())
    }
}

fn slots_from(doc: &LopdfDocument) -> Result<Vec<PageSlot>> {
    let mut slots = Vec::new();
    for (_num, id) in doc.get_pages() {
        let rotation = page_rotate(doc, id);
        slots.push(PageSlot {
            obj_id: id,
            rotation,
        });
    }
    if slots.is_empty() {
        return Err(Error::EmptyDocument);
    }
    Ok(slots)
}

pub(crate) fn apply_slots(doc: &mut LopdfDocument, slots: &[PageSlot]) -> Result<()> {
    if slots.is_empty() {
        return Err(Error::EmptyDocument);
    }
    let pages_id = doc
        .catalog()?
        .get(b"Pages")?
        .as_reference()
        .map_err(|e| Error::structure(e.to_string()))?;
    {
        let pages_obj = doc.get_object_mut(pages_id)?;
        let dict: &mut Dictionary = pages_obj
            .as_dict_mut()
            .map_err(|e| Error::structure(e.to_string()))?;
        dict.set(
            "Kids",
            Object::Array(slots.iter().map(|s| Object::Reference(s.obj_id)).collect()),
        );
        dict.set("Count", Object::Integer(slots.len() as i64));
    }
    for slot in slots {
        if let Ok(Object::Dictionary(dict)) = doc.get_object_mut(slot.obj_id) {
            if slot.rotation == 0 {
                dict.remove(b"Rotate");
            } else {
                dict.set("Rotate", slot.rotation as i64);
            }
            dict.set("Parent", Object::Reference(pages_id));
        }
    }
    Ok(())
}

fn page_rotate(doc: &LopdfDocument, mut id: ObjectId) -> i32 {
    for _ in 0..16 {
        let Ok(Object::Dictionary(dict)) = doc.get_object(id) else {
            break;
        };
        if let Ok(Object::Integer(r)) = dict.get(b"Rotate") {
            return normalize_rotation(*r as i32);
        }
        match dict.get(b"Parent").ok().and_then(|o| o.as_reference().ok()) {
            Some(parent) => id = parent,
            None => break,
        }
    }
    0
}

fn media_box(doc: &LopdfDocument, mut id: ObjectId) -> Result<(f32, f32)> {
    for _ in 0..16 {
        let obj = doc.get_object(id)?;
        let dict = obj
            .as_dict()
            .map_err(|e| Error::structure(e.to_string()))?;
        if let Ok(mb) = dict.get(b"MediaBox") {
            return parse_box(mb);
        }
        match dict.get(b"Parent").ok().and_then(|o| o.as_reference().ok()) {
            Some(parent) => id = parent,
            None => break,
        }
    }
    Err(Error::structure("page has no MediaBox"))
}

fn parse_box(obj: &Object) -> Result<(f32, f32)> {
    let arr = obj
        .as_array()
        .map_err(|e| Error::structure(e.to_string()))?;
    if arr.len() < 4 {
        return Err(Error::structure("MediaBox too short"));
    }
    let n = |i: usize| number(&arr[i]);
    let llx = n(0)?;
    let lly = n(1)?;
    let urx = n(2)?;
    let ury = n(3)?;
    Ok(((urx - llx).abs(), (ury - lly).abs()))
}

fn number(obj: &Object) -> Result<f32> {
    match obj {
        Object::Integer(i) => Ok(*i as f32),
        Object::Real(r) => Ok(*r as f32),
        _ => Err(Error::structure("expected number")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::corpus;

    #[test]
    fn open_hello_and_sizes() {
        let doc = Document::open(corpus("hello.pdf")).unwrap();
        assert_eq!(doc.page_count(), 1);
        let p = doc.page(0).unwrap();
        assert!((p.width_pt - 612.0).abs() < 0.1);
        assert!((p.height_pt - 792.0).abs() < 0.1);
        assert!(!doc.is_signed());
        assert!(!doc.is_read_only());
        doc.ensure_editable().unwrap();
    }

    #[test]
    fn outline_fixture() {
        let doc = Document::open(corpus("outline.pdf")).unwrap();
        let outline = doc.outline().unwrap();
        assert_eq!(outline.len(), 2);
        assert_eq!(outline[0].title, "Chapter 1");
        assert_eq!(outline[0].page, Some(0));
        assert_eq!(outline[1].title, "Chapter 2");
        assert_eq!(outline[1].page, Some(1));
    }

    #[test]
    fn rotate_undo_redo() {
        let mut doc = Document::open(corpus("hello.pdf")).unwrap();
        doc.rotate_page(0, 90).unwrap();
        assert_eq!(doc.page(0).unwrap().rotation, 90);
        assert!(doc.undo().unwrap());
        assert_eq!(doc.page(0).unwrap().rotation, 0);
        assert!(doc.redo().unwrap());
        assert_eq!(doc.page(0).unwrap().rotation, 90);
    }

    #[test]
    fn delete_and_move() {
        let mut doc = Document::open(corpus("two_page.pdf")).unwrap();
        assert_eq!(doc.page_count(), 2);
        doc.delete_page(0).unwrap();
        assert_eq!(doc.page_count(), 1);
        assert!(doc.undo().unwrap());
        assert_eq!(doc.page_count(), 2);
        doc.move_page(0, 2).unwrap();
        assert_eq!(doc.page_count(), 2);
    }
}
