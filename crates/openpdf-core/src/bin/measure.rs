// Machine-readable open / first-tile / peak-RSS measurement.
//
// cargo run -p openpdf-core --release --bin openpdf-measure
// cargo bench -p openpdf-core --bench perf
//
// One line of key=value fields. Core times are per-step durations from
// document-open start, not process start. Never emits ui_* (GTK owns those).
// Optional budgets: OPENPDF_BENCH_MAX_OPEN_MS, OPENPDF_BENCH_MAX_TILE_MS,
// OPENPDF_BENCH_MAX_RSS_BYTES. Page count: OPENPDF_BENCH_PAGES (default 80).

use std::time::{Duration, Instant};

use openpdf_core::{
    blank_document, page_pixel_size, tile_grid, Engine, TileEvent, TileKey, TileRequest, TILE_SIZE,
};

/// GTK is still a stub. Letter page at this zoom is a 4×4 grid of 256px tiles (16),
/// matching the GUI's reported initial window for the 32-page corpus.
const GUI_INIT_ZOOM: f32 = 1.27;
const LETTER_PT: (f32, f32) = (612.0, 792.0);

fn pages() -> u32 {
    std::env::var("OPENPDF_BENCH_PAGES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(80)
}

fn budget_f64(name: &str) -> Option<f64> {
    std::env::var(name).ok().and_then(|s| s.parse().ok())
}

fn budget_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|s| s.parse().ok())
}

fn peak_rss_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            let Some(rest) = line.strip_prefix("VmHWM:") else {
                continue;
            };
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb.saturating_mul(1024));
        }
        None
    }
    #[cfg(target_os = "macos")]
    {
        unsafe {
            let mut usage: libc::rusage = std::mem::zeroed();
            if libc::getrusage(libc::RUSAGE_SELF, &mut usage) == 0 {
                Some(usage.ru_maxrss as u64)
            } else {
                None
            }
        }
    }
    #[cfg(windows)]
    {
        windows_peak_working_set()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        None
    }
}

#[cfg(windows)]
fn windows_peak_working_set() -> Option<u64> {
    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    extern "system" {
        fn GetCurrentProcess() -> *mut core::ffi::c_void;
        fn K32GetProcessMemoryInfo(
            process: *mut core::ffi::c_void,
            counters: *mut ProcessMemoryCounters,
            size: u32,
        ) -> i32;
    }
    unsafe {
        let mut pmc = std::mem::zeroed::<ProcessMemoryCounters>();
        pmc.cb = std::mem::size_of::<ProcessMemoryCounters>() as u32;
        if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, pmc.cb) != 0 {
            Some(pmc.peak_working_set_size as u64)
        } else {
            None
        }
    }
}

fn help_text() -> &'static str {
    "\
openpdf-measure — machine-readable core startup timings

Prints one stdout line of key=value fields. Core times are per-step
durations measured from document-open start, not process start
(process/CRT startup is excluded). GTK owns keys with the ui prefix;
this binary never emits them.

Keys:
  version os arch pages
  open_ms            lopdf Document::from_bytes
  first_tile_ms      first render_tile_blocking (after Engine::new + bind)
  peak_rss_bytes
  pdfium_load_ms     dlopen + FPDF_InitLibrary
  doc_open_ms        PDFium document open (Engine::bind)
  convert_ms         PDFium buffer copy after the first tile (no BGRA swizzle)
  visible_tiles      tiles in the GUI initial view (letter @ zoom 1.27 = 16)
  first_ready_ms     first TileEvent::Ready after requesting those tiles
  first_ready_x first_ready_y
  visible_tiles_ms   time until every visible tile is Ready

Budgets (optional env, fail if exceeded):
  OPENPDF_BENCH_MAX_OPEN_MS  OPENPDF_BENCH_MAX_TILE_MS
  OPENPDF_BENCH_MAX_RSS_BYTES
