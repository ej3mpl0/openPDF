//! Simple bench: open time and tile render on a large synthetic PDF.
//! Run with `cargo bench -p openpdf-core`.

use std::time::Instant;

use openpdf_core::{blank_document, Engine, TileKey};

fn pages() -> u32 {
    std::env::var("OPENPDF_BENCH_PAGES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(80)
}

fn main() {
    let n = pages();
    let t0 = Instant::now();
    let doc = blank_document(n).expect("blank_document");
    let build_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let bytes = doc.to_bytes().expect("to_bytes");
    let t1 = Instant::now();
    let opened = openpdf_core::Document::from_bytes(bytes, None).expect("open");
    let open_ms = t1.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(opened.page_count(), n);

    match Engine::new() {
        Ok(engine) => {
            engine.bind(&opened).expect("bind");
            let key = TileKey::new(opened.id(), 0, 1.0, 0, 0);
            let t2 = Instant::now();
            let tile = engine.render_tile_blocking(key).expect("tile");
            let tile_ms = t2.elapsed().as_secs_f64() * 1000.0;
            println!(
                "pages={n} build_ms={build_ms:.2} open_ms={open_ms:.2} tile_ms={tile_ms:.2} tile={}x{}",
                tile.width, tile.height
            );
        }
        Err(e) => {
            if std::env::var("CI").is_ok() {
                panic!("PDFium required in CI: {e}");
            }
            println!("pages={n} build_ms={build_ms:.2} open_ms={open_ms:.2} tile_ms=skip ({e})");
        }
    }
}
