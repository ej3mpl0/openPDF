//! Tiles stitched together must match a full-page render.

use openpdf_core::{quantize_zoom, tile_grid, tile_rect, Engine, TileKey, TILE_SIZE};

fn corpus(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .join(name)
}

fn engine_or_skip() -> Option<Engine> {
    match Engine::new() {
        Ok(e) => Some(e),
        Err(err) => {
            if std::env::var("CI").is_ok() {
                panic!("PDFium required in CI: {err}");
            }
            eprintln!("skipping PDFium test: {err}");
            None
        }
    }
}

fn check_zoom(engine: &Engine, file: &str, zoom: f32) {
    let doc = engine.open(corpus(file)).unwrap();
    let zoom = quantize_zoom(zoom) as f32 / 1000.0;
    let full = engine.render_page(doc.id(), 0, zoom).unwrap();
    let (cols, rows) = tile_grid(full.width, full.height, TILE_SIZE);
    let mut stitched = vec![0u8; full.rgba.len()];
    for ty in 0..rows {
        for tx in 0..cols {
            let tile = engine
                .render_tile_blocking(TileKey::new(doc.id(), 0, zoom, tx, ty))
                .unwrap();
            let (tw, th, x0, y0) = tile_rect(full.width, full.height, tx, ty, TILE_SIZE);
            assert_eq!(
                (tile.width, tile.height),
                (tw, th),
                "tile ({tx},{ty}) size at {zoom}"
            );
            for row in 0..th {
                let src = (row * tw * 4) as usize;
                let dst = (((y0 + row) * full.width + x0) * 4) as usize;
                stitched[dst..dst + (tw * 4) as usize]
                    .copy_from_slice(&tile.rgba[src..src + (tw * 4) as usize]);
            }
        }
    }
    let tolerance = 24u8;
    let bad = full
        .rgba
        .iter()
        .zip(&stitched)
        .filter(|(a, b)| a.abs_diff(**b) > tolerance)
        .count();
    let limit = full.rgba.len() / 1000;
    assert!(
        bad <= limit,
        "{file} zoom {zoom}: {bad} bytes differ by more than {tolerance} (limit {limit}), {}x{} px",
        full.width,
        full.height
    );
}

#[test]
fn tiles_match_full_page_render() {
    let Some(engine) = engine_or_skip() else {
        return;
    };
    for file in ["hello.pdf", "two_page.pdf", "outline.pdf"] {
        for zoom in [1.0, 1.47, 2.0] {
            check_zoom(&engine, file, zoom);
        }
    }
}
