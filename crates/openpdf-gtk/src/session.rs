// SPDX-License-Identifier: MPL-2.0
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use openpdf_core::{
    Command, Document, DocumentId, Engine, OutlineItem, Priority, SearchHit, SignRequest, Signer,
    Tile, TileKey, TileRequest,
};

use crate::backend::{BackendError, DocumentBackend, MockDocument, PageSize, TileBitmap};
use crate::tiles::{TileCache, TileKey as ViewTile, DEFAULT_CACHE_BYTES};

/// UI-thread document session. `Document` is not shared across threads;
/// tiles go through the process-wide [`Engine`] queue.
pub struct Session {
    engine: Engine,
    doc: RefCell<Document>,
    title: RefCell<String>,
    path: RefCell<Option<PathBuf>>,
}

/// `document.pdf` → `document-signed.pdf`. Opening an already-signed sibling
/// writes `document-signed-2.pdf` so the source file is never overwritten.
pub fn sibling_signed_path(src: &Path) -> PathBuf {
    let parent = src.parent().unwrap_or_else(|| Path::new("."));
    let raw_stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    let ext = src.extension().and_then(|s| s.to_str()).unwrap_or("pdf");
    let stem = raw_stem.strip_suffix("-signed").unwrap_or(raw_stem);
    let first = parent.join(format!("{stem}-signed.{ext}"));
    if first != src {
        return first;
    }
    for n in 2u32.. {
        let dest = parent.join(format!("{stem}-signed-{n}.{ext}"));
        if dest != src {
            return dest;
        }
    }
    first
}

impl Session {
    pub fn open(engine: &Engine, path: &Path) -> Result<Self, BackendError> {
        let doc = engine.open(path).map_err(core_err)?;
        let title = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("document.pdf")
            .to_string();
        Ok(Self {
            engine: engine.clone(),
            doc: RefCell::new(doc),
            title: RefCell::new(title),
            path: RefCell::new(Some(path.to_path_buf())),
        })
    }

    pub fn title(&self) -> String {
        self.title.borrow().clone()
    }

    pub fn path(&self) -> Option<PathBuf> {
        self.path.borrow().clone()
    }

    pub fn doc_id(&self) -> DocumentId {
        self.doc.borrow().id()
    }

    pub fn page_count(&self) -> usize {
        self.doc.borrow().page_count() as usize
    }

    pub fn page_size(&self, page: u32) -> Result<PageSize, BackendError> {
        let info = self.doc.borrow().page(page).map_err(core_err)?;
        Ok(PageSize {
            width: f64::from(info.width_pt),
            height: f64::from(info.height_pt),
        })
    }

    pub fn pages(&self) -> Vec<PageSize> {
        (0..self.page_count())
            .filter_map(|i| self.page_size(i as u32).ok())
            .collect()
    }

    pub fn is_locked(&self) -> bool {
        let doc = self.doc.borrow();
        doc.is_signed() || doc.is_read_only()
    }

    pub fn is_dirty(&self) -> bool {
        self.doc.borrow().is_dirty()
    }

    pub fn can_undo(&self) -> bool {
        !self.is_locked() && self.doc.borrow().can_undo()
    }

    pub fn can_redo(&self) -> bool {
        !self.is_locked() && self.doc.borrow().can_redo()
    }

    pub fn outline(&self) -> Vec<OutlineItem> {
        self.doc.borrow().outline().unwrap_or_default()
    }

    pub fn rotate(&self, page: u32, delta: i32) -> Result<(), BackendError> {
        self.ensure_writable()?;
        let mut doc = self.doc.borrow_mut();
        doc.rotate_page(page, delta).map_err(core_err)?;
        self.engine.refresh(&doc).map_err(core_err)
    }

    pub fn delete_page(&self, page: u32) -> Result<(), BackendError> {
        self.ensure_writable()?;
        let mut doc = self.doc.borrow_mut();
        doc.delete_page(page).map_err(core_err)?;
        self.engine.refresh(&doc).map_err(core_err)
    }

    pub fn move_page(&self, from: u32, to: u32) -> Result<(), BackendError> {
        self.ensure_writable()?;
        let mut doc = self.doc.borrow_mut();
        doc.move_page(from, to).map_err(core_err)?;
        self.engine.refresh(&doc).map_err(core_err)
    }

