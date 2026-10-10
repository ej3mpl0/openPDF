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

/// Font warmup state, once per process (the worker is a process singleton).
///
/// PDFium enumerates the system font directories lazily, on the first render
/// (on Linux ~1500 files under /usr/share/fonts, ~150 ms cold). The render
/// thread does that once right after start, on a tiny in-memory PDF, so it
/// overlaps with the app's UI startup instead of the first visible tile.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
static FONT_WARMUP_RUNS: AtomicU64 = AtomicU64::new(0);
static FONT_WARMUP_DONE: OnceLock<Duration> = OnceLock::new();

/// A 1x1 pt page showing one glyph in a non-embedded, non-standard font, so
/// PDFium has to consult its system font mapper.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn font_warmup_pdf() -> Result<Vec<u8>> {
    use lopdf::{dictionary, Object, Stream};
    let mut doc = lopdf::Document::with_version("1.4");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "TrueType",
        "BaseFont" => "OpenPDFWarmupSans",
        "Encoding" => "WinAnsiEncoding",
    });
    let content = doc.add_object(Stream::new(
        dictionary! {},
        b"BT /F1 1 Tf 0 0 Td (a) Tj ET".to_vec(),
    ));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), 1.into(), 1.into()],
        "Contents" => content,
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out)
        .map_err(|e| Error::pdfium(e.to_string()))?;
    Ok(out)
}

/// Set when the warmup starts (test hook to force overlap with `bind`).
#[cfg(test)]
static FONT_WARMUP_BEGUN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// While false, the test build holds the warmup open (max 10 s) so a test can
/// deterministically send `bind` during it.
#[cfg(test)]
static FONT_WARMUP_RELEASE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Render the warmup page once. Errors and panics are swallowed: the warmup is
/// only an optimisation, the first real render would scan fonts anyway, and
/// it must never take the render thread down.
#[cfg(target_os = "linux")]
fn font_warmup(pdfium: &Pdfium, started: Instant) {
    FONT_WARMUP_RUNS.fetch_add(1, Ordering::SeqCst);
    let body = std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        {
            FONT_WARMUP_BEGUN.store(true, Ordering::SeqCst);
            let until = Instant::now() + Duration::from_secs(10);
            while !FONT_WARMUP_RELEASE.load(Ordering::SeqCst) && Instant::now() < until {
                thread::sleep(Duration::from_millis(1));
            }
        }
        if let Ok(bytes) = font_warmup_pdf() {
            if let Ok(doc) = pdfium.load_pdf_from_byte_vec(bytes, None) {
                let _ = pdfium_bind::render_page(&doc, 0, 1.0);
            }
        }
        #[cfg(test)]
        panic!("test-only: warmup panic must be caught");
    });
    let _ = std::panic::catch_unwind(body);
    let _ = FONT_WARMUP_DONE.set(started.elapsed());
}

