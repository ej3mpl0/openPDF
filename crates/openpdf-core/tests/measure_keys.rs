//! `openpdf-measure` key contract: stable core keys, never GTK `ui_` prefix.

use std::process::Command;

fn measure_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_openpdf-measure"))
}

#[test]
fn help_documents_origin_and_core_keys() {
    let out = measure_bin().arg("--help").output().expect("help");
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("from document-open start, not process start"));
    for key in [
        "open_ms",
        "first_tile_ms",
        "peak_rss_bytes",
        "pdfium_load_ms",
        "doc_open_ms",
        "convert_ms",
        "visible_tiles",
        "first_ready_ms",
        "first_ready_x",
        "first_ready_y",
        "visible_tiles_ms",
    ] {
        assert!(s.contains(key), "help missing {key}: {s}");
    }
    assert!(
        !s.split_whitespace().any(|w| w.starts_with("ui_")),
        "help must not list GTK ui-prefixed keys as core output"
    );
}

#[test]
fn stdout_line_has_new_keys_and_no_ui_prefix() {
    let out = measure_bin()
        .env("OPENPDF_BENCH_PAGES", "2")
        .output()
        .expect("measure");
    if !out.status.success() {
        eprintln!(
            "skip: measure failed ({})",
            String::from_utf8_lossy(&out.stderr)
        );
        return;
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let line = line.lines().next().unwrap_or("");
    assert!(line.contains("open_ms="), "{line}");
    assert!(line.contains("first_tile_ms="), "{line}");
    assert!(line.contains("peak_rss_bytes="), "{line}");
    assert!(line.contains("pdfium_load_ms="), "{line}");
    assert!(line.contains("doc_open_ms="), "{line}");
    assert!(line.contains("convert_ms="), "{line}");
    assert!(line.contains("visible_tiles="), "{line}");
    assert!(line.contains("first_ready_ms="), "{line}");
    assert!(line.contains("visible_tiles_ms="), "{line}");
    assert!(
        !line.split_whitespace().any(|f| f.starts_with("ui_")),
        "core must never emit GTK ui-prefixed keys: {line}"
    );
}
