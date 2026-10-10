// SPDX-License-Identifier: MPL-2.0
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use openpdf_core::{merge, Document};

/// Single `--measure` time origin. Set first thing in `main`; every `ui_*`
/// mark and `startup_ms` is milliseconds from this Instant.
static PROCESS_START: OnceLock<Instant> = OnceLock::new();

/// Pin the `--measure` clock to process start (first call wins).
pub fn mark_process_start() {
    let _ = PROCESS_START.set(Instant::now());
}

/// Milliseconds since [`mark_process_start`].
///
/// If `main` forgot to call [`mark_process_start`], the first read pins the
/// clock so later marks still share one origin.
pub fn process_elapsed_ms() -> u128 {
    PROCESS_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
}

// Each mark is a OnceLock: first call wins, later calls are no-ops.
static DOC_LOAD_START: OnceLock<u128> = OnceLock::new();
static DOC_LOADED: OnceLock<u128> = OnceLock::new();
static TILE_REQUESTED: OnceLock<u128> = OnceLock::new();
static TILE_RENDERED: OnceLock<u128> = OnceLock::new();
static SURFACE_READY: OnceLock<u128> = OnceLock::new();
static TILE_PAINTED: OnceLock<u128> = OnceLock::new();
static THUMBS_DONE: OnceLock<u128> = OnceLock::new();
static ALL_VISIBLE: OnceLock<u128> = OnceLock::new();
static IDLE_LOADS_DONE: OnceLock<u128> = OnceLock::new();
static IDLE_LOADS_PENDING: AtomicU32 = AtomicU32::new(0);

pub fn mark_doc_load_start() {
    let _ = DOC_LOAD_START.get_or_init(process_elapsed_ms);
}
pub fn mark_doc_loaded() {
    let _ = DOC_LOADED.get_or_init(process_elapsed_ms);
}
pub fn mark_first_tile_requested() {
    let _ = TILE_REQUESTED.get_or_init(process_elapsed_ms);
}
pub fn mark_first_tile_rendered() {
    let _ = TILE_RENDERED.get_or_init(process_elapsed_ms);
}
pub fn mark_first_surface_ready() {
    let _ = SURFACE_READY.get_or_init(process_elapsed_ms);
}
pub fn mark_first_tile_painted() {
    let _ = TILE_PAINTED.get_or_init(process_elapsed_ms);
}
pub fn mark_thumbs_done() {
    let _ = THUMBS_DONE.get_or_init(process_elapsed_ms);
}
pub fn mark_all_visible() {
    let _ = ALL_VISIBLE.get_or_init(process_elapsed_ms);
}

pub fn ui_doc_load_start_ms() -> u128 {
    DOC_LOAD_START.get().copied().unwrap_or(0)
}
pub fn ui_doc_loaded_ms() -> u128 {
    DOC_LOADED.get().copied().unwrap_or(0)
}
pub fn ui_first_tile_requested_ms() -> u128 {
    TILE_REQUESTED.get().copied().unwrap_or(0)
}
pub fn ui_first_tile_rendered_ms() -> u128 {
    TILE_RENDERED.get().copied().unwrap_or(0)
}
pub fn ui_first_surface_ready_ms() -> u128 {
    SURFACE_READY.get().copied().unwrap_or(0)
}
pub fn ui_first_tile_painted_ms() -> u128 {
    TILE_PAINTED.get().copied().unwrap_or(0)
}
pub fn ui_thumbs_done_ms() -> u128 {
    THUMBS_DONE.get().copied().unwrap_or(0)
}
pub fn ui_all_visible_painted_ms() -> u128 {
    ALL_VISIBLE.get().copied().unwrap_or(0)
}

/// One deferred icon / idle load still outstanding.
pub fn note_idle_load_pending() {
    IDLE_LOADS_PENDING.fetch_add(1, Ordering::SeqCst);
}

/// A deferred load finished. The last one pins [`ui_idle_loads_done_ms`].
pub fn note_idle_load_finished() {
    let prev = IDLE_LOADS_PENDING.fetch_sub(1, Ordering::SeqCst);
    if prev == 1 {
        let _ = IDLE_LOADS_DONE.get_or_init(process_elapsed_ms);
    }
}

