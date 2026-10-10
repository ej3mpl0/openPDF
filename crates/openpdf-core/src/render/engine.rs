use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use pdfium_render::prelude::{PdfDocument, Pdfium};

use crate::document::{DocSecret, Document, DocumentId};
use crate::error::{Error, InvalidKind, Result};
use zeroize::Zeroizing;

use super::cache::TileCache;
use super::pdfium::{self as pdfium_bind};
use super::queue::RenderQueue;
use super::{Raster, SearchHit, Tile, TileEvent, TileKey, TileRequest, DEFAULT_CACHE_BYTES};

enum WorkerCmd {
    Bind {
        id: DocumentId,
        bytes: Vec<u8>,
        password: Option<Arc<DocSecret>>,
        reply: Sender<Result<()>>,
    },
    Unbind {
        id: DocumentId,
    },
    Request {
        reqs: Vec<TileRequest>,
    },
    Cancel {
        keys: Vec<TileKey>,
    },
    CancelExcept {
        doc: DocumentId,
        keep: Vec<TileKey>,
    },
    Bump,
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
    events: Mutex<Receiver<TileEvent>>,
    event_tx: Sender<TileEvent>,
    cache: Arc<Mutex<TileCache>>,
}

static WORKER: OnceLock<std::result::Result<WorkerHandle, String>> = OnceLock::new();

fn worker() -> Result<&'static WorkerHandle> {
    match WORKER.get_or_init(|| start_worker(None).map_err(|e| e.to_string())) {
        Ok(h) => Ok(h),
        Err(e) => Err(Error::pdfium(e.clone())),
    }
}

fn start_worker(bundled: Option<&Path>) -> Result<WorkerHandle> {
    let pdfium = pdfium_bind::bind_pdfium(bundled)?;
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let cache = Arc::new(Mutex::new(TileCache::new(DEFAULT_CACHE_BYTES)));
    let cache_thread = Arc::clone(&cache);
    let event_tx_thread = event_tx.clone();

    thread::Builder::new()
        .name("openpdf-render".into())
        .spawn(move || worker_loop(pdfium, cmd_rx, event_tx_thread, cache_thread))
        .map_err(|e| Error::pdfium(e.to_string()))?;

    Ok(WorkerHandle {
        cmd: cmd_tx,
        events: Mutex::new(event_rx),
        event_tx,
        cache,
    })
}

struct BoundDoc {
    // SAFETY: created from `pdfium` on this thread and dropped before `pdfium`.
    doc: PdfDocument<'static>,
}

/// Load `bytes` into PDFium, decrypting with `password` if the file is encrypted.
///
/// PDFium receives the password as a `&str` and keeps its own internal copy
/// for the document's lifetime; that copy lives in C memory we cannot zeroize.
/// Our side stays in the document's zeroizing `DocSecret`.
fn load_bound(pdfium: &Pdfium, bytes: Vec<u8>, password: Option<&str>) -> Result<BoundDoc> {
    let doc = pdfium
        .load_pdf_from_byte_vec(bytes, password)
        .map_err(|e| Error::pdfium(e.to_string()))?;
    // See BoundDoc.
    let doc = unsafe { std::mem::transmute::<PdfDocument<'_>, PdfDocument<'static>>(doc) };
    Ok(BoundDoc { doc })
}