    pub fn undo(&self) -> Result<bool, BackendError> {
        self.ensure_writable()?;
        let mut doc = self.doc.borrow_mut();
        let changed = doc.undo().map_err(core_err)?;
        if changed {
            self.engine.refresh(&doc).map_err(core_err)?;
        }
        Ok(changed)
    }

    pub fn redo(&self) -> Result<bool, BackendError> {
        self.ensure_writable()?;
        let mut doc = self.doc.borrow_mut();
        let changed = doc.redo().map_err(core_err)?;
        if changed {
            self.engine.refresh(&doc).map_err(core_err)?;
        }
        Ok(changed)
    }

    pub fn execute(&self, command: Command) -> Result<(), BackendError> {
        self.ensure_writable()?;
        let mut doc = self.doc.borrow_mut();
        doc.execute(command).map_err(core_err)?;
        self.engine.refresh(&doc).map_err(core_err)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, BackendError> {
        self.doc.borrow().to_bytes().map_err(core_err)
    }

    /// Bytes handed to [`Signer::sign`] on a worker thread. Matches
    /// [`Document::apply_signer`]: incremental for an already-signed file.
    pub fn bytes_for_sign(&self) -> Result<Vec<u8>, BackendError> {
        let doc = self.doc.borrow();
        if doc.is_signed() {
            doc.to_incremental_bytes().map_err(core_err)
        } else {
            doc.to_bytes().map_err(core_err)
        }
    }

    /// Same contract as [`openpdf_core::Document::original_bytes`] once Jian
    /// makes that method `pub` on #25. Until then, read the file that was
    /// opened. Never [`Document::to_bytes`] — that is `Signed(Rewrite)`.
    pub fn original_bytes(&self) -> Result<Vec<u8>, BackendError> {
        if let Some(path) = self.path.borrow().as_ref() {
            if path.exists() {
                return std::fs::read(path).map_err(BackendError::from);
            }
        }
        // When `Document::original_bytes` is `pub`:
        //   return Ok(self.doc.borrow().original_bytes().to_vec());
        Err(BackendError::Invalid(openpdf_core::tr(
            "Cannot verify: original file bytes are unavailable.",
        )))
    }

    pub fn bytes_for_verify(&self) -> Result<Vec<u8>, BackendError> {
        self.original_bytes()
    }

    pub fn apply_signer(
        &self,
        signer: &dyn Signer,
        request: &SignRequest,
    ) -> Result<(), BackendError> {
        let old_id = self.doc.borrow().id();
        let mut doc = self.doc.borrow_mut();
        doc.apply_signer(signer, request).map_err(core_err)?;
        self.engine.bind(&doc).map_err(core_err)?;
        let _ = self.engine.close(old_id);
        Ok(())
    }

    pub fn apply_signed_bytes(&self, signed: Vec<u8>) -> Result<(), BackendError> {
        let old_id = self.doc.borrow().id();
        let path = self.path.borrow().clone();
        let doc = Document::from_bytes(signed, path).map_err(core_err)?;
        self.engine.bind(&doc).map_err(core_err)?;
        let _ = self.engine.close(old_id);
        *self.doc.borrow_mut() = doc;
        Ok(())
    }

    /// Write the signed bytes next to the source (or into `$TMP`), then reopen
    /// that file so verify reads the on-disk original, not `to_bytes()`.
    pub fn persist_signed(&self, signed: Vec<u8>) -> Result<PathBuf, BackendError> {
        let dest = match self.path.borrow().as_ref() {
            Some(p) => sibling_signed_path(p),
            None => {
                std::env::temp_dir().join(format!("document-signed-{}.pdf", std::process::id()))
            }
        };
        std::fs::write(&dest, &signed)?;
        self.reopen(&dest)?;
        Ok(dest)
    }

    pub fn reopen(&self, path: &Path) -> Result<(), BackendError> {
        let old_id = self.doc.borrow().id();
        let doc = self.engine.open(path).map_err(core_err)?;
        self.engine.bind(&doc).map_err(core_err)?;
        let _ = self.engine.close(old_id);
        *self.doc.borrow_mut() = doc;
        *self.path.borrow_mut() = Some(path.to_path_buf());
        *self.title.borrow_mut() = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("document.pdf")
            .to_string();
        Ok(())
    }

    pub fn begin_frame(&self) {
        let _ = self.engine.begin_frame();
    }

    pub fn request_view_tiles(&self, keys: &[ViewTile], priority: Priority) {
        if keys.is_empty() {
            return;
        }
        crate::measure::mark_first_tile_requested();
        let id = self.doc_id();
        let to_req = |k: &ViewTile| TileRequest {
            key: TileKey::new(id, k.page, k.zoom() as f32, k.tx, k.ty),
            priority,
        };
        // Top-left (first key) goes to the worker before the rest of the viewport.
        let _ = self.engine.request_tiles(std::iter::once(to_req(&keys[0])));
        if keys.len() > 1 {
            let _ = self.engine.request_tiles(keys[1..].iter().map(to_req));
        }
    }

    pub fn cancel_except(&self, keep: &[ViewTile]) {
        let id = self.doc_id();
        let keep: Vec<TileKey> = keep
            .iter()
            .map(|k| TileKey::new(id, k.page, k.zoom() as f32, k.tx, k.ty))
            .collect();
        let _ = self.engine.cancel_except(id, keep);
    }

    pub fn cache_get(&self, key: &ViewTile) -> Option<Arc<Tile>> {
        let core = TileKey::new(self.doc_id(), key.page, key.zoom() as f32, key.tx, key.ty);
        self.engine.cache_get(&core)
    }

    pub fn poll(&self) -> bool {
        let mut dirty = false;
        while self.poll_one() {
            dirty = true;
        }
        dirty
    }

    /// Take at most one finished tile so the UI can `queue_draw` per arrival.
    pub fn poll_one(&self) -> bool {
        if self.engine.try_recv().is_some() {
            crate::measure::mark_first_tile_rendered();
            true
        } else {
            false
        }
    }

    pub fn page_text(&self, page: u32) -> Result<String, BackendError> {
        self.engine.page_text(self.doc_id(), page).map_err(core_err)
    }

    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>, BackendError> {
        self.engine.search(self.doc_id(), query).map_err(core_err)
    }

    fn ensure_writable(&self) -> Result<(), BackendError> {
        self.doc.borrow().ensure_editable().map_err(core_err)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let id = self.doc.borrow().id();
        let _ = self.engine.close(id);
    }
}

fn core_err(e: openpdf_core::Error) -> BackendError {
    BackendError::Invalid(crate::i18n::format_core_error(&e))
}

/// Sync placeholder used when PDFium is missing (unit tests and the sample
/// fallback). `--measure` requires a real [`Engine`] and fails otherwise.
pub struct MockView {
    doc: Arc<MockDocument>,
    cache: Mutex<TileCache>,
    locked: Cell<bool>,
}

impl MockView {
    pub fn new(doc: Arc<MockDocument>) -> Self {
        Self {
            doc,
            cache: Mutex::new(TileCache::new(DEFAULT_CACHE_BYTES)),
            locked: Cell::new(false),
        }
    }

