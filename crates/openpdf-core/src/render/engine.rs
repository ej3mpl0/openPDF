use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use pdfium_render::prelude::{PdfDocument, Pdfium};

use crate::document::{Document, DocumentId};
use crate::error::{Error, InvalidKind, Result};

use super::cache::TileCache;
use super::pdfium::{self as pdfium_bind};
use super::queue::RenderQueue;
use super::{Raster, SearchHit, Tile, TileEvent, TileKey, TileRequest, DEFAULT_CACHE_BYTES};

type EngineId = u64;

static NEXT_ENGINE_ID: AtomicU64 = AtomicU64::new(1);

/// Per-engine queue and event channel on the shared render thread, so
/// engines never see or cancel each other's tiles.
struct EngineSlot {
    queue: RenderQueue,
    events: Sender<TileEvent>,
    /// Documents this engine bound; released on `Unregister`.
    docs: HashSet<DocumentId>,
}

enum WorkerCmd {
    Bind {
        engine: EngineId,
        id: DocumentId,
        bytes: Arc<Vec<u8>>,
        reply: Sender<Result<()>>,
    },
    /// Test-only: address of the buffer PDFium loaded `id` from.
    #[cfg(test)]
    LoadedPtr {
        id: DocumentId,
        reply: Sender<Option<usize>>,
    },
    Unbind {
        engine: EngineId,
        id: DocumentId,
    },
    #[cfg(test)]
    Probe {
        engine: EngineId,
        doc: DocumentId,
        reply: Sender<(bool, bool)>,
    },
    Register {
        engine: EngineId,
        events: Sender<TileEvent>,
    },
    Unregister {
        engine: EngineId,
    },
    Request {
        engine: EngineId,
        reqs: Vec<TileRequest>,
    },
    Cancel {
        engine: EngineId,
        keys: Vec<TileKey>,
    },
    CancelExcept {
        engine: EngineId,
        doc: DocumentId,
        keep: Vec<TileKey>,
    },
    Bump {
        engine: EngineId,
    },
    Text {
        id: DocumentId,
        page: u32,
        reply: Sender<Result<String>>,
    },
    RenderPage {
        id: DocumentId,
        page: u32,
        zoom: f32,
        reply: Sender<Result<Raster>>,
    },
    RenderTile {
        key: TileKey,
        reply: Sender<Result<Tile>>,
    },
    Search {
        id: DocumentId,
        query: String,
        reply: Sender<Result<Vec<SearchHit>>>,
    },
}

struct WorkerHandle {
    cmd: Sender<WorkerCmd>,
    cache: Arc<Mutex<TileCache>>,
    /// dlopen + FPDF_InitLibrary only (not worker spawn).
    pdfium_load: Duration,
}

static WORKER: OnceLock<std::result::Result<WorkerHandle, String>> = OnceLock::new();

fn worker() -> Result<&'static WorkerHandle> {
    match WORKER.get_or_init(|| start_worker(None).map_err(|e| e.to_string())) {
        Ok(h) => Ok(h),
        Err(e) => Err(Error::pdfium(e.clone())),
    }
}

fn start_worker(bundled: Option<&Path>) -> Result<WorkerHandle> {
    let t_load = Instant::now();
    let pdfium = pdfium_bind::bind_pdfium(bundled)?;
    let pdfium_load = t_load.elapsed();
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let cache = Arc::new(Mutex::new(TileCache::new(DEFAULT_CACHE_BYTES)));
    let cache_thread = Arc::clone(&cache);

    thread::Builder::new()
        .name("openpdf-render".into())
        .spawn(move || worker_loop(pdfium, cmd_rx, cache_thread))
        .map_err(|e| Error::pdfium(e.to_string()))?;

    Ok(WorkerHandle {
        cmd: cmd_tx,
        cache,
        pdfium_load,
    })
}

self_cell::self_cell!(
    /// A PDFium document reading the shared bytes in place (no copy).
    ///
    /// Owner: the same `Arc<Vec<u8>>` as `Document::original_bytes()` (or a
    /// fresh rewrite for dirty documents). Dependent: the PDFium document
    /// borrowing that buffer. `self_cell` keeps the owner alive and drops the
    /// dependent first.
    struct BoundDoc {
        owner: Arc<Vec<u8>>,

        #[not_covariant]
        dependent: BoundPdf,
    }
);