pub fn mark_idle_loads_done() {
    let _ = IDLE_LOADS_DONE.get_or_init(process_elapsed_ms);
}

pub fn ui_idle_loads_done_ms() -> u128 {
    IDLE_LOADS_DONE.get().copied().unwrap_or(0)
}

/// Deferred icon loads must finish within this window after the first frame.
pub const MAX_IDLE_LOAD_MS: u128 = 2_000;
/// After idle loads, wait this long before reading `peak_rss`.
pub const IDLE_LOAD_SETTLE_MS: u128 = 500;

/// Pages in the `--measure` fixture. Enough for a thumbnail strip plus a
/// multi-page continuous layout without committing a 300-page PDF.
pub const DEFAULT_MEASURE_PAGES: usize = 32;

/// Fail gate for [`BudgetReport::startup_ms`]: process start until every
/// visible page tile and every visible thumbnail has been painted.
///
/// Measured locally under `xvfb-run` + `GSK_RENDERER=cairo`: ~140–150 ms for
/// 32 pages (16 visible tiles + 3 thumbs). GitHub-hosted runners load
/// GTK/PDFium more slowly; 8 s is the fail gate, not the expected time.
pub const MAX_STARTUP_MS: u128 = 8_000;

/// Peak RSS (`VmHWM`) for the whole UI process (GTK + libadwaita + PDFium +
/// tile cache) with [`DEFAULT_MEASURE_PAGES`] pages.
///
/// Measured locally: ~132–141 MiB. The 250 MiB plan ceiling stays the gate
/// so CI noise and a warmer font/icon cache do not flake the job.
pub const MAX_RSS_BYTES: u64 = 250 * 1024 * 1024;

/// Give up waiting for tiles (must be ≥ [`MAX_STARTUP_MS`]).
pub const MAX_MEASURE_WAIT_MS: u128 = 15_000;

/// Visible-page tiles that must be in cache before the run counts as ready.
pub const MIN_VISIBLE_TILES: usize = 1;
/// Sidebar thumbnail tiles that must be in cache (a 800 px strip shows ~4).
pub const MIN_THUMB_TILES: usize = 3;

#[derive(Debug, Clone, Default)]
pub struct BudgetReport {
    pub pages: usize,
    /// Process start → all visible page tiles and visible thumbnails painted.
    /// Same instant as [`Self::ui_all_visible_ms`].
    pub startup_ms: u128,
    pub rss_bytes: u64,
    pub visible_ready: usize,
    pub visible_total: usize,
    pub thumb_ready: usize,
    pub thumb_total: usize,
    /// GTK `activate` callback, milliseconds from process start.
    pub ui_activate_ms: u128,
    /// `window.present()`, milliseconds from process start.
    pub ui_window_present_ms: u128,
    /// First frame-clock paint, milliseconds from process start.
    pub ui_first_frame_ms: u128,
    /// First real page tile painted in the DrawingArea draw func.
    pub ui_first_tile_ms: u128,
    /// `adw::init` + icon GResource registration, from process start.
    pub ui_adw_init_ms: u128,
    /// Application CSS provider load, from process start.
    pub ui_css_ms: u128,
    /// `OpenPdfWindow::new` returns (chrome only; home lists idle), from process start.
    pub ui_window_build_ms: u128,
    /// Idle/async document open begins.
    pub ui_doc_load_start_ms: u128,
    /// `Engine::open` / `Session::open` returned.
    pub ui_doc_loaded_ms: u128,
    /// First `request_tiles` (top-left visible).
    pub ui_first_tile_requested_ms: u128,
    /// Render worker finished the first tile (`try_recv`).
    pub ui_first_tile_rendered_ms: u128,
    /// First cairo `ImageSurface` created for a tile (cairo has no texture).
    pub ui_first_surface_ready_ms: u128,
    /// Alias of [`Self::startup_ms`]: all visible tiles + visible thumbs painted.
    pub ui_all_visible_ms: u128,
    /// Thumbnail strip meets the measure gate (`MIN_THUMB_TILES`).
    pub ui_thumbs_done_ms: u128,
    /// Menus / tool / sign / zoom icons finished loading after the first frame.
    pub ui_idle_loads_done_ms: u128,
    pub ok: bool,
    pub error: Option<String>,
}