    pub fn title(&self) -> &str {
        self.doc.title()
    }

    pub fn path(&self) -> Option<&Path> {
        self.doc.path()
    }

    pub fn page_count(&self) -> usize {
        self.doc.page_count()
    }

    pub fn page_size(&self, page: u32) -> Result<PageSize, BackendError> {
        self.doc.page_size(page)
    }

    pub fn pages(&self) -> Vec<PageSize> {
        (0..self.page_count())
            .filter_map(|i| self.page_size(i as u32).ok())
            .collect()
    }

    pub fn is_locked(&self) -> bool {
        self.locked.get()
    }

    pub fn lock_after_sign(&self) {
        self.locked.set(true);
    }

    pub fn request_view_tiles(&self, keys: &[ViewTile], _priority: Priority) {
        let Ok(mut cache) = self.cache.lock() else {
            return;
        };
        for key in keys {
            if cache.get(key).is_some() {
                continue;
            }
            if let Ok(tile) = self.doc.render_tile(crate::backend::TileRequest {
                page: key.page,
                zoom: key.zoom(),
                tx: key.tx,
                ty: key.ty,
                tile_size: key.tile_size,
            }) {
                cache.insert(*key, Arc::new(tile));
            }
        }
    }

    pub fn cache_bitmap(&self, key: &ViewTile) -> Option<Arc<TileBitmap>> {
        self.cache.lock().ok().and_then(|mut c| c.get(key))
    }