type BoundPdf<'a> = PdfDocument<'a>;

impl BoundDoc {
    fn with<R>(&self, f: impl FnOnce(&PdfDocument<'_>) -> R) -> R {
        self.with_dependent(|_, doc| f(doc))
    }
}

fn load_bound(pdfium: &'static Pdfium, bytes: Arc<Vec<u8>>) -> Result<BoundDoc> {
    BoundDoc::try_new(bytes, |bytes| {
        pdfium
            .load_pdf_from_byte_slice(bytes.as_slice(), None)
            .map_err(|e| Error::pdfium(e.to_string()))
    })
}

fn worker_loop(pdfium: Pdfium, cmd_rx: Receiver<WorkerCmd>, cache: Arc<Mutex<TileCache>>) {
    // The worker lives for the whole process (global OnceLock), so PDFium is
    // leaked to get a 'static borrow for the documents; no lifetime casts.
    let pdfium: &'static Pdfium = Box::leak(Box::new(pdfium));
    let mut docs: HashMap<DocumentId, BoundDoc> = HashMap::new();
    let mut engines: HashMap<EngineId, EngineSlot> = HashMap::new();
    let mut order: Vec<EngineId> = Vec::new();
    let mut next = 0usize;

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            if !handle_cmd(pdfium, cmd, &mut docs, &mut engines, &cache) {
                return;
            }
        }

        order.clear();
        order.extend(engines.keys().copied());
        order.sort_unstable();
        let mut job = None;
        for i in 0..order.len() {
            let id = order[(next + i) % order.len()];
            if let Some(slot) = engines.get_mut(&id) {
                if let Some(popped) = slot.queue.pop() {
                    next = (next + i + 1) % order.len();
                    job = Some((id, popped));
                    break;
                }
            }
        }

        if let Some((engine, (key, _pri, gen))) = job {
            let cancelled = |engines: &HashMap<EngineId, EngineSlot>| {
                engines
                    .get(&engine)
                    .is_none_or(|slot| slot.queue.is_cancelled(&key, gen))
            };
            let send = |engines: &HashMap<EngineId, EngineSlot>, ev: TileEvent| {
                if let Some(slot) = engines.get(&engine) {
                    let _ = slot.events.send(ev);
                }
            };
            if cancelled(&engines) {
                send(&engines, TileEvent::Cancelled(key));
                continue;
            }
            if let Some(hit) = cache.lock().ok().and_then(|mut c| c.get(&key)) {
                send(&engines, TileEvent::Ready(hit));
                continue;
            }
            let result = match docs.get(&key.doc) {
                Some(bound) => bound
                    .with(|doc| {
                        pdfium_bind::render_tile(
                            doc,
                            key.page,
                            key.zoom(),
                            key.x,
                            key.y,
                            super::TILE_SIZE,
                        )
                    })
                    .map(|(w, h, rgba)| Tile {
                        key,
                        width: w,
                        height: h,
                        rgba,
                    }),
                None => Err(Error::invalid(InvalidKind::DocumentNotBound)),
            };
            // Commands that arrived while rendering (cancel, new frame) apply first.
            while let Ok(cmd) = cmd_rx.try_recv() {
                if !handle_cmd(pdfium, cmd, &mut docs, &mut engines, &cache) {
                    return;
                }
            }
            if cancelled(&engines) {
                send(&engines, TileEvent::Cancelled(key));
                continue;
            }
            match result {
                Ok(tile) => {
                    let tile = Arc::new(tile);
                    if let Ok(mut c) = cache.lock() {
                        c.insert(Arc::clone(&tile));
                    }
                    send(&engines, TileEvent::Ready(tile));
                }
                Err(e) => send(
                    &engines,
                    TileEvent::Failed {
                        key,
                        message: e.to_string(),
                    },
                ),
            }
            continue;
        }

