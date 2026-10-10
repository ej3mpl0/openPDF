use std::collections::HashMap;
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
}

enum WorkerCmd {
    Bind {
        id: DocumentId,
        bytes: Vec<u8>,
        reply: Sender<Result<()>>,
    },
    Unbind {
        id: DocumentId,
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

struct BoundDoc {
    // SAFETY: created from `pdfium` on this thread and dropped before `pdfium`.
    doc: PdfDocument<'static>,
}

fn load_bound(pdfium: &Pdfium, bytes: Vec<u8>) -> Result<BoundDoc> {
    let doc = pdfium
        .load_pdf_from_byte_vec(bytes, None)
        .map_err(|e| Error::pdfium(e.to_string()))?;
    // See BoundDoc.
    let doc = unsafe { std::mem::transmute::<PdfDocument<'_>, PdfDocument<'static>>(doc) };
    Ok(BoundDoc { doc })
}

fn worker_loop(pdfium: Pdfium, cmd_rx: Receiver<WorkerCmd>, cache: Arc<Mutex<TileCache>>) {
    let mut docs: HashMap<DocumentId, BoundDoc> = HashMap::new();
    let mut engines: HashMap<EngineId, EngineSlot> = HashMap::new();
    let mut order: Vec<EngineId> = Vec::new();
    let mut next = 0usize;

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            if !handle_cmd(&pdfium, cmd, &mut docs, &mut engines, &cache) {
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
                Some(bound) => pdfium_bind::render_tile(
                    &bound.doc,
                    key.page,
                    key.zoom(),
                    key.x,
                    key.y,
                    super::TILE_SIZE,
                )
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
                if !handle_cmd(&pdfium, cmd, &mut docs, &mut engines, &cache) {
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
                if !handle_cmd(&pdfium, cmd, &mut docs, &mut engines, &cache) {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn handle_cmd(
    pdfium: &Pdfium,
    cmd: WorkerCmd,
    docs: &mut HashMap<DocumentId, BoundDoc>,
    engines: &mut HashMap<EngineId, EngineSlot>,
    cache: &Arc<Mutex<TileCache>>,
) -> bool {
    match cmd {
        WorkerCmd::Bind { id, bytes, reply } => {
            let result = load_bound(pdfium, bytes).map(|bound| {
                docs.insert(id, bound);
                if let Ok(mut c) = cache.lock() {
                    c.invalidate_doc(id);
                }
            });
            let _ = reply.send(result);
        }
        WorkerCmd::Unbind { id } => {
            docs.remove(&id);
            if let Ok(mut c) = cache.lock() {
                c.invalidate_doc(id);
            }
        }
        WorkerCmd::Register { engine, events } => {
            engines.insert(
                engine,
                EngineSlot {
                    queue: RenderQueue::new(),
                    events,
                },
            );
        }
        WorkerCmd::Unregister { engine } => {
            engines.remove(&engine);
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
                Some(b) => pdfium_bind::page_text(&b.doc, page),
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
                    pdfium_bind::render_page(&b.doc, page, zoom).map(|(w, h, rgba)| Raster {
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
                Some(b) => pdfium_bind::render_tile(
                    &b.doc,
                    key.page,
                    key.zoom(),
                    key.x,
                    key.y,
                    super::TILE_SIZE,
                )
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
                Some(b) => search_doc(&b.doc, &query),
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
    id: EngineId,
    events: Arc<Mutex<Receiver<TileEvent>>>,
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
            doc.to_bytes()?
        } else {
            doc.original_bytes().to_vec()
        };
        self.bind_bytes(doc.id(), bytes)
    }

    pub fn refresh(&self, doc: &Document) -> Result<()> {
        self.bind(doc)
    }

    fn bind_bytes(&self, id: DocumentId, bytes: Vec<u8>) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        self.send(WorkerCmd::Bind {
            id,
            bytes,
            reply: tx,
        })?;
        rx.recv()
            .map_err(|_| Error::pdfium("render thread closed"))?
    }

    pub fn close(&self, id: DocumentId) -> Result<()> {
        self.send(WorkerCmd::Unbind { id })
    }

    /// Begin a new viewport frame: stale queued tiles are dropped.
    pub fn begin_frame(&self) -> Result<u64> {
        self.send(WorkerCmd::Bump { engine: self.id })?;
        Ok(1)
    }

    pub fn request_tiles(&self, reqs: impl IntoIterator<Item = TileRequest>) -> Result<()> {
        self.send(WorkerCmd::Request {
            engine: self.id,
            reqs: reqs.into_iter().collect(),
        })
    }

    pub fn cancel(&self, keys: impl IntoIterator<Item = TileKey>) -> Result<()> {
        self.send(WorkerCmd::Cancel {
            engine: self.id,
            keys: keys.into_iter().collect(),
        })
    }

    pub fn cancel_except(
        &self,
        doc: DocumentId,
        keep: impl IntoIterator<Item = TileKey>,
    ) -> Result<()> {
        self.send(WorkerCmd::CancelExcept {
            engine: self.id,
            doc,
            keep: keep.into_iter().collect(),
        })
    }

    pub fn cache_get(&self, key: &TileKey) -> Option<Arc<Tile>> {
        worker().ok()?.cache.lock().ok()?.get(key)
    }

    pub fn try_recv(&self) -> Option<TileEvent> {
        let rx = self.events.lock().ok()?;
        rx.try_recv().ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<TileEvent> {
        let rx = self.events.lock().ok()?;
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
            id,
            events: Arc::new(Mutex::new(rx)),
        })
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if Arc::strong_count(&self.events) == 1 {
            let _ = self.send(WorkerCmd::Unregister { engine: self.id });
        }
    }
}
