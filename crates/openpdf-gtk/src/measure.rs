// SPDX-License-Identifier: MPL-2.0
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
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
    /// Host fontconfig file (FONTCONFIG_FILE or FcConfigFilename).
    pub fontconfig_file: String,
    /// `FcFontList` count (empty pattern, `file` objects).
    pub fontconfig_fonts: i32,
    pub gtk_icon_theme: String,
    pub gtk_icon_search_path: String,
    pub xdg_cache_home: String,
    pub xdg_data_dirs: String,
    pub xdg_data_home: String,
    pub xdg_config_home: String,
    pub fontconfig_file_env: String,
    pub fontconfig_path_env: String,
    pub gtk_env: String,
    /// `GSK_RENDERER` environment variable (may be empty).
    pub gsk_renderer_env: String,
    /// Realized `GskRenderer` GType name (`GskCairoRenderer` / `GskNglRenderer` / …).
    pub gsk_renderer: String,
    /// `GL_RENDERER` from an already-created `GdkGLContext`, or `none`.
    pub gl_renderer: String,
    /// `gtk_get_{major,minor,micro}_version`.
    pub gtk_version: String,
    /// Variables `--measure` code itself set. Must be empty.
    pub env_overrides: Vec<String>,
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
        writeln!(f, "fontconfig_file={}", self.fontconfig_file)?;
        writeln!(f, "fontconfig_fonts={}", self.fontconfig_fonts)?;
        writeln!(f, "gtk_icon_theme={}", self.gtk_icon_theme)?;
        writeln!(f, "gtk_icon_search_path={}", self.gtk_icon_search_path)?;
        writeln!(f, "XDG_CACHE_HOME={}", self.xdg_cache_home)?;
        writeln!(f, "XDG_DATA_DIRS={}", self.xdg_data_dirs)?;
        writeln!(f, "XDG_DATA_HOME={}", self.xdg_data_home)?;
        writeln!(f, "XDG_CONFIG_HOME={}", self.xdg_config_home)?;
        writeln!(f, "FONTCONFIG_FILE={}", self.fontconfig_file_env)?;
        writeln!(f, "FONTCONFIG_PATH={}", self.fontconfig_path_env)?;
        write!(f, "{}", self.gtk_env)?;
        writeln!(f, "GSK_RENDERER={}", self.gsk_renderer_env)?;
        writeln!(f, "gsk_renderer={}", or_none(&self.gsk_renderer))?;
        writeln!(f, "gl_renderer={}", or_none(&self.gl_renderer))?;
        writeln!(f, "gtk_version={}", self.gtk_version)?;
        writeln!(f, "env_overrides={}", self.env_overrides.join(","))?;
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
        if !self.env_overrides.is_empty() {
            error.get_or_insert_with(|| {
                format!(
                    "measure set env vars the app path does not: {}",
                    self.env_overrides.join(",")
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

/// Opt-in `--measure` RSS dump. Any value except empty / `0` / `false` / `off`.
pub fn rss_breakdown_enabled() -> bool {
    match std::env::var("OPENPDF_MEASURE_RSS_BREAKDOWN") {
        Ok(v) => {
            let v = v.trim();
            !v.is_empty()
                && v != "0"
                && !v.eq_ignore_ascii_case("false")
                && !v.eq_ignore_ascii_case("off")
        }
        Err(_) => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RssClass {
    Heap,
    Pdfium,
    Gtk,
    Adw,
    Icon,
    Font,
    Cairo,
    Other,
}

impl RssClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Heap => "heap",
            Self::Pdfium => "libpdfium",
            Self::Gtk => "gtk",
            Self::Adw => "adw",
            Self::Icon => "icon",
            Self::Font => "fontconfig",
            Self::Cairo => "cairo",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmapsMapping {
    pub pathname: String,
    pub rss_kb: u64,
    pub pss_kb: u64,
    pub class: RssClass,
}

/// Classify a `/proc/self/smaps` pathname into the host-delta buckets.
pub fn classify_smaps_path(pathname: &str) -> RssClass {
    let p = pathname.to_ascii_lowercase();
    if p.contains("pdfium") {
        return RssClass::Pdfium;
    }
    if p.contains("/icons/")
        || p.contains("icon-theme")
        || p.contains("icon_theme")
        || p.contains(".icon-cache")
        || p.contains("icon-theme.cache")
    {
        return RssClass::Icon;
    }
    if p.contains("fontconfig")
        || p.contains("/fonts/")
        || p.ends_with(".ttf")
        || p.ends_with(".otf")
        || p.ends_with(".ttc")
        || p.ends_with(".pfb")
        || p.contains("libpango")
        || p.contains("libharfbuzz")
        || p.contains("libfreetype")
    {
        return RssClass::Font;
    }
    if p.contains("libcairo") || p.contains("libpixman") {
        return RssClass::Cairo;
    }
    if p.contains("libadwaita") {
        return RssClass::Adw;
    }
    if p.contains("libgtk")
        || p.contains("libgdk-4")
        || p.contains("libgdk_pixbuf")
        || p.contains("libgsk")
        || p.contains("libgraphene")
    {
        return RssClass::Gtk;
    }
    if pathname.is_empty()
        || pathname == "[heap]"
        || pathname == "[stack]"
        || pathname.starts_with("[anon")
        || pathname.starts_with("[anon:")
        || (pathname.starts_with('[') && !pathname.contains('/'))
    {
        return RssClass::Heap;
    }
    RssClass::Other
}

pub fn parse_smaps_rollup(text: &str) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        if key.eq_ignore_ascii_case("VmFlags") {
            continue;
        }
        let kb = rest
            .split_whitespace()
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        out.push((key.to_string(), kb));
    }
    out
}

pub fn parse_smaps(text: &str) -> Vec<SmapsMapping> {
    let mut maps = Vec::new();
    let mut pathname = String::new();
    let mut rss_kb = 0u64;
    let mut pss_kb = 0u64;
    let mut in_map = false;

    let flush = |pathname: &mut String,
                 rss_kb: &mut u64,
                 pss_kb: &mut u64,
                 maps: &mut Vec<SmapsMapping>| {
        if pathname.is_empty() && *rss_kb == 0 && *pss_kb == 0 {
            return;
        }
        let class = classify_smaps_path(pathname);
        maps.push(SmapsMapping {
            pathname: std::mem::take(pathname),
            rss_kb: *rss_kb,
            pss_kb: *pss_kb,
            class,
        });
        *rss_kb = 0;
        *pss_kb = 0;
    };

    for line in text.lines() {
        if is_smaps_header(line) {
            if in_map {
                flush(&mut pathname, &mut rss_kb, &mut pss_kb, &mut maps);
            }
            pathname = smaps_pathname(line);
            rss_kb = 0;
            pss_kb = 0;
            in_map = true;
            continue;
        }
        if !in_map {
            continue;
        }
        if let Some(rest) = line.strip_prefix("Rss:") {
            rss_kb = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("Pss:") {
            pss_kb = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
        }
    }
    if in_map {
        flush(&mut pathname, &mut rss_kb, &mut pss_kb, &mut maps);
    }
    maps
}

fn is_smaps_header(line: &str) -> bool {
    let Some((range, _)) = line.split_once(' ') else {
        return false;
    };
    range.contains('-') && !line.contains(" kB") && !line.starts_with("VmFlags:")
}

fn smaps_pathname(header: &str) -> String {
    let mut parts = header.split_whitespace();
    // address perms offset dev inode [pathname...]
    for _ in 0..5 {
        if parts.next().is_none() {
            return String::new();
        }
    }
    parts.collect::<Vec<_>>().join(" ")
}

/// Coalesce same-class / same-path RSS, then take the top `n` by Rss.
pub fn top_smaps_mappings(maps: &[SmapsMapping], n: usize) -> Vec<SmapsMapping> {
    let mut by_path: std::collections::BTreeMap<String, SmapsMapping> =
        std::collections::BTreeMap::new();
    for m in maps {
        let key = if m.pathname.is_empty() {
            format!("anon:{}", m.class.as_str())
        } else {
            m.pathname.clone()
        };
        by_path
            .entry(key)
            .and_modify(|e| {
                e.rss_kb = e.rss_kb.saturating_add(m.rss_kb);
                e.pss_kb = e.pss_kb.saturating_add(m.pss_kb);
            })
            .or_insert_with(|| m.clone());
    }
    let mut merged: Vec<SmapsMapping> = by_path.into_values().collect();
    merged.sort_by(|a, b| {
        b.rss_kb
            .cmp(&a.rss_kb)
            .then_with(|| a.pathname.cmp(&b.pathname))
    });
    merged.truncate(n);
    merged
}

pub fn class_rss_kb(maps: &[SmapsMapping]) -> [(RssClass, u64); 8] {
    let mut out = [
        (RssClass::Heap, 0u64),
        (RssClass::Pdfium, 0),
        (RssClass::Gtk, 0),
        (RssClass::Adw, 0),
        (RssClass::Icon, 0),
        (RssClass::Font, 0),
        (RssClass::Cairo, 0),
        (RssClass::Other, 0),
    ];
    for m in maps {
        let slot = match m.class {
            RssClass::Heap => 0,
            RssClass::Pdfium => 1,
            RssClass::Gtk => 2,
            RssClass::Adw => 3,
            RssClass::Icon => 4,
            RssClass::Font => 5,
            RssClass::Cairo => 6,
            RssClass::Other => 7,
        };
        out[slot].1 = out[slot].1.saturating_add(m.rss_kb);
    }
    out
}

fn read_proc(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Snapshot `/proc/self/smaps_rollup` + top 15 mappings (merged by path).
pub fn format_rss_breakdown() -> String {
    format_rss_breakdown_from(
        &read_proc("/proc/self/smaps_rollup"),
        &read_proc("/proc/self/smaps"),
    )
}

pub fn format_rss_breakdown_from(rollup: &str, smaps: &str) -> String {
    let mut out = String::from("rss_breakdown=1\n");
    for (key, kb) in parse_smaps_rollup(rollup) {
        let slug = key
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect::<String>();
        out.push_str(&format!("smaps_rollup_{slug}_kb={kb}\n"));
    }
    let maps = parse_smaps(smaps);
    for (class, kb) in class_rss_kb(&maps) {
        out.push_str(&format!("rss_class_{}_kb={kb}\n", class.as_str()));
    }
    for (i, m) in top_smaps_mappings(&maps, 15).into_iter().enumerate() {
        let path = if m.pathname.is_empty() {
            "[anon]".to_string()
        } else {
            m.pathname
        };
        out.push_str(&format!(
            "rss_map_{}={} {} {}\n",
            i + 1,
            m.class.as_str(),
            m.rss_kb,
            path
        ));
    }
    out
}

static RSS_BREAKDOWN: OnceLock<String> = OnceLock::new();

/// Capture the peak-time smaps dump once (first call wins).
pub fn capture_rss_breakdown_if_enabled() {
    if !rss_breakdown_enabled() {
        return;
    }
    let _ = RSS_BREAKDOWN.set(format_rss_breakdown());
}

pub fn take_rss_breakdown() -> Option<String> {
    RSS_BREAKDOWN.get().cloned()
}

/// Env keys `--measure` must not invent. Same process env as a normal run.
pub const MEASURE_FORBIDDEN_ENV: &[&str] = &[
    "FONTCONFIG_FILE",
    "FONTCONFIG_PATH",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_DATA_DIRS",
    "XDG_STATE_HOME",
    "GTK_THEME",
    "GTK_ICON_THEME_NAME",
    "GTK_A11Y",
    "PANGO_LANGUAGE",
    "OPENPDF_TILE_CACHE_BYTES",
    "GSK_RENDERER",
];

static INHERITED_ENV: OnceLock<BTreeMap<String, Option<String>>> = OnceLock::new();
static MEASURE_SET_VARS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn is_watched_env(key: &str) -> bool {
    MEASURE_FORBIDDEN_ENV.contains(&key)
        || key.starts_with("XDG_")
        || key.starts_with("FONTCONFIG_")
        || key.starts_with("GTK_")
}

/// Pin the process env `--measure` inherited. First call wins (from `main`).
pub fn remember_inherited_env() {
    let mut map = BTreeMap::new();
    for key in MEASURE_FORBIDDEN_ENV {
        map.insert((*key).to_string(), std::env::var(key).ok());
    }
    for (key, val) in std::env::vars() {
        if is_watched_env(&key) {
            map.insert(key, Some(val));
        }
    }
    let _ = INHERITED_ENV.set(map);
}

/// Record a var set by measure code. The live `--measure` path must never call this.
pub fn measure_set_var(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    MEASURE_SET_VARS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(key.to_string());
    std::env::set_var(key, value);
}

/// Variables measure code set, plus watched keys that changed since
/// [`remember_inherited_env`]. Must be empty for a valid `--measure` run.
pub fn measure_env_overrides() -> Vec<String> {
    remember_inherited_env();
    let mut out = MEASURE_SET_VARS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    if let Some(start) = INHERITED_ENV.get() {
        for key in MEASURE_FORBIDDEN_ENV {
            let old = start.get(*key).cloned().flatten();
            if std::env::var(key).ok() != old {
                out.push((*key).to_string());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `--measure` must not set FONTCONFIG_*/XDG_*/GTK icon search env.
pub fn measure_env_is_clean() -> bool {
    !measure_env_overrides().iter().any(|k| is_watched_env(k))
}

fn env_or_empty(key: &str) -> String {
    std::env::var(key).unwrap_or_default()
}

fn or_none(value: &str) -> &str {
    if value.is_empty() {
        "none"
    } else {
        value
    }
}

/// `gtk_get_major/minor/micro_version` as `major.minor.micro`.
pub fn gtk_version_string() -> String {
    format!(
        "{}.{}.{}",
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version()
    )
}

/// GType name of the realized window's `GskRenderer`, or empty if none.
pub fn realized_gsk_renderer_name(widget: &impl gtk::prelude::IsA<gtk::Widget>) -> String {
    use gtk::prelude::{NativeExt, ObjectExt, WidgetExt};
    widget
        .as_ref()
        .native()
        .and_then(|n| n.renderer())
        .map(|r| r.type_().name().to_string())
        .unwrap_or_default()
}

/// `GL_RENDERER` from `GdkGLContext::current()` only. Never creates a context.
pub fn existing_gl_renderer() -> String {
    if gtk::gdk::GLContext::current().is_none() {
        return String::new();
    }
    gl_renderer_string().unwrap_or_default()
}

const GL_RENDERER: u32 = 0x1F01;

#[link(name = "epoxy")]
extern "C" {
    fn epoxy_glGetString(name: u32) -> *const std::ffi::c_uchar;
}

fn gl_renderer_string() -> Option<String> {
    unsafe {
        let ptr = epoxy_glGetString(GL_RENDERER);
        if ptr.is_null() {
            return None;
        }
        Some(
            std::ffi::CStr::from_ptr(ptr.cast())
                .to_string_lossy()
                .into_owned(),
        )
    }
}

fn gtk_star_env_block() -> String {
    let mut keys: Vec<String> = std::env::vars()
        .map(|(k, _)| k)
        .filter(|k| k.starts_with("GTK_"))
        .collect();
    keys.sort();
    let mut out = String::new();
    for key in keys {
        out.push_str(&format!("{key}={}\n", env_or_empty(&key)));
    }
    out
}

/// Snapshot fontconfig + GTK icon theme + watched env (GTK thread after init).
pub fn collect_runtime_env() -> (
    String,
    i32,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Vec<String>,
) {
    let (theme, paths) = gtk_icon_theme_report();
    (
        fontconfig_config_file(),
        fontconfig_font_list_count(),
        theme,
        paths,
        env_or_empty("XDG_CACHE_HOME"),
        env_or_empty("XDG_DATA_DIRS"),
        env_or_empty("XDG_DATA_HOME"),
        env_or_empty("XDG_CONFIG_HOME"),
        env_or_empty("FONTCONFIG_FILE"),
        env_or_empty("FONTCONFIG_PATH"),
        gtk_star_env_block(),
        env_or_empty("GSK_RENDERER"),
        gtk_version_string(),
        measure_env_overrides(),
    )
}

fn gtk_icon_theme_report() -> (String, String) {
    let Some(display) = gtk::gdk::Display::default() else {
        return (String::new(), String::new());
    };
    let theme = gtk::IconTheme::for_display(&display);
    let name = theme.theme_name().to_string();
    let paths = theme
        .search_path()
        .into_iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(":");
    (name, paths)
}

#[repr(C)]
struct FcFontSet {
    nfont: i32,
    _sfont: i32,
    _fonts: *mut *mut std::ffi::c_void,
}

#[link(name = "fontconfig")]
extern "C" {
    fn FcInit() -> i32;
    fn FcConfigFilename(name: *const std::ffi::c_char) -> *mut std::ffi::c_char;
    fn FcStrFree(s: *mut std::ffi::c_char);
    fn FcPatternCreate() -> *mut std::ffi::c_void;
    fn FcPatternDestroy(p: *mut std::ffi::c_void);
    fn FcObjectSetCreate() -> *mut std::ffi::c_void;
    fn FcObjectSetAdd(os: *mut std::ffi::c_void, object: *const std::ffi::c_char) -> i32;
    fn FcObjectSetDestroy(os: *mut std::ffi::c_void);
    fn FcFontList(
        config: *mut std::ffi::c_void,
        p: *mut std::ffi::c_void,
        os: *mut std::ffi::c_void,
    ) -> *mut FcFontSet;
    fn FcFontSetDestroy(s: *mut FcFontSet);
}

/// FONTCONFIG_FILE, else `FcConfigFilename(NULL)`.
pub fn fontconfig_config_file() -> String {
    if let Ok(p) = std::env::var("FONTCONFIG_FILE") {
        if !p.is_empty() {
            return p;
        }
    }
    unsafe {
        FcInit();
        let ptr = FcConfigFilename(std::ptr::null());
        if ptr.is_null() {
            return String::new();
        }
        let s = std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned();
        FcStrFree(ptr);
        s
    }
}

/// `FcFontList` count with an empty pattern (one entry per font file).
pub fn fontconfig_font_list_count() -> i32 {
    unsafe {
        FcInit();
        let pat = FcPatternCreate();
        let os = FcObjectSetCreate();
        let file = std::ffi::CString::new("file").expect("fc object");
        FcObjectSetAdd(os, file.as_ptr());
        let set = FcFontList(std::ptr::null_mut(), pat, os);
        let n = if set.is_null() { 0 } else { (*set).nfont };
        if !set.is_null() {
            FcFontSetDestroy(set);
        }
        FcObjectSetDestroy(os);
        FcPatternDestroy(pat);
        n
    }
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

impl BudgetReport {
    /// Fill fontconfig / GTK / XDG lines and `env_overrides` from the live process.
    pub fn with_runtime_env(mut self) -> Self {
        let (
            fontconfig_file,
            fontconfig_fonts,
            gtk_icon_theme,
            gtk_icon_search_path,
            xdg_cache_home,
            xdg_data_dirs,
            xdg_data_home,
            xdg_config_home,
            fontconfig_file_env,
            fontconfig_path_env,
            gtk_env,
            gsk_renderer_env,
            gtk_version,
            env_overrides,
        ) = collect_runtime_env();
        self.fontconfig_file = fontconfig_file;
        self.fontconfig_fonts = fontconfig_fonts;
        self.gtk_icon_theme = gtk_icon_theme;
        self.gtk_icon_search_path = gtk_icon_search_path;
        self.xdg_cache_home = xdg_cache_home;
        self.xdg_data_dirs = xdg_data_dirs;
        self.xdg_data_home = xdg_data_home;
        self.xdg_config_home = xdg_config_home;
        self.fontconfig_file_env = fontconfig_file_env;
        self.fontconfig_path_env = fontconfig_path_env;
        self.gtk_env = gtk_env;
        self.gsk_renderer_env = gsk_renderer_env;
        self.gtk_version = gtk_version;
        self.env_overrides = env_overrides;
        self
    }

    /// Fill realized `gsk_renderer` / `gl_renderer` from a mapped window.
    /// Does not create a GL context.
    pub fn with_window_graphics(mut self, widget: &impl gtk::prelude::IsA<gtk::Widget>) -> Self {
        self.gsk_renderer = realized_gsk_renderer_name(widget);
        self.gl_renderer = existing_gl_renderer();
        self
    }
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
        assert!(text.contains("env_overrides="));
        assert!(text.contains("fontconfig_fonts="));
        assert!(text.contains("gtk_icon_theme="));
        assert!(text.contains("gsk_renderer=none"));
        assert!(text.contains("gl_renderer=none"));
        assert!(text.contains("gtk_version="));
        let named = BudgetReport {
            gsk_renderer: "GskCairoRenderer".into(),
            gl_renderer: "llvmpipe".into(),
            gtk_version: "4.14.5".into(),
            ..Default::default()
        }
        .to_string();
        assert!(named.contains("gsk_renderer=GskCairoRenderer"));
        assert!(named.contains("gl_renderer=llvmpipe"));
        assert!(named.contains("gtk_version=4.14.5"));
    }

    #[test]
    fn gtk_version_string_is_dotted_triple() {
        let v = gtk_version_string();
        let parts: Vec<&str> = v.split('.').collect();
        assert_eq!(parts.len(), 3, "{v}");
        assert!(
            parts
                .iter()
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
            "{v}"
        );
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
    fn measure_setup_does_not_set_fontconfig_xdg_or_icon_env() {
        remember_inherited_env();
        let before: Vec<(&str, Option<String>)> = MEASURE_FORBIDDEN_ENV
            .iter()
            .map(|k| (*k, std::env::var(k).ok()))
            .collect();
        assert!(
            measure_env_is_clean(),
            "measure must not set env: {:?}",
            measure_env_overrides()
        );
        assert!(measure_env_overrides().is_empty());
        for (key, old) in before {
            assert_eq!(
                std::env::var(key).ok(),
                old,
                "measure must not change {key}"
            );
        }
        for key in [
            "FONTCONFIG_FILE",
            "FONTCONFIG_PATH",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
            "XDG_DATA_DIRS",
            "XDG_CONFIG_HOME",
        ] {
            assert!(
                !measure_env_overrides().iter().any(|k| k == key),
                "{key} must stay inherited"
            );
        }
    }

    #[test]
    fn env_overrides_fail_the_report_and_print() {
        let report = BudgetReport {
            pages: 32,
            startup_ms: 200,
            rss_bytes: 10 * 1024 * 1024,
            visible_ready: 16,
            visible_total: 16,
            thumb_ready: 4,
            thumb_total: 4,
            ui_first_tile_ms: 230,
            ui_first_tile_rendered_ms: 209,
            env_overrides: vec!["FONTCONFIG_FILE".into(), "XDG_CACHE_HOME".into()],
            ..Default::default()
        }
        .evaluate();
        assert!(!report.ok);
        let err = report.error.unwrap();
        assert!(err.contains("FONTCONFIG_FILE"), "{err}");
        assert!(err.contains("XDG_CACHE_HOME"), "{err}");
        let text = BudgetReport {
            env_overrides: vec!["FONTCONFIG_FILE".into()],
            ..Default::default()
        }
        .to_string();
        assert!(text.contains("env_overrides=FONTCONFIG_FILE"));
        let clean = BudgetReport {
            pages: 32,
            startup_ms: 230,
            rss_bytes: 10 * 1024 * 1024,
            visible_ready: 16,
            visible_total: 16,
            thumb_ready: 4,
            thumb_total: 4,
            ui_first_tile_ms: 230,
            ui_first_tile_rendered_ms: 209,
            env_overrides: Vec::new(),
            ..Default::default()
        }
        .evaluate();
        assert!(clean.ok, "{:?}", clean.error);
    }

    #[test]
    fn write_measure_pdf_is_multi_page() {
        let path = write_measure_pdf(4).unwrap();
        let doc = Document::open(&path).unwrap();
        assert_eq!(doc.page_count(), 4);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn classify_smaps_buckets() {
        assert_eq!(classify_smaps_path("[heap]"), RssClass::Heap);
        assert_eq!(classify_smaps_path(""), RssClass::Heap);
        assert_eq!(classify_smaps_path("[anon:cairo]"), RssClass::Heap);
        assert_eq!(
            classify_smaps_path("/usr/local/lib/libpdfium.so"),
            RssClass::Pdfium
        );
        assert_eq!(
            classify_smaps_path("/usr/lib/x86_64-linux-gnu/libgtk-4.so.1"),
            RssClass::Gtk
        );
        assert_eq!(
            classify_smaps_path("/usr/lib/x86_64-linux-gnu/libadwaita-1.so.0"),
            RssClass::Adw
        );
        assert_eq!(
            classify_smaps_path("/usr/share/icons/Adwaita/icon-theme.cache"),
            RssClass::Icon
        );
        assert_eq!(
            classify_smaps_path("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
            RssClass::Font
        );
        assert_eq!(
            classify_smaps_path("/usr/lib/x86_64-linux-gnu/libcairo.so.2"),
            RssClass::Cairo
        );
        assert_eq!(
            classify_smaps_path("/usr/lib/x86_64-linux-gnu/libc.so.6"),
            RssClass::Other
        );
    }

    #[test]
    fn parse_smaps_and_format_top15() {
        let rollup = "Rss:                4096 kB\nPss:                3000 kB\nPrivate_Dirty:      1024 kB\n";
        let smaps = "\
7f0000000000-7f0000100000 rw-p 00000000 00:00 0                          [heap]
Size:               1024 kB
Rss:                 512 kB
Pss:                 512 kB
7f1000000000-7f1000200000 r-xp 00000000 08:01 1                          /usr/local/lib/libpdfium.so
Rss:                2048 kB
Pss:                2048 kB
7f2000000000-7f2000010000 r--p 00000000 08:01 2                          /usr/share/icons/Adwaita/icon-theme.cache
Rss:                 256 kB
Pss:                  64 kB
7f3000000000-7f3000010000 r-xp 00000000 08:01 3                          /usr/lib/x86_64-linux-gnu/libgtk-4.so.1
Rss:                 128 kB
Pss:                  32 kB
";
        let maps = parse_smaps(smaps);
        assert_eq!(maps.len(), 4);
        assert_eq!(maps[0].class, RssClass::Heap);
        assert_eq!(maps[0].rss_kb, 512);
        assert_eq!(maps[1].class, RssClass::Pdfium);
        let classes = class_rss_kb(&maps);
        assert_eq!(classes[1], (RssClass::Pdfium, 2048));
        assert_eq!(classes[4], (RssClass::Icon, 256));
        let text = format_rss_breakdown_from(rollup, smaps);
        assert!(text.contains("rss_breakdown=1"));
        assert!(text.contains("smaps_rollup_rss_kb=4096"));
        assert!(text.contains("rss_class_libpdfium_kb=2048"));
        assert!(text.contains("rss_class_heap_kb=512"));
        assert!(text.contains("rss_class_icon_kb=256"));
        assert!(text.contains("rss_class_gtk_kb=128"));
        assert!(text.contains("rss_map_1=libpdfium 2048 /usr/local/lib/libpdfium.so"));
        assert!(text.contains("rss_map_2=heap 512 [heap]"));
        let top = top_smaps_mappings(&maps, 15);
        assert_eq!(top[0].pathname, "/usr/local/lib/libpdfium.so");
        assert_eq!(top.len(), 4);
    }
}