fn worker_loop(
    pdfium: Pdfium,
    cmd_rx: Receiver<WorkerCmd>,
    event_tx: Sender<TileEvent>,
    cache: Arc<Mutex<TileCache>>,
) {
    let mut docs: HashMap<DocumentId, BoundDoc> = HashMap::new();
    let mut queue = RenderQueue::new();

    loop {
        while let Ok(cmd) = cmd_rx.try_recv() {
            if !handle_cmd(&pdfium, cmd, &mut docs, &mut queue, &event_tx, &cache) {
                return;
            }
        }

        if let Some((key, _pri, gen)) = queue.pop() {
            if queue.is_cancelled(&key, gen) {
                let _ = event_tx.send(TileEvent::Cancelled(key));
                continue;
            }
            if let Some(hit) = cache.lock().ok().and_then(|mut c| c.get(&key)) {
                let _ = event_tx.send(TileEvent::Ready(hit));
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
            if queue.is_cancelled(&key, gen) {
                let _ = event_tx.send(TileEvent::Cancelled(key));
                continue;
            }
            match result {
                Ok(tile) => {
                    let tile = Arc::new(tile);
                    if let Ok(mut c) = cache.lock() {
                        c.insert(Arc::clone(&tile));
                    }
                    let _ = event_tx.send(TileEvent::Ready(tile));
                }
                Err(e) => {
                    let _ = event_tx.send(TileEvent::Failed {
                        key,
                        message: e.to_string(),
                    });
                }
            }
            continue;
        }

        match cmd_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(cmd) => {
                if !handle_cmd(&pdfium, cmd, &mut docs, &mut queue, &event_tx, &cache) {
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
    queue: &mut RenderQueue,
    event_tx: &Sender<TileEvent>,
    cache: &Arc<Mutex<TileCache>>,
) -> bool {
    match cmd {
        WorkerCmd::Bind {
            id,
            bytes,
            password,
            reply,
        } => {
            let result = load_bound(pdfium, bytes, password.as_deref().map(DocSecret::as_str)).map(
                |bound| {
                    docs.insert(id, bound);
                    if let Ok(mut c) = cache.lock() {
                        c.invalidate_doc(id);
                    }
                },
            );
            let _ = reply.send(result);
        }
        WorkerCmd::Unbind { id } => {
            docs.remove(&id);
            if let Ok(mut c) = cache.lock() {
                c.invalidate_doc(id);
            }
        }
        WorkerCmd::Request { reqs } => {
            for req in reqs {
                if let Some(hit) = cache.lock().ok().and_then(|mut c| c.get(&req.key)) {
                    let _ = event_tx.send(TileEvent::Ready(hit));
                    continue;
                }
                queue.push(req.key, req.priority);
            }
        }
        WorkerCmd::Cancel { keys } => {
            for key in keys {
                queue.cancel(key);
            }
        }
        WorkerCmd::CancelExcept { doc, keep } => {
            let keep: Vec<TileKey> = keep.into_iter().filter(|k| k.doc == doc).collect();
            queue.cancel_except(&keep);
        }
        WorkerCmd::Bump => {
            queue.bump_generation();
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
#[derive(Clone, Debug)]
pub struct Engine {
    _private: (),
}

impl Engine {
    /// Bind PDFium from `OPENPDF_PDFIUM`, a bundled path, or the system library.
    pub fn new() -> Result<Self> {
        worker()?;
        Ok(Self { _private: () })
    }

    /// First successful call wins; later engines share the same worker.
    pub fn with_bundled_library(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        match WORKER.get_or_init(|| start_worker(Some(&path)).map_err(|e| e.to_string())) {
            Ok(_) => Ok(Self { _private: () }),
            Err(e) => Err(Error::pdfium(e.clone())),
        }
    }

    pub fn open(&self, path: impl AsRef<Path>) -> Result<Document> {
        let doc = Document::open(path)?;
        self.bind(&doc)?;
        Ok(doc)
    }

    pub fn open_with_password(
        &self,
        path: impl AsRef<Path>,
        password: Zeroizing<String>,
    ) -> Result<Document> {
        let doc = Document::open_with_password(path, password)?;
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
        self.bind_bytes(doc.id(), bytes, doc.password().get().cloned())
    }

    pub fn refresh(&self, doc: &Document) -> Result<()> {
        self.bind(doc)
    }

    fn bind_bytes(
        &self,
        id: DocumentId,
        bytes: Vec<u8>,
        password: Option<Arc<DocSecret>>,
    ) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        self.send(WorkerCmd::Bind {
            id,
            bytes,
            password,
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
        self.send(WorkerCmd::Bump)?;
        Ok(1)
    }

    pub fn request_tiles(&self, reqs: impl IntoIterator<Item = TileRequest>) -> Result<()> {
        self.send(WorkerCmd::Request {
            reqs: reqs.into_iter().collect(),
        })
    }

    pub fn cancel(&self, keys: impl IntoIterator<Item = TileKey>) -> Result<()> {
        self.send(WorkerCmd::Cancel {
            keys: keys.into_iter().collect(),
        })
    }

    pub fn cancel_except(
        &self,
        doc: DocumentId,
        keep: impl IntoIterator<Item = TileKey>,
    ) -> Result<()> {
        self.send(WorkerCmd::CancelExcept {
            doc,
            keep: keep.into_iter().collect(),
        })
    }

    pub fn cache_get(&self, key: &TileKey) -> Option<Arc<Tile>> {
        worker().ok()?.cache.lock().ok()?.get(key)
    }

    pub fn try_recv(&self) -> Option<TileEvent> {
        let h = worker().ok()?;
        let rx = h.events.lock().ok()?;
        rx.try_recv().ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<TileEvent> {
        let h = worker().ok()?;
        let rx = h.events.lock().ok()?;
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
    #[allow(dead_code)]
    fn event_sender(&self) -> Result<Sender<TileEvent>> {
        Ok(worker()?.event_tx.clone())
    }
}