impl Display for BudgetReport {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "mode=ui")?;
        writeln!(f, "pages={}", self.pages)?;
        writeln!(f, "startup_ms={}", self.startup_ms)?;
        writeln!(f, "ui_all_visible_ms={}", self.ui_all_visible_ms)?;
        writeln!(f, "ui_thumbs_done_ms={}", self.ui_thumbs_done_ms)?;
        writeln!(f, "ui_idle_loads_done_ms={}", self.ui_idle_loads_done_ms)?;
        writeln!(f, "peak_rss_bytes={}", self.rss_bytes)?;
        writeln!(
            f,
            "peak_rss_mb={:.1}",
            self.rss_bytes as f64 / (1024.0 * 1024.0)
        )?;
        writeln!(
            f,
            "visible_tiles={}/{}",
            self.visible_ready, self.visible_total
        )?;
        writeln!(f, "thumb_tiles={}/{}", self.thumb_ready, self.thumb_total)?;
        writeln!(f, "ui_activate_ms={}", self.ui_activate_ms)?;
        writeln!(f, "ui_window_present_ms={}", self.ui_window_present_ms)?;
        writeln!(f, "ui_first_frame_ms={}", self.ui_first_frame_ms)?;
        writeln!(f, "ui_first_tile_ms={}", self.ui_first_tile_ms)?;
        writeln!(f, "ui_adw_init_ms={}", self.ui_adw_init_ms)?;
        writeln!(f, "ui_css_ms={}", self.ui_css_ms)?;
        writeln!(f, "ui_window_build_ms={}", self.ui_window_build_ms)?;
        writeln!(f, "ui_doc_load_start_ms={}", self.ui_doc_load_start_ms)?;
        writeln!(f, "ui_doc_loaded_ms={}", self.ui_doc_loaded_ms)?;
        writeln!(
            f,
            "ui_first_tile_requested_ms={}",
            self.ui_first_tile_requested_ms
        )?;
        writeln!(
            f,
            "ui_first_tile_rendered_ms={}",
            self.ui_first_tile_rendered_ms
        )?;
        writeln!(
            f,
            "ui_first_surface_ready_ms={}",
            self.ui_first_surface_ready_ms
        )?;
        writeln!(f, "limit_startup_ms={MAX_STARTUP_MS}")?;
        writeln!(f, "limit_peak_rss_bytes={MAX_RSS_BYTES}")?;
        writeln!(f, "ok={}", self.ok)?;
        if let Some(err) = &self.error {
            writeln!(f, "error={err}")?;
        }
        Ok(())
    }
}

impl BudgetReport {
    pub fn evaluate(mut self) -> Self {
        let mut error = self.error.take();
        if self.visible_ready < MIN_VISIBLE_TILES {
            error.get_or_insert_with(|| {
                format!(
                    "visible tiles {}/{} (need ≥ {MIN_VISIBLE_TILES})",
                    self.visible_ready, self.visible_total
                )
            });
        }
        if self.thumb_ready < MIN_THUMB_TILES {
            error.get_or_insert_with(|| {
                format!(
                    "thumbnail tiles {}/{} (need ≥ {MIN_THUMB_TILES})",
                    self.thumb_ready, self.thumb_total
                )
            });
        }
        if self.startup_ms > MAX_STARTUP_MS {
            error.get_or_insert_with(|| {
                format!("startup {} ms exceeds {MAX_STARTUP_MS} ms", self.startup_ms)
            });
        }
        if self.rss_bytes > MAX_RSS_BYTES {
            error.get_or_insert_with(|| {
                format!("peak RSS {} exceeds {MAX_RSS_BYTES}", self.rss_bytes)
            });
        }
        if self.ui_idle_loads_done_ms == 0 {
            error.get_or_insert_with(|| {
                format!("deferred loads did not finish within {MAX_IDLE_LOAD_MS} ms")
            });
        }
        self.ok = error.is_none();
        self.error = error;
        self
    }
}