/// Milliseconds from the start of PDFium init until the render thread finished
/// its one-time system font warmup. `None` while it runs, if PDFium is not
/// loaded, or on non-Linux (PDFium uses GDI/CoreText there, no warmup).
pub fn font_warmup_ms() -> Option<f64> {
    FONT_WARMUP_DONE.get().map(|d| d.as_secs_f64() * 1000.0)
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
        .spawn(move || worker_loop(pdfium, cmd_rx, cache_thread, t_load))
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

fn worker_loop(
    pdfium: Pdfium,
    cmd_rx: Receiver<WorkerCmd>,
    cache: Arc<Mutex<TileCache>>,
    started: Instant,
) {
    let mut docs: HashMap<DocumentId, BoundDoc> = HashMap::new();
    let mut engines: HashMap<EngineId, EngineSlot> = HashMap::new();
    let mut order: Vec<EngineId> = Vec::new();
    let mut next = 0usize;

    // Commands already waiting (register, bind) go first; then the font scan,
    // before any tile render. Commands sent during the warmup queue in the
    // channel and are handled right after it.
    while let Ok(cmd) = cmd_rx.try_recv() {
        if !handle_cmd(&pdfium, cmd, &mut docs, &mut engines, &cache) {
            return;
        }
    }
    #[cfg(target_os = "linux")]
    font_warmup(&pdfium, started);
    #[cfg(not(target_os = "linux"))]
    let _ = started;

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

    /// Milliseconds from the start of PDFium init until the render thread
    /// finished its one-time system font warmup; `None` while it is running
    /// (or if PDFium is unavailable).
    pub fn font_warmup_ms(&self) -> Option<f64> {
        font_warmup_ms()
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

    /// Blocking. The first bind of the process may wait up to
    /// [`font_warmup_ms`] (~150-200 ms cold on Linux) while the render thread
    /// scans system fonts. Do not call from a UI thread.
    pub fn open(&self, path: impl AsRef<Path>) -> Result<Document> {
        let doc = Document::open(path)?;
        self.bind(&doc)?;
        Ok(doc)
    }

    /// Blocking. The first bind of the process may wait up to
    /// [`font_warmup_ms`] (~150-200 ms cold on Linux) while the render thread
    /// scans system fonts. Do not call from a UI thread.
    pub fn open_bytes(&self, bytes: Vec<u8>, path: Option<std::path::PathBuf>) -> Result<Document> {
        let doc = Document::from_bytes(bytes, path)?;
        self.bind(&doc)?;
        Ok(doc)
    }

    /// Blocking. The first bind of the process may wait up to
    /// [`font_warmup_ms`] (~150-200 ms cold on Linux) while the render thread
    /// scans system fonts. Do not call from a UI thread.
    pub fn bind(&self, doc: &Document) -> Result<()> {
        let bytes = if doc.is_dirty() {
            doc.to_bytes()?
        } else {
            doc.original_bytes().to_vec()
        };
        self.bind_bytes(doc.id(), bytes)
    }

    /// Blocking. The first bind of the process may wait up to
    /// [`font_warmup_ms`] (~150-200 ms cold on Linux) while the render thread
    /// scans system fonts. Do not call from a UI thread.
    pub fn refresh(&self, doc: &Document) -> Result<()> {
        self.bind(doc)
    }

    /// Blocking. The first bind of the process may wait up to
    /// [`font_warmup_ms`] (~150-200 ms cold on Linux) while the render thread
    /// scans system fonts. Do not call from a UI thread.
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

#[cfg(test)]
mod font_warmup_tests {
    use super::*;

    fn engine_or_skip() -> Option<Engine> {
        match Engine::new() {
            Ok(e) => Some(e),
            Err(e) if std::env::var("CI").is_err() => {
                eprintln!("skipping: {e}");
                None
            }
            Err(e) => panic!("PDFium required in CI: {e}"),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn font_warmup_runs_once_and_does_not_deadlock() {
        let Some(first) = engine_or_skip() else {
            return;
        };
        let bytes = crate::blank_document(1).unwrap().to_bytes().unwrap();

        // Many engines registering, binding and rendering concurrently while
        // the warmup may still be running: all must finish, warmup only once.
        let (done_tx, done_rx) = mpsc::channel();
        for _ in 0..8 {
            let done_tx = done_tx.clone();
            let bytes = bytes.clone();
            thread::spawn(move || {
                let doc = Document::from_bytes(bytes, None).unwrap();
                let engine = Engine::new().unwrap();
                engine.bind(&doc).unwrap();
                let tile = engine
                    .render_tile_blocking(TileKey::new(doc.id(), 0, 1.0, 0, 0))
                    .unwrap();
                done_tx.send(tile.width > 0).unwrap();
            });
        }
        drop(done_tx);
        for _ in 0..8 {
            let ok = done_rx
                .recv_timeout(Duration::from_secs(30))
                .expect("engine thread deadlocked or panicked");
            assert!(ok);
        }

        let deadline = Instant::now() + Duration::from_secs(30);
        while first.font_warmup_ms().is_none() {
            assert!(Instant::now() < deadline, "font warmup never finished");
            thread::sleep(Duration::from_millis(5));
        }
        assert!(first.font_warmup_ms().unwrap() >= first.pdfium_load_ms());
        assert_eq!(FONT_WARMUP_RUNS.load(Ordering::SeqCst), 1);
    }

    /// Forces `bind` to arrive while the warmup is running, then checks it
    /// finishes and waits no longer than about the warmup itself.
    #[cfg(target_os = "linux")]
    #[test]
    fn bind_during_warmup_waits_then_completes() {
        let Some(engine) = engine_or_skip() else {
            FONT_WARMUP_RELEASE.store(true, Ordering::SeqCst);
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        while !FONT_WARMUP_BEGUN.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "warmup never began");
            thread::sleep(Duration::from_millis(1));
        }
        let overlapped = font_warmup_ms().is_none();
        let bytes = crate::blank_document(1).unwrap().to_bytes().unwrap();
        let doc = Document::from_bytes(bytes, None).unwrap();
        let (tx, rx) = mpsc::channel();
        let t0 = Instant::now();
        let h = thread::spawn(move || {
            engine.bind(&doc).unwrap();
            tx.send(t0.elapsed()).unwrap();
        });
        thread::sleep(Duration::from_millis(20));
        let release_at = t0.elapsed();
        FONT_WARMUP_RELEASE.store(true, Ordering::SeqCst);
        let took = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("bind deadlocked behind warmup");
        h.join().unwrap();
        let warm = font_warmup_ms().expect("warmup done");
        let real_scan = Duration::from_secs_f64(warm / 1000.0);
        eprintln!(
            "bind_during_warmup: overlapped={overlapped} bind_ms={:.1} held_ms={:.1} font_warmup_ms={warm:.1}",
            took.as_secs_f64() * 1000.0,
            release_at.as_secs_f64() * 1000.0,
        );
        assert!(took <= release_at + real_scan + Duration::from_secs(2));
    }

    /// The test build panics inside the warmup; the render thread must survive.
    #[cfg(target_os = "linux")]
    #[test]
    fn warmup_panic_does_not_kill_render_thread() {
        let Some(engine) = engine_or_skip() else {
            return;
        };
        FONT_WARMUP_RELEASE.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(30);
        while font_warmup_ms().is_none() {
            assert!(Instant::now() < deadline, "warmup never finished");
            thread::sleep(Duration::from_millis(5));
        }
        let bytes = crate::blank_document(1).unwrap().to_bytes().unwrap();
        let doc = Document::from_bytes(bytes, None).unwrap();
        engine.bind(&doc).unwrap();
        let tile = engine
            .render_tile_blocking(TileKey::new(doc.id(), 0, 1.0, 0, 0))
            .unwrap();
        assert!(tile.width > 0);
    }

    #[test]
    fn font_warmup_pdf_is_valid() {
        let bytes = font_warmup_pdf().unwrap();
        let doc = lopdf::Document::load_mem(&bytes).unwrap();
        assert_eq!(doc.get_pages().len(), 1);
    }
}