        match cmd_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(cmd) => {
                if !handle_cmd(pdfium, cmd, &mut docs, &mut engines, &cache) {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Drop a bound document unless another live engine still has it bound.
fn release_doc(
    id: DocumentId,
    docs: &mut HashMap<DocumentId, BoundDoc>,
    engines: &HashMap<EngineId, EngineSlot>,
    cache: &Arc<Mutex<TileCache>>,
) {
    if engines.values().any(|slot| slot.docs.contains(&id)) {
        return;
    }
    docs.remove(&id);
    if let Ok(mut c) = cache.lock() {
        c.invalidate_doc(id);
    }
}

fn handle_cmd(
    pdfium: &'static Pdfium,
    cmd: WorkerCmd,
    docs: &mut HashMap<DocumentId, BoundDoc>,
    engines: &mut HashMap<EngineId, EngineSlot>,
    cache: &Arc<Mutex<TileCache>>,
) -> bool {
    match cmd {
        #[cfg(test)]
        WorkerCmd::LoadedPtr { id, reply } => {
            let _ = reply.send(docs.get(&id).map(|b| b.borrow_owner().as_ptr() as usize));
        }
        WorkerCmd::Bind {
            engine,
            id,
            bytes,
            reply,
        } => {
            let result = load_bound(pdfium, bytes).map(|bound| {
                docs.insert(id, bound);
                if let Some(slot) = engines.get_mut(&engine) {
                    slot.docs.insert(id);
                }
                if let Ok(mut c) = cache.lock() {
                    c.invalidate_doc(id);
                }
            });
            let _ = reply.send(result);
        }
        WorkerCmd::Unbind { engine, id } => {
            if let Some(slot) = engines.get_mut(&engine) {
                slot.docs.remove(&id);
            }
            release_doc(id, docs, engines, cache);
        }
        #[cfg(test)]
        WorkerCmd::Probe { engine, doc, reply } => {
            let _ = reply.send((engines.contains_key(&engine), docs.contains_key(&doc)));
        }
        WorkerCmd::Register { engine, events } => {
            engines.insert(
                engine,
                EngineSlot {
                    queue: RenderQueue::new(),
                    events,
                    docs: HashSet::new(),
                },
            );
        }
        WorkerCmd::Unregister { engine } => {
            if let Some(slot) = engines.remove(&engine) {
                for id in slot.docs {
                    release_doc(id, docs, engines, cache);
                }
            }
        }
        WorkerCmd::Request { engine, reqs } => {
            if let Some(slot) = engines.get_mut(&engine) {
                for req in reqs {
                    if let Some(hit) = cache.lock().ok().and_then(|mut c| c.get(&req.key)) {
                        let _ = slot.events.send(TileEvent::Ready(hit));
                        continue;
                    }
                    slot.queue.push(req.key, req.priority);
                }
            }
        }
        WorkerCmd::Cancel { engine, keys } => {
            if let Some(slot) = engines.get_mut(&engine) {
                for key in keys {
                    slot.queue.cancel(key);
                }
            }
        }
        WorkerCmd::CancelExcept { engine, doc, keep } => {
            if let Some(slot) = engines.get_mut(&engine) {
                let keep: Vec<TileKey> = keep.into_iter().filter(|k| k.doc == doc).collect();
                slot.queue.cancel_except(&keep);
            }
        }
        WorkerCmd::Bump { engine } => {
            if let Some(slot) = engines.get_mut(&engine) {
                slot.queue.bump_generation();
            }
        }
        WorkerCmd::Text { id, page, reply } => {
            let result = match docs.get(&id) {
                Some(b) => b.with(|doc| pdfium_bind::page_text(doc, page)),
                None => Err(Error::invalid(InvalidKind::DocumentNotBound)),
            };
            let _ = reply.send(result);
        }
        WorkerCmd::RenderPage {
            id,
            page,
            zoom,
            reply,
        } => {
            let result = match docs.get(&id) {
                Some(b) => {
                    b.with(|doc| pdfium_bind::render_page(doc, page, zoom))
                        .map(|(w, h, rgba)| Raster {
                            width: w,
                            height: h,
                            rgba,
                        })
                }
                None => Err(Error::invalid(InvalidKind::DocumentNotBound)),
            };
            let _ = reply.send(result);
        }
        WorkerCmd::RenderTile { key, reply } => {
            if let Some(hit) = cache.lock().ok().and_then(|mut c| c.get(&key)) {
                let _ = reply.send(Ok((*hit).clone()));
                return true;
            }
            let result = match docs.get(&key.doc) {
                Some(b) => b
                    .with(|doc| {
                        pdfium_bind::render_tile(
                            doc,
                            key.page,
                            key.zoom(),
                            key.x,
                            key.y,
                            super::TILE_SIZE,
                        )
                    })
                    .map(|(w, h, rgba)| Tile {
                        key,
                        width: w,
                        height: h,
                        rgba,
                    }),
                None => Err(Error::invalid(InvalidKind::DocumentNotBound)),
            };
            if let Ok(ref tile) = result {
                if let Ok(mut c) = cache.lock() {
                    c.insert(Arc::new(tile.clone()));
                }
            }
            let _ = reply.send(result);
        }
        WorkerCmd::Search { id, query, reply } => {
            let result = match docs.get(&id) {
                Some(b) => b.with(|doc| search_doc(doc, &query)),
                None => Err(Error::invalid(InvalidKind::DocumentNotBound)),
            };
            let _ = reply.send(result);
        }
    }
    true
}

fn search_doc(doc: &PdfDocument<'_>, query: &str) -> Result<Vec<SearchHit>> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let needle = query.to_lowercase();
    let mut hits = Vec::new();
    let n = doc.pages().len() as u32;
    for page in 0..n {
        let text = pdfium_bind::page_text(doc, page)?;
        let lower = text.to_lowercase();
        let mut from = 0;
        while let Some(rel) = lower[from..].find(&needle) {
            let start = from + rel;
            let end = start + needle.len();
            let ctx_lo = start.saturating_sub(24);
            let ctx_hi = (end + 24).min(text.len());
            hits.push(SearchHit {
                page,
                start,
                end,
                context: text[ctx_lo..ctx_hi].to_string(),
            });
            from = end;
            if from >= lower.len() {
                break;
            }
        }
    }
    Ok(hits)
}

/// Process-wide PDFium handle plus the single render thread.
///
/// Each engine (and its clones) has its own tile queue and event channel on
/// the shared render thread.
#[derive(Clone, Debug)]
pub struct Engine {
    inner: Arc<EngineInner>,
}

/// Shared by all clones of an [`Engine`]; unregisters exactly once when the
/// last clone is gone.
#[derive(Debug)]
struct EngineInner {
    id: EngineId,
    events: Mutex<Receiver<TileEvent>>,
}

impl Drop for EngineInner {
    fn drop(&mut self) {
        if let Ok(h) = worker() {
            let _ = h.cmd.send(WorkerCmd::Unregister { engine: self.id });
        }
    }
}

impl Engine {
    /// Bind PDFium from absolute paths only (`OPENPDF_PDFIUM`, next to the exe, system dirs).
    pub fn new() -> Result<Self> {
        worker()?;
        Self::register()
    }

    /// Milliseconds for dlopen + `FPDF_InitLibrary` on the shared worker.
    /// `-1.0` if PDFium is unavailable.
    pub fn pdfium_load_ms(&self) -> f64 {
        match worker() {
            Ok(h) => h.pdfium_load.as_secs_f64() * 1000.0,
            Err(_) => -1.0,
        }
    }

    /// Last tile/page buffer copy after PDFium render. See `pdfium::last_convert_ms`.
    pub fn last_convert_ms(&self) -> f64 {
        pdfium_bind::last_convert_ms()
    }

    /// First successful call wins; later engines share the same worker.
    pub fn with_bundled_library(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        match WORKER.get_or_init(|| start_worker(Some(&path)).map_err(|e| e.to_string())) {
            Ok(_) => Self::register(),
            Err(e) => Err(Error::pdfium(e.clone())),
        }
    }

    pub fn open(&self, path: impl AsRef<Path>) -> Result<Document> {
        let doc = Document::open(path)?;
        self.bind(&doc)?;
        Ok(doc)
    }

    pub fn open_bytes(&self, bytes: Vec<u8>, path: Option<std::path::PathBuf>) -> Result<Document> {
        let doc = Document::from_bytes(bytes, path)?;
        self.bind(&doc)?;
        Ok(doc)
    }

    pub fn bind(&self, doc: &Document) -> Result<()> {
        let bytes = if doc.is_dirty() {
            Arc::new(doc.to_bytes()?)
        } else {
            doc.original_bytes()
        };
        self.bind_bytes(doc.id(), bytes)
    }

    pub fn refresh(&self, doc: &Document) -> Result<()> {
        self.bind(doc)
    }

    fn bind_bytes(&self, id: DocumentId, bytes: Arc<Vec<u8>>) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        self.send(WorkerCmd::Bind {
            engine: self.inner.id,
            id,
            bytes,
            reply: tx,
        })?;
        rx.recv()
            .map_err(|_| Error::pdfium("render thread closed"))?
    }

    pub fn close(&self, id: DocumentId) -> Result<()> {
        self.send(WorkerCmd::Unbind {
            engine: self.inner.id,
            id,
        })
    }

    /// Begin a new viewport frame: stale queued tiles are dropped.
    pub fn begin_frame(&self) -> Result<u64> {
        self.send(WorkerCmd::Bump {
            engine: self.inner.id,
        })?;
        Ok(1)
    }

    pub fn request_tiles(&self, reqs: impl IntoIterator<Item = TileRequest>) -> Result<()> {
        self.send(WorkerCmd::Request {
            engine: self.inner.id,
            reqs: reqs.into_iter().collect(),
        })
    }

    pub fn cancel(&self, keys: impl IntoIterator<Item = TileKey>) -> Result<()> {
        self.send(WorkerCmd::Cancel {
            engine: self.inner.id,
            keys: keys.into_iter().collect(),
        })
    }

    pub fn cancel_except(
        &self,
        doc: DocumentId,
        keep: impl IntoIterator<Item = TileKey>,
    ) -> Result<()> {
        self.send(WorkerCmd::CancelExcept {
            engine: self.inner.id,
            doc,
            keep: keep.into_iter().collect(),
        })
    }

    pub fn cache_get(&self, key: &TileKey) -> Option<Arc<Tile>> {
        worker().ok()?.cache.lock().ok()?.get(key)
    }

    pub fn try_recv(&self) -> Option<TileEvent> {
        let rx = self.inner.events.lock().ok()?;
        rx.try_recv().ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<TileEvent> {
        let rx = self.inner.events.lock().ok()?;
        rx.recv_timeout(timeout).ok()
    }

    pub fn page_text(&self, id: DocumentId, page: u32) -> Result<String> {
        let (tx, rx) = mpsc::channel();
        self.send(WorkerCmd::Text {
            id,
            page,
            reply: tx,
        })?;
        rx.recv()
            .map_err(|_| Error::pdfium("render thread closed"))?
    }

    pub fn search(&self, id: DocumentId, query: &str) -> Result<Vec<SearchHit>> {
        let (tx, rx) = mpsc::channel();
        self.send(WorkerCmd::Search {
            id,
            query: query.to_string(),
            reply: tx,
        })?;
        rx.recv()
            .map_err(|_| Error::pdfium("render thread closed"))?
    }

    pub fn render_page(&self, id: DocumentId, page: u32, zoom: f32) -> Result<Raster> {
        let (tx, rx) = mpsc::channel();
        self.send(WorkerCmd::RenderPage {
            id,
            page,
            zoom,
            reply: tx,
        })?;
        rx.recv()
            .map_err(|_| Error::pdfium("render thread closed"))?
    }

    pub fn render_tile_blocking(&self, key: TileKey) -> Result<Tile> {
        let (tx, rx) = mpsc::channel();
        self.send(WorkerCmd::RenderTile { key, reply: tx })?;
        rx.recv()
            .map_err(|_| Error::pdfium("render thread closed"))?
    }

    fn send(&self, cmd: WorkerCmd) -> Result<()> {
        worker()?
            .cmd
            .send(cmd)
            .map_err(|_| Error::pdfium("render thread closed"))
    }
}

impl Engine {
    fn register() -> Result<Self> {
        let id = NEXT_ENGINE_ID.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        worker()?
            .cmd
            .send(WorkerCmd::Register {
                engine: id,
                events: tx,
            })
            .map_err(|_| Error::pdfium("render thread closed"))?;
        Ok(Self {
            inner: Arc::new(EngineInner {
                id,
                events: Mutex::new(rx),
            }),
        })
    }
}

#[cfg(test)]
mod drop_tests {
    use super::*;

    fn probe(engine: EngineId, doc: DocumentId) -> (bool, bool) {
        let (tx, rx) = mpsc::channel();
        worker()
            .unwrap()
            .cmd
            .send(WorkerCmd::Probe {
                engine,
                doc,
                reply: tx,
            })
            .unwrap();
        rx.recv().unwrap()
    }

    fn engine_or_skip() -> Option<Engine> {
        match Engine::new() {
            Ok(e) => Some(e),
            Err(err) => {
                assert!(std::env::var("CI").is_err(), "PDFium required in CI: {err}");
                None
            }
        }
    }

    #[test]
    fn concurrent_drop_of_last_two_clones_unregisters_once() {
        if engine_or_skip().is_none() {
            return;
        }
        for _ in 0..1000 {
            let a = Engine::new().unwrap();
            let id = a.inner.id;
            let b = a.clone();
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let handles: Vec<_> = [a, b]
                .into_iter()
                .map(|e| {
                    let barrier = Arc::clone(&barrier);
                    thread::spawn(move || {
                        barrier.wait();
                        drop(e);
                    })
                })
                .collect();
            for h in handles {
                h.join().unwrap();
            }
            assert!(
                !probe(id, DocumentId(u64::MAX)).0,
                "engine {id} still registered"
            );
        }
    }

    #[test]
    fn dropping_engine_releases_its_open_documents() {
        let Some(engine) = engine_or_skip() else {
            return;
        };
        let doc = crate::blank_document(1).unwrap();
        engine.bind(&doc).unwrap();
        let id = engine.inner.id;
        assert_eq!(probe(id, doc.id()), (true, true));
        drop(engine);
        assert_eq!(probe(id, doc.id()), (false, false));
    }

    #[test]
    fn document_bound_by_two_engines_survives_one_drop() {
        let Some(a) = engine_or_skip() else {
            return;
        };
        let b = Engine::new().unwrap();
        let doc = crate::blank_document(1).unwrap();
        a.bind(&doc).unwrap();
        b.bind(&doc).unwrap();
        let (ida, idb) = (a.inner.id, b.inner.id);
        drop(a);
        assert_eq!(probe(ida, doc.id()), (false, true));
        drop(b);
        assert_eq!(probe(idb, doc.id()), (false, false));
    }

    /// One page with `segments` stroked lines: slow enough that a tile is
    /// still rendering when the engine is dropped.
    fn heavy_pdf(segments: u32) -> Vec<u8> {
        let mut content = String::from("0.2 w\n");
        let mut seed: u32 = 1;
        let mut next = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 8) % 600
        };
        for _ in 0..segments {
            let (x, y, dx, dy) = (next(), next(), next() % 60, next() % 60);
            content.push_str(&format!("{x} {y} m {} {} l S\n", x + dx, y + dy));
        }
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_string(),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes(),
        );
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

    #[test]
    fn drop_engine_while_tile_is_rendering() {
        let Some(witness) = engine_or_skip() else {
            return;
        };
        let bytes = heavy_pdf(40_000);
        for _ in 0..200 {
            let engine = Engine::new().unwrap();
            let doc = engine.open_bytes(bytes.clone(), None).unwrap();
            let id = engine.inner.id;
            let reqs: Vec<_> = (0..4)
                .flat_map(|y| (0..4).map(move |x| (x, y)))
                .map(|(x, y)| TileRequest::visible(TileKey::new(doc.id(), 0, 1.27, x, y)))
                .collect();
            engine.request_tiles(reqs).unwrap();
            thread::sleep(Duration::from_millis(2));
            drop(engine);
            // Probe is queued behind Unregister on the render thread.
            assert_eq!(probe(id, doc.id()), (false, false));
            assert!(
                witness.try_recv().is_none(),
                "stray event on another engine"
            );
        }
        // The render thread survived every drop.
        let doc = witness.open_bytes(heavy_pdf(10), None).unwrap();
        assert!(witness
            .render_tile_blocking(TileKey::new(doc.id(), 0, 1.0, 0, 0))
            .is_ok());
    }
}

#[cfg(test)]
mod bind_tests {
    use super::*;

    #[test]
    fn pdfium_reads_the_documents_own_buffer() {
        let engine = match Engine::new() {
            Ok(e) => e,
            Err(err) => {
                assert!(std::env::var("CI").is_err(), "PDFium required in CI: {err}");
                return;
            }
        };
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/hello.pdf");
        let doc = engine.open(&path).unwrap();
        let (tx, rx) = mpsc::channel();
        engine
            .send(WorkerCmd::LoadedPtr {
                id: doc.id(),
                reply: tx,
            })
            .unwrap();
        let loaded = rx.recv().unwrap().expect("document not bound");
        assert_eq!(loaded, doc.original_ptr() as usize);
        // Still renders from the shared buffer.
        assert!(engine.render_page(doc.id(), 0, 1.0).is_ok());
    }
}