    pub fn extract_text(&self, page: u32) -> Result<String, BackendError> {
        self.doc.extract_text(page)
    }
}

pub enum Opened {
    Core(Session),
    Mock(MockView),
}

impl Opened {
    pub fn title(&self) -> String {
        match self {
            Self::Core(s) => s.title().to_string(),
            Self::Mock(s) => s.title().to_string(),
        }
    }

    pub fn path(&self) -> Option<PathBuf> {
        match self {
            Self::Core(s) => s.path(),
            Self::Mock(s) => s.path().map(|p| p.to_path_buf()),
        }
    }

    pub fn page_count(&self) -> usize {
        match self {
            Self::Core(s) => s.page_count(),
            Self::Mock(s) => s.page_count(),
        }
    }

    pub fn pages(&self) -> Vec<PageSize> {
        match self {
            Self::Core(s) => s.pages(),
            Self::Mock(s) => s.pages(),
        }
    }

    pub fn is_placeholder(&self) -> bool {
        matches!(self, Self::Mock(_))
    }

    pub fn is_locked(&self) -> bool {
        match self {
            Self::Core(s) => s.is_locked(),
            Self::Mock(s) => s.is_locked(),
        }
    }

    pub fn lock_after_sign(&self) {
        if let Self::Mock(s) = self {
            s.lock_after_sign();
        }
    }

    pub fn outline(&self) -> Vec<OutlineItem> {
        match self {
            Self::Core(s) => s.outline(),
            Self::Mock(_) => Vec::new(),
        }
    }

    pub fn begin_frame(&self) {
        if let Self::Core(s) = self {
            s.begin_frame();
        }
    }

    pub fn request_tiles(&self, keys: &[ViewTile], priority: Priority) {
        match self {
            Self::Core(s) => s.request_view_tiles(keys, priority),
            Self::Mock(s) => s.request_view_tiles(keys, priority),
        }
    }

    pub fn cancel_except(&self, keep: &[ViewTile]) {
        if let Self::Core(s) = self {
            s.cancel_except(keep);
        }
    }

    pub fn poll(&self) -> bool {
        match self {
            Self::Core(s) => s.poll(),
            Self::Mock(_) => false,
        }
    }

    pub fn poll_one(&self) -> bool {
        match self {
            Self::Core(s) => s.poll_one(),
            Self::Mock(_) => false,
        }
    }

    pub fn paint_tile(&self, key: &ViewTile) -> Option<PaintTile> {
        match self {
            Self::Core(s) => s.cache_get(key).map(|t| PaintTile {
                width: t.width,
                height: t.height,
                rgba: t.rgba.clone(),
            }),
            Self::Mock(s) => s.cache_bitmap(key).map(|t| PaintTile {
                width: t.width,
                height: t.height,
                rgba: t.pixels.clone(),
            }),
        }
    }

    pub fn extract_text(&self, page: u32) -> Result<String, BackendError> {
        match self {
            Self::Core(s) => s.page_text(page),
            Self::Mock(s) => s.extract_text(page),
        }
    }

    pub fn search_pages(&self, query: &str) -> Result<Vec<u32>, BackendError> {
        match self {
            Self::Core(s) => Ok(s
                .search(query)?
                .into_iter()
                .map(|h: SearchHit| h.page)
                .collect()),
            Self::Mock(s) => {
                let mut pages = Vec::new();
                let q = query.to_lowercase();
                for i in 0..s.page_count() {
                    if let Ok(text) = s.extract_text(i as u32) {
                        if text.to_lowercase().contains(&q) {
                            pages.push(i as u32);
                        }
                    }
                }
                Ok(pages)
            }
        }
    }

    pub fn rotate(&self, page: u32, delta: i32) -> Result<(), BackendError> {
        match self {
            Self::Core(s) => s.rotate(page, delta),
            Self::Mock(_) => Err(BackendError::Invalid(openpdf_core::tr(
                "Page edits need a real document.",
            ))),
        }
    }

    pub fn delete_page(&self, page: u32) -> Result<(), BackendError> {
        match self {
            Self::Core(s) => s.delete_page(page),
            Self::Mock(_) => Err(BackendError::Invalid(openpdf_core::tr(
                "Page edits need a real document.",
            ))),
        }
    }

