// SPDX-License-Identifier: MPL-2.0
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

use gtk4 as gtk;
use openpdf_core::{merge, Document};
use pangocairo::prelude::*;

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
static PDFIUM_LOADED: OnceLock<u128> = OnceLock::new();
static PANGO_INIT: OnceLock<u128> = OnceLock::new();
static GTK_STARTUP: OnceLock<u128> = OnceLock::new();
static ENGINE_CREATED: OnceLock<u128> = OnceLock::new();

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
pub fn mark_pdfium_loaded() {
    let _ = PDFIUM_LOADED.get_or_init(process_elapsed_ms);
}
pub fn mark_pango_init() {
    let _ = PANGO_INIT.get_or_init(process_elapsed_ms);
}
pub fn mark_gtk_startup() {
    let _ = GTK_STARTUP.get_or_init(process_elapsed_ms);
}
pub fn mark_engine_created() {
    let _ = ENGINE_CREATED.get_or_init(process_elapsed_ms);
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
pub fn ui_pdfium_load_ms() -> u128 {
    PDFIUM_LOADED.get().copied().unwrap_or(0)
}
pub fn ui_pango_init_ms() -> u128 {
    PANGO_INIT.get().copied().unwrap_or(0)
}
pub fn ui_gtk_startup_ms() -> u128 {
    GTK_STARTUP.get().copied().unwrap_or(0)
}
pub fn ui_engine_created_ms() -> u128 {
    ENGINE_CREATED.get().copied().unwrap_or(0)
}

/// `ui_first_tile_ms` must be a real draw-path paint, not before the worker mark.
pub fn first_tile_mark_valid(painted_ms: u128, rendered_ms: u128) -> bool {
    painted_ms > 0 && painted_ms >= rendered_ms
}
pub fn ui_thumbs_done_ms() -> u128 {
    THUMBS_DONE.get().copied().unwrap_or(0)
}
pub fn ui_all_visible_painted_ms() -> u128 {
    ALL_VISIBLE.get().copied().unwrap_or(0)
}

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

/// Hard timeout for the final-layout visible-tile + min-thumbnail wait,
/// counted from process start. Override with `OPENPDF_MEASURE_TIMEOUT_MS`.
pub const DEFAULT_MEASURE_TIMEOUT_MS: u128 = 10_000;

/// [`DEFAULT_MEASURE_TIMEOUT_MS`], or a positive `OPENPDF_MEASURE_TIMEOUT_MS`.
pub fn measure_timeout_ms() -> u128 {
    parse_measure_timeout_ms(std::env::var("OPENPDF_MEASURE_TIMEOUT_MS").ok().as_deref())
}

pub fn parse_measure_timeout_ms(raw: Option<&str>) -> u128 {
    raw.and_then(|s| s.parse().ok())
        .filter(|&ms| ms > 0)
        .unwrap_or(DEFAULT_MEASURE_TIMEOUT_MS)
}

/// Stderr line for a timed-out `--measure` wait. Contains no `startup_ms`.
pub fn format_measure_timeout(
    timeout_ms: u128,
    visible_ready: usize,
    visible_total: usize,
    thumb_ready: usize,
    thumb_total: usize,
) -> String {
    format!(
        "measure timeout after {timeout_ms} ms: tiles {visible_ready}/{visible_total}, thumbs {thumb_ready}/{thumb_total}"
    )
}

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
    /// `Engine::new` / `bind_pdfium` finished, from process start.
    pub pdfium_load_ms: u128,
    /// `Engine::new` returned in `main`, from process start (font warmup continues).
    pub ui_engine_created_ms: u128,
    /// Fontconfig + Pango fontmap warmup, from process start (before `adw::init`).
    pub ui_pango_init_ms: u128,
    /// `connect_startup` entered (process + GTK .so load, before Pango warmup).
    pub ui_gtk_startup_ms: u128,
    /// SHA-256 of the `libpdfium.so` that `--measure` will bind.
    pub pdfium_sha256: String,
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
        writeln!(f, "ui_gtk_startup_ms={}", self.ui_gtk_startup_ms)?;
        writeln!(f, "ui_pango_init_ms={}", self.ui_pango_init_ms)?;
        writeln!(f, "pdfium_load_ms={}", self.pdfium_load_ms)?;
        writeln!(f, "ui_engine_created_ms={}", self.ui_engine_created_ms)?;
        writeln!(f, "pdfium_sha256={}", self.pdfium_sha256)?;
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
        if !first_tile_mark_valid(self.ui_first_tile_ms, self.ui_first_tile_rendered_ms) {
            error.get_or_insert_with(|| {
                format!(
                    "ui_first_tile_ms {} must be >0 and >= first_tile_rendered {}",
                    self.ui_first_tile_ms, self.ui_first_tile_rendered_ms
                )
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

/// Path `--measure` / `Engine` will bind, matching `OPENPDF_PDFIUM` first.
pub fn pdfium_library_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("OPENPDF_PDFIUM") {
        if !p.is_empty() {
            let path = PathBuf::from(p);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    [
        "/usr/local/lib/libpdfium.so",
        "/usr/lib/libpdfium.so",
        "/usr/lib/x86_64-linux-gnu/libpdfium.so",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|p| p.is_file())
}

/// SHA-256 (hex) of [`pdfium_library_path`], or empty if the file is missing.
pub fn pdfium_library_sha256() -> String {
    let Some(path) = pdfium_library_path() else {
        return String::new();
    };
    sha256_file(&path).unwrap_or_default()
}

fn sha256_file(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    Some(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// Load fontconfig + a Pango "Sans 11" fontset (in-process, no GTK display).
pub fn warmup_fontconfig_pango() {
    let fontmap = pangocairo::FontMap::default();
    let ctx = fontmap.create_context();
    let desc = gtk::pango::FontDescription::from_string("Sans 11");
    let _ = ctx.load_font(&desc);
    let lang = gtk::pango::Language::from_string("en-US");
    let _ = ctx.load_fontset(&desc, &lang);
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
            ..Default::default()
        }
        .to_string();
        assert!(text.contains("startup_ms=200"));
        assert!(text.contains("ui_all_visible_ms=200"));
        assert!(text.contains("ui_thumbs_done_ms=180"));
        assert!(text.contains("peak_rss_bytes=1024"));
        assert!(text.contains("ui_activate_ms=10"));
        assert!(text.contains("ui_window_present_ms=20"));
        assert!(text.contains("ui_first_frame_ms=30"));
        assert!(text.contains("ui_first_tile_ms=40"));
        assert!(text.contains("ui_pango_init_ms="));
        assert!(text.contains("pdfium_load_ms="));
        assert!(text.contains("pdfium_sha256="));
        assert!(text.contains("ui_adw_init_ms="));
        assert!(text.contains("ui_css_ms="));
        assert!(text.contains("ui_window_build_ms="));
        assert!(text.contains("ui_doc_load_start_ms="));
        assert!(text.contains("ui_first_tile_requested_ms="));
        assert!(text.contains("ui_first_surface_ready_ms="));
    }

    #[test]
    fn first_tile_ms_is_positive_and_not_before_rendered() {
        assert!(!first_tile_mark_valid(0, 209));
        assert!(!first_tile_mark_valid(0, 0));
        assert!(!first_tile_mark_valid(376, 382));
        assert!(first_tile_mark_valid(230, 209));
        assert!(first_tile_mark_valid(376, 376));
        let missing = BudgetReport {
            pages: 32,
            startup_ms: 200,
            rss_bytes: 10 * 1024 * 1024,
            visible_ready: 16,
            visible_total: 16,
            thumb_ready: 4,
            thumb_total: 4,
            ui_first_tile_ms: 0,
            ui_first_tile_rendered_ms: 209,
            ..Default::default()
        }
        .evaluate();
        assert!(!missing.ok);
        assert!(missing.error.unwrap().contains("ui_first_tile_ms"));
        let early = BudgetReport {
            pages: 32,
            startup_ms: 400,
            rss_bytes: 10 * 1024 * 1024,
            visible_ready: 16,
            visible_total: 16,
            thumb_ready: 4,
            thumb_total: 4,
            ui_first_tile_ms: 376,
            ui_first_tile_rendered_ms: 382,
            ..Default::default()
        }
        .evaluate();
        assert!(!early.ok);
        assert!(early.error.unwrap().contains("ui_first_tile_ms"));
        let ok = BudgetReport {
            pages: 32,
            startup_ms: 230,
            rss_bytes: 10 * 1024 * 1024,
            visible_ready: 16,
            visible_total: 16,
            thumb_ready: 4,
            thumb_total: 4,
            ui_first_tile_ms: 230,
            ui_first_tile_rendered_ms: 209,
            ..Default::default()
        }
        .evaluate();
        assert!(ok.ok, "{:?}", ok.error);
    }

    #[test]
    fn timeout_path_message_has_counts_and_no_startup_ms() {
        assert_eq!(parse_measure_timeout_ms(None), 10_000);
        assert_eq!(parse_measure_timeout_ms(Some("250")), 250);
        assert_eq!(parse_measure_timeout_ms(Some("0")), 10_000);
        assert_eq!(parse_measure_timeout_ms(Some("nope")), 10_000);
        assert_eq!(
            parse_measure_timeout_ms(Some("10000")),
            DEFAULT_MEASURE_TIMEOUT_MS
        );
        let msg = format_measure_timeout(10_000, 0, 16, 0, 4);
        assert_eq!(
            msg,
            "measure timeout after 10000 ms: tiles 0/16, thumbs 0/4"
        );
        assert!(!msg.contains("startup_ms"));
        assert!(!msg.contains("startup"));
        let report = BudgetReport {
            startup_ms: 10_000,
            ..Default::default()
        }
        .to_string();
        assert!(report.contains("startup_ms="));
        assert!(!msg.lines().any(|l| l.starts_with("startup_ms=")));
    }

    #[test]
    fn write_measure_pdf_is_multi_page() {
        let path = write_measure_pdf(4).unwrap();
        let doc = Document::open(&path).unwrap();
        assert_eq!(doc.page_count(), 4);
        let _ = std::fs::remove_file(path);
    }
}
