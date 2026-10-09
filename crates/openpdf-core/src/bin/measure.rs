//! Machine-readable open / first-tile / peak-RSS measurement.
//!
//! ```text
//! cargo run -p openpdf-core --release --bin openpdf-measure
//! cargo bench -p openpdf-core --bench perf
//! ```
//!
//! One line of `key=value` fields. Optional budgets (milliseconds / bytes) fail
//! the process when exceeded:
//! `OPENPDF_BENCH_MAX_OPEN_MS`, `OPENPDF_BENCH_MAX_TILE_MS`,
//! `OPENPDF_BENCH_MAX_RSS_BYTES`. Page count: `OPENPDF_BENCH_PAGES` (default 80).

use std::time::Instant;

use openpdf_core::{blank_document, Engine, TileKey};

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

fn main() {
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

    let (first_tile_ms, tile_ok) = match Engine::new() {
        Ok(engine) => {
            engine.bind(&opened).expect("bind");
            let key = TileKey::new(opened.id(), 0, 1.0, 0, 0);
            let t2 = Instant::now();
            let tile = engine.render_tile_blocking(key).expect("tile");
            let ms = t2.elapsed().as_secs_f64() * 1000.0;
            assert!(tile.width > 0 && tile.height > 0);
            (ms, true)
        }
        Err(e) => {
            if std::env::var("CI").is_ok() {
                panic!("PDFium required in CI: {e}");
            }
            (-1.0, false)
        }
    };

    let rss = peak_rss_bytes();
    let rss_s = rss.map(|v| v.to_string()).unwrap_or_else(|| "na".into());

    println!(
        "openpdf_measure version=1 os={os} arch={arch} pages={n} open_ms={open_ms:.3} first_tile_ms={first_tile_ms:.3} peak_rss_bytes={rss_s}"
    );

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