    pub fn undo(&self) -> Result<bool, BackendError> {
        match self {
            Self::Core(s) => s.undo(),
            Self::Mock(_) => Ok(false),
        }
    }

    pub fn redo(&self) -> Result<bool, BackendError> {
        match self {
            Self::Core(s) => s.redo(),
            Self::Mock(_) => Ok(false),
        }
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, BackendError> {
        match self {
            Self::Core(s) => s.to_bytes(),
            Self::Mock(_) => Err(BackendError::Invalid(openpdf_core::tr(
                "Placeholder has no PDF bytes.",
            ))),
        }
    }

    pub fn bytes_for_sign(&self) -> Result<Vec<u8>, BackendError> {
        match self {
            Self::Core(s) => s.bytes_for_sign(),
            Self::Mock(_) => Err(BackendError::Invalid(openpdf_core::tr(
                "Placeholder has no PDF bytes.",
            ))),
        }
    }

    /// Prepared for [`openpdf_core::Document::original_bytes`] (`pub` on #25).
    /// Until that lands, this reads the opened path and errors if it cannot.
    pub fn original_bytes(&self) -> Result<Vec<u8>, BackendError> {
        match self {
            Self::Core(s) => s.original_bytes(),
            Self::Mock(_) => Err(BackendError::Invalid(openpdf_core::tr(
                "Placeholder has no PDF bytes.",
            ))),
        }
    }

    pub fn bytes_for_verify(&self) -> Result<Vec<u8>, BackendError> {
        self.original_bytes()
    }

    pub fn apply_signer(
        &self,
        signer: &dyn Signer,
        request: &SignRequest,
    ) -> Result<(), BackendError> {
        match self {
            Self::Core(s) => s.apply_signer(signer, request),
            Self::Mock(s) => {
                let _ = (signer, request);
                s.lock_after_sign();
                Ok(())
            }
        }
    }

    pub fn apply_signed_bytes(&self, signed: Vec<u8>) -> Result<(), BackendError> {
        match self {
            Self::Core(s) => s.apply_signed_bytes(signed),
            Self::Mock(s) => {
                s.lock_after_sign();
                let _ = signed;
                Ok(())
            }
        }
    }

    pub fn persist_signed(&self, signed: Vec<u8>) -> Result<PathBuf, BackendError> {
        match self {
            Self::Core(s) => s.persist_signed(signed),
            Self::Mock(s) => {
                s.lock_after_sign();
                let dest = s.path().map(sibling_signed_path).unwrap_or_else(|| {
                    std::env::temp_dir().join(format!("document-signed-{}.pdf", std::process::id()))
                });
                std::fs::write(&dest, signed)?;
                Ok(dest)
            }
        }
    }
}

pub struct PaintTile {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn sample_pdf_path() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/two_page.pdf"),
        PathBuf::from("corpus/two_page.pdf"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/hello.pdf"),
        PathBuf::from("corpus/hello.pdf"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_opened_renders_and_searches() {
        let opened = Opened::Mock(MockView::new(MockDocument::sample(4)));
        assert_eq!(opened.page_count(), 4);
        assert!(!opened.is_locked());
        let key = ViewTile::new(0, 1.0, 0, 0, 256);
        opened.request_tiles(&[key], Priority::Visible);
        assert!(opened.paint_tile(&key).is_some());
        let hits = opened.search_pages("Page 2").unwrap();
        assert_eq!(hits, vec![1]);
        assert!(opened.rotate(0, 90).is_err());
    }

    #[test]
    fn engine_renders_measure_document_thumbs() {
        let Ok(engine) = Engine::new() else {
            return;
        };
        let path = crate::measure::write_measure_pdf(4).unwrap();
        let session = Session::open(&engine, &path).unwrap();
        assert!(session.page_count() >= 4);
        let zoom = 132.0 / 595.0;
        let keys: Vec<ViewTile> = (0..3)
            .map(|page| ViewTile::new(page, zoom, 0, 0, 256))
            .collect();
        session.request_view_tiles(&keys, Priority::Thumbnail);
        let t0 = std::time::Instant::now();
        let mut ready = 0usize;
        while t0.elapsed().as_secs() < 5 {
            let _ = session.poll();
            ready = keys
                .iter()
                .filter(|k| session.cache_get(k).is_some())
                .count();
            if ready == 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = std::fs::remove_file(path);
        assert_eq!(ready, 3, "expected 3 thumbnail tiles in cache");
    }

    #[test]
    fn core_session_lock_follows_document_is_signed() {
        let Ok(engine) = Engine::new() else {
            return;
        };
        let Some(path) = sample_pdf_path() else {
            return;
        };
        let session = Session::open(&engine, &path).unwrap();
        assert!(!session.doc.borrow().is_signed());
        assert!(!session.doc.borrow().is_read_only());
        assert!(!session.is_locked());
        session.doc.borrow().ensure_editable().unwrap();
    }

    /// GTK placement (`tx * tile`, `ty * tile`) vs a single full-page render
    /// at 147% zoom. Core `tile_rect` is not modified; this only checks our
    /// offsets against `Engine::render_page`.
    #[test]
    fn tiled_placement_matches_full_render_at_147_percent() {
        let Ok(engine) = Engine::new() else {
            return;
        };
        let path = match crate::measure::write_measure_pdf(2) {
            Ok(p) => p,
            Err(_) => return,
        };
        let session = Session::open(&engine, &path).unwrap();
        let id = session.doc_id();
        let zoom = 1.47f32;
        let full = match engine.render_page(id, 0, zoom) {
            Ok(r) => r,
            Err(_) => {
                let _ = std::fs::remove_file(&path);
                return;
            }
        };
        let tile = crate::tiles::DEFAULT_TILE_SIZE;
        let (cols, rows) = crate::tiles::tile_grid(full.width, full.height, tile);
        assert!(
            cols >= 2 && rows >= 2,
            "147% A4-ish page must span several tiles, got {cols}x{rows} of {}x{}",
            full.width,
            full.height
        );
        let mut composite = vec![0u8; full.width as usize * full.height as usize * 4];
        for ty in 0..rows {
            for tx in 0..cols {
                let key = openpdf_core::TileKey::new(id, 0, zoom, tx, ty);
                let rendered = engine
                    .render_tile_blocking(key)
                    .expect("tile render at 147%");
                crate::tiles::place_tile_rgba(
                    &mut composite,
                    full.width,
                    full.height,
                    &rendered.rgba,
                    rendered.width,
                    rendered.height,
                    tx,
                    ty,
                    tile,
                );
            }
        }
        let _ = std::fs::remove_file(&path);
        let mismatch = crate::tiles::pixel_mismatch_ratio(&composite, &full.rgba, 12);
        assert!(
            mismatch < 0.01,
            "tiled composite at 147% diverges from full render ({:.2}% pixels, max Δ {})",
            mismatch * 100.0,
            crate::tiles::max_channel_delta(&composite, &full.rgba)
        );
    }

    #[test]
    fn sibling_signed_path_keeps_the_source() {
        let src = PathBuf::from("/tmp/report.pdf");
        assert_eq!(
            sibling_signed_path(&src),
            PathBuf::from("/tmp/report-signed.pdf")
        );
        let already = PathBuf::from("/tmp/report-signed.pdf");
        assert_eq!(
            sibling_signed_path(&already),
            PathBuf::from("/tmp/report-signed-2.pdf")
        );
    }

    #[test]
    fn persist_signed_reopen_reads_the_written_file_for_verify() {
        let Ok(engine) = Engine::new() else {
            return;
        };
        let hello = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/hello.pdf");
        let other = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/two_page.pdf");
        if !hello.exists() || !other.exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "openpdf-persist-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.pdf");
        std::fs::copy(&hello, &src).unwrap();
        let session = Session::open(&engine, &src).unwrap();
        let written = std::fs::read(&other).unwrap();
        let dest = session
            .persist_signed(written.clone())
            .expect("write sibling and reopen");
        assert_eq!(dest, dir.join("src-signed.pdf"));
        assert_eq!(session.path(), Some(dest.clone()));
        let verify = session.original_bytes().expect("verify original bytes");
        assert_eq!(verify, written, "verify must read the written sibling");
        assert_ne!(
            verify,
            std::fs::read(&src).unwrap(),
            "verify must not reread the unsigned source"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