pub fn current_rss_bytes() -> u64 {
    proc_status_bytes("VmRSS:")
}

pub fn peak_rss_bytes() -> u64 {
    proc_status_bytes("VmHWM:").max(current_rss_bytes())
}

fn proc_status_bytes(key: &str) -> u64 {
    let Ok(text) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    parse_status_kb(&text, key).saturating_mul(1024)
}

fn parse_status_kb(status: &str, key: &str) -> u64 {
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            return rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
        }
    }
    0
}

/// Isolate settings so `--measure` does not touch `~/.config/openpdf`.
pub fn isolate_xdg() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("openpdf-measure-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    std::env::set_var("XDG_CONFIG_HOME", &dir);
    std::env::set_var("XDG_STATE_HOME", &dir);
    dir
}

/// Letter-size PDF with repeating text so every 256px column is distinct at 147%.
///
/// `hello.pdf` is a single short line in the top-left tile; a LAST-pointer
/// cache would still match a full-page composite of that document.
pub fn write_sample_text_pdf(pages: usize) -> Result<PathBuf, String> {
    let pages = pages.max(1);
    let font_obj = 3 + pages * 2;

    let mut kids = String::new();
    for i in 0..pages {
        let page_id = 3 + i * 2;
        if i > 0 {
            kids.push(' ');
        }
        kids.push_str(&format!("{page_id} 0 R"));
    }
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_vec();
    let pages_obj =
        format!("2 0 obj\n<< /Type /Pages /Kids [{kids}] /Count {pages} >>\nendobj\n").into_bytes();

    let mut page_objs = Vec::new();
    for i in 0..pages {
        let page_id = 3 + i * 2;
        let content_id = page_id + 1;
        let n = i + 1;
        let line = format!(
            "openPDF sample - page {n} of {pages} - openPDF sample - page {n} of {pages} - openPDF sample"
        );
        let mut stream = String::from("BT\n/F1 18 Tf\n36 750 Td\n");
        for row in 0..28 {
            if row > 0 {
                stream.push_str("0 -24 Td\n");
            }
            stream.push_str(&format!("({line} / row {row}) Tj\n"));
        }
        stream.push_str("ET\n");
        let stream_bytes = stream.into_bytes();
        let page = format!(
            "{page_id} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Contents {content_id} 0 R /Resources << /Font << /F1 {font_obj} 0 R >> >> >>\nendobj\n"
        );
        let content = format!(
            "{content_id} 0 obj\n<< /Length {} >>\nstream\n",
            stream_bytes.len()
        );
        let mut content_obj = content.into_bytes();
        content_obj.extend_from_slice(&stream_bytes);
        if !stream_bytes.ends_with(b"\n") {
            content_obj.push(b'\n');
        }
        content_obj.extend_from_slice(b"endstream\nendobj\n");
        page_objs.push(page.into_bytes());
        page_objs.push(content_obj);
    }
    let font = format!(
        "{font_obj} 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n"
    )
    .into_bytes();

    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n");
    let mut offsets = vec![0u32; font_obj + 1];
    let write_obj = |id: usize, bytes: &[u8], pdf: &mut Vec<u8>, offsets: &mut [u32]| {
        offsets[id] = pdf.len() as u32;
        pdf.extend_from_slice(bytes);
    };
    write_obj(1, &catalog, &mut pdf, &mut offsets);
    write_obj(2, &pages_obj, &mut pdf, &mut offsets);
    for (i, obj) in page_objs.iter().enumerate() {
        write_obj(3 + i, obj, &mut pdf, &mut offsets);
    }
    write_obj(font_obj, &font, &mut pdf, &mut offsets);

    let xref_at = pdf.len();
    let nobj = font_obj + 1;
    pdf.extend_from_slice(format!("xref\n0 {nobj}\n").as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for id in 1..nobj {
        pdf.extend_from_slice(format!("{:010} 00000 n \n", offsets[id]).as_bytes());
    }
    pdf.extend_from_slice(
        format!("trailer\n<< /Size {nobj} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n")
            .as_bytes(),
    );

    let dest = std::env::temp_dir().join(format!(
        "openpdf-sample-text-{}-{}.pdf",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&dest, pdf).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// Build a real multi-page PDF by repeating `corpus/hello.pdf` (no fat fixture).
pub fn write_measure_pdf(pages: usize) -> Result<PathBuf, String> {
    let pages = pages.max(2);
    let src = corpus_pdf("hello.pdf").ok_or_else(|| {
        "corpus/hello.pdf is missing; cannot build the measure document".to_string()
    })?;
    let src = Document::open(&src).map_err(|e| e.to_string())?;
    if src.page_count() == 0 {
        return Err("hello.pdf has no pages".into());
    }
    let copies = pages.div_ceil(src.page_count() as usize);
    let docs = vec![src; copies];
    let mut merged = merge(&docs).map_err(|e| e.to_string())?;
    while merged.page_count() as usize > pages {
        let last = merged.page_count() - 1;
        merged.delete_page(last).map_err(|e| e.to_string())?;
    }
    let dest = std::env::temp_dir().join(format!(
        "openpdf-measure-{}-{}.pdf",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    merged.save(&dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

fn corpus_pdf(name: &str) -> Option<PathBuf> {
    let candidates = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus")
            .join(name),
        PathBuf::from("corpus").join(name),
    ];
    candidates.into_iter().find(|p| p.exists())
}

pub fn has_display() -> bool {
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

pub fn display_hint() -> &'static str {
    "no display; run under xvfb-run (CI) or a local X/Wayland session"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rss_and_hwm() {
        let status = "VmPeak:\t  1000 kB\nVmRSS:\t   512 kB\nVmHWM:\t   768 kB\n";
        assert_eq!(parse_status_kb(status, "VmRSS:"), 512);
        assert_eq!(parse_status_kb(status, "VmHWM:"), 768);
    }

    #[test]
    fn evaluate_rejects_missing_tiles() {
        let report = BudgetReport {
            pages: 32,
            startup_ms: 100,
            rss_bytes: 10 * 1024 * 1024,
            visible_ready: 0,
            visible_total: 0,
            thumb_ready: 0,
            thumb_total: 0,
            ok: false,
            error: None,
            ..Default::default()
        }
        .evaluate();
        assert!(!report.ok);
        assert!(report.error.unwrap().contains("visible tiles"));
    }

    #[test]
    fn display_keeps_startup_rss_and_adds_ui_marks() {
        let text = BudgetReport {
            pages: 32,
            startup_ms: 200,
            rss_bytes: 1024,
            ui_activate_ms: 10,
            ui_window_present_ms: 20,
            ui_first_frame_ms: 30,
            ui_first_tile_ms: 40,
            ui_all_visible_ms: 200,
            ui_thumbs_done_ms: 180,
            ui_idle_loads_done_ms: 190,
            ..Default::default()
        }
        .to_string();
        assert!(text.contains("startup_ms=200"));
        assert!(text.contains("ui_all_visible_ms=200"));
        assert!(text.contains("ui_thumbs_done_ms=180"));
        assert!(text.contains("ui_idle_loads_done_ms=190"));
        assert!(text.contains("peak_rss_bytes=1024"));
        assert!(text.contains("ui_activate_ms=10"));
        assert!(text.contains("ui_window_present_ms=20"));
        assert!(text.contains("ui_first_frame_ms=30"));
        assert!(text.contains("ui_first_tile_ms=40"));
        assert!(text.contains("ui_adw_init_ms="));
        assert!(text.contains("ui_css_ms="));
        assert!(text.contains("ui_window_build_ms="));
        assert!(text.contains("ui_doc_load_start_ms="));
        assert!(text.contains("ui_first_tile_requested_ms="));
        assert!(text.contains("ui_first_surface_ready_ms="));
    }

    #[test]
    fn write_measure_pdf_is_multi_page() {
        let path = write_measure_pdf(4).unwrap();
        let doc = Document::open(&path).unwrap();
        assert_eq!(doc.page_count(), 4);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn write_sample_text_pdf_is_multi_page() {
        let path = write_sample_text_pdf(8).unwrap();
        let doc = Document::open(&path).unwrap();
        assert_eq!(doc.page_count(), 8);
        let _ = std::fs::remove_file(path);
    }
}