Page count: OPENPDF_BENCH_PAGES (default 80).
"
}

fn format_line(os: &str, arch: &str, fields: &MeasureFields<'_>) -> String {
    format!(
        "openpdf_measure version=1 os={os} arch={arch} pages={n} open_ms={open_ms:.3} first_tile_ms={first_tile_ms:.3} peak_rss_bytes={rss} pdfium_load_ms={pdfium_load_ms:.3} doc_open_ms={doc_open_ms:.3} convert_ms={convert_ms:.3} visible_tiles={vt} first_ready_ms={first_ready_ms:.3} first_ready_x={fx} first_ready_y={fy} visible_tiles_ms={visible_tiles_ms:.3}",
        n = fields.pages,
        open_ms = fields.open_ms,
        first_tile_ms = fields.first_tile_ms,
        rss = fields.rss,
        pdfium_load_ms = fields.pdfium_load_ms,
        doc_open_ms = fields.doc_open_ms,
        convert_ms = fields.convert_ms,
        vt = fields.visible_tiles,
        first_ready_ms = fields.first_ready_ms,
        fx = fields.first_ready_x,
        fy = fields.first_ready_y,
        visible_tiles_ms = fields.visible_tiles_ms,
    )
}

struct MeasureFields<'a> {
    pages: u32,
    open_ms: f64,
    first_tile_ms: f64,
    rss: &'a str,
    pdfium_load_ms: f64,
    doc_open_ms: f64,
    convert_ms: f64,
    visible_tiles: u32,
    first_ready_ms: f64,
    first_ready_x: i32,
    first_ready_y: i32,
    visible_tiles_ms: f64,
}

fn gui_visible_keys(doc: openpdf_core::DocumentId) -> Vec<TileKey> {
    let (pw, ph) = page_pixel_size(LETTER_PT.0, LETTER_PT.1, GUI_INIT_ZOOM);
    let (cols, rows) = tile_grid(pw, ph, TILE_SIZE);
    debug_assert_eq!(
        (cols, rows),
        (4, 4),
        "GUI initial view should be 16 tiles, got {cols}x{rows} at zoom {GUI_INIT_ZOOM} ({pw}x{ph})"
    );
    let mut keys = Vec::with_capacity((cols * rows) as usize);
    for y in 0..rows {
        for x in 0..cols {
            keys.push(TileKey::new(doc, 0, GUI_INIT_ZOOM, x, y));
        }
    }
    keys
}

fn measure_visible_queue(
    engine: &Engine,
    doc: openpdf_core::DocumentId,
) -> (u32, f64, i32, i32, f64, f64) {
    while engine.try_recv().is_some() {}
    let _ = engine.begin_frame();
    let keys = gui_visible_keys(doc);
    let n = keys.len() as u32;
    let reqs: Vec<TileRequest> = keys.into_iter().map(TileRequest::visible).collect();
    let t0 = Instant::now();
    engine.request_tiles(reqs).expect("request_tiles");
    let mut first_ready_ms = -1.0;
    let mut convert_ms = -1.0;
    let mut fx = -1;
    let mut fy = -1;
    let mut got = 0u32;
    let deadline = Instant::now() + Duration::from_secs(5);
    while got < n && Instant::now() < deadline {
        if let Some(TileEvent::Ready(tile)) = engine.recv_timeout(Duration::from_millis(200)) {
            if first_ready_ms < 0.0 {
                first_ready_ms = t0.elapsed().as_secs_f64() * 1000.0;
                convert_ms = engine.last_convert_ms();
                fx = tile.key.x as i32;
                fy = tile.key.y as i32;
            }
            got += 1;
        }
    }
    let visible_tiles_ms = t0.elapsed().as_secs_f64() * 1000.0;
    debug_assert_eq!(got, n, "visible tile queue delivered {got}/{n}");
    (n, first_ready_ms, fx, fy, visible_tiles_ms, convert_ms)
}

fn main() {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        print!("{}", help_text());
        return;
    }
    openpdf_core::init();
    let n = pages();
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let bytes = {
        let t0 = Instant::now();
        let doc = blank_document(n).expect("blank_document");
        let _build_ms = t0.elapsed().as_secs_f64() * 1000.0;
        doc.to_bytes().expect("to_bytes")
    };

    let t1 = Instant::now();
    let opened = openpdf_core::Document::from_bytes(bytes, None).expect("open");
    let open_ms = t1.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(opened.page_count(), n);

    let (
        first_tile_ms,
        tile_ok,
        pdfium_load_ms,
        doc_open_ms,
        convert_ms,
        visible_tiles,
        first_ready_ms,
        first_ready_x,
        first_ready_y,
        visible_tiles_ms,
    ) = match Engine::new() {
        Ok(engine) => {
            let pdfium_load_ms = engine.pdfium_load_ms();
            let t_doc = Instant::now();
            engine.bind(&opened).expect("bind");
            let doc_open_ms = t_doc.elapsed().as_secs_f64() * 1000.0;
            let (
                visible_tiles,
                first_ready_ms,
                first_ready_x,
                first_ready_y,
                visible_tiles_ms,
                convert_ms,
            ) = measure_visible_queue(&engine, opened.id());
            let key = TileKey::new(opened.id(), 0, 1.0, 0, 0);
            let t2 = Instant::now();
            let tile = engine.render_tile_blocking(key).expect("tile");
            let ms = t2.elapsed().as_secs_f64() * 1000.0;
            assert!(tile.width > 0 && tile.height > 0);
            (
                ms,
                true,
                pdfium_load_ms,
                doc_open_ms,
                convert_ms,
                visible_tiles,
                first_ready_ms,
                first_ready_x,
                first_ready_y,
                visible_tiles_ms,
            )
        }
        Err(e) => {
            if std::env::var("CI").is_ok() {
                panic!("PDFium required in CI: {e}");
            }
            (-1.0, false, -1.0, -1.0, -1.0, 0, -1.0, -1, -1, -1.0)
        }
    };

    let rss = peak_rss_bytes();
    let rss_s = rss.map(|v| v.to_string()).unwrap_or_else(|| "na".into());

    let line = format_line(
        os,
        arch,
        &MeasureFields {
            pages: n,
            open_ms,
            first_tile_ms,
            rss: &rss_s,
            pdfium_load_ms,
            doc_open_ms,
            convert_ms,
            visible_tiles,
            first_ready_ms,
            first_ready_x,
            first_ready_y,
            visible_tiles_ms,
        },
    );
    debug_assert!(
        !line.split_whitespace().any(|f| f.starts_with("ui_")),
        "core must never emit GTK ui-prefixed keys"
    );
    println!("{line}");

    let mut failed = false;
    if let Some(max) = budget_f64("OPENPDF_BENCH_MAX_OPEN_MS") {
        if open_ms > max {
            eprintln!("open_ms {open_ms:.3} exceeds OPENPDF_BENCH_MAX_OPEN_MS={max}");
            failed = true;
        }
    }
    if tile_ok {
        if let Some(max) = budget_f64("OPENPDF_BENCH_MAX_TILE_MS") {
            if first_tile_ms > max {
                eprintln!(
                    "first_tile_ms {first_tile_ms:.3} exceeds OPENPDF_BENCH_MAX_TILE_MS={max}"
                );
                failed = true;
            }
        }
    }
    if let (Some(rss), Some(max)) = (rss, budget_u64("OPENPDF_BENCH_MAX_RSS_BYTES")) {
        if rss > max {
            eprintln!("peak_rss_bytes {rss} exceeds OPENPDF_BENCH_MAX_RSS_BYTES={max}");
            failed = true;
        }
    }
    if failed {
        std::process::exit(1);
    }
}
