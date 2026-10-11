use std::time::Duration;

use openpdf_core::{
    blank_document, merge, split_all, Document, Engine, Priority, TileKey, TileRequest, TILE_SIZE,
};

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

#[test]
fn info_and_outline_without_pdfium() {
    let hello = Document::open(corpus("hello.pdf")).unwrap();
    assert_eq!(hello.page_count(), 1);
    let two = Document::open(corpus("two_page.pdf")).unwrap();
    assert_eq!(two.page_count(), 2);
    let outline = Document::open(corpus("outline.pdf")).unwrap().outline().unwrap();
    assert_eq!(outline.len(), 2);
}

#[test]
fn merge_split_roundtrip() {
    let a = Document::open(corpus("hello.pdf")).unwrap();
    let b = Document::open(corpus("two_page.pdf")).unwrap();
    let merged = merge(&[a, b]).unwrap();
    assert_eq!(merged.page_count(), 3);
    let parts = split_all(&merged).unwrap();
    assert_eq!(parts.len(), 3);
}

#[test]
fn render_text_search_and_tile() {
    let Some(engine) = engine_or_skip() else {
        return;
    };
    let doc = engine.open(corpus("two_page.pdf")).unwrap();
    let text0 = engine.page_text(doc.id(), 0).unwrap();
    assert!(
        text0.contains("ONE") || text0.to_lowercase().contains("one"),
        "extracted: {text0:?}"
    );
    let hits = engine.search(doc.id(), "TWO").unwrap();
    assert!(hits.iter().any(|h| h.page == 1), "hits: {hits:?}");

    let raster = engine.render_page(doc.id(), 0, 1.0).unwrap();
    assert!(raster.width > 0 && raster.height > 0);
    assert_eq!(raster.rgba.len(), (raster.width * raster.height * 4) as usize);
    assert!(
        raster.rgba.iter().any(|b| *b < 250),
        "page render looks blank"
    );

    let key = TileKey::new(doc.id(), 0, 1.0, 0, 0);
    let tile = engine.render_tile_blocking(key).unwrap();
    assert!(tile.width <= TILE_SIZE && tile.height <= TILE_SIZE);
    assert_eq!(tile.rgba.len(), (tile.width * tile.height * 4) as usize);
}

#[test]
fn cancel_stale_tile_requests() {
    let Some(engine) = engine_or_skip() else {
        return;
    };
    let doc = engine.open(corpus("hello.pdf")).unwrap();
    let keep = TileKey::new(doc.id(), 0, 1.0, 0, 0);
    let stale = TileKey::new(doc.id(), 0, 2.0, 0, 0);
    engine.begin_frame().unwrap();
    engine
        .request_tiles([
            TileRequest {
                key: stale,
                priority: Priority::Thumbnail,
            },
            TileRequest {
                key: keep,
                priority: Priority::Visible,
            },
        ])
        .unwrap();
    engine.cancel_except(doc.id(), [keep]).unwrap();

    let mut saw_keep = false;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if let Some(ev) = engine.recv_timeout(Duration::from_millis(200)) {
            match ev {
                openpdf_core::TileEvent::Ready(t) if t.key == keep => saw_keep = true,
                openpdf_core::TileEvent::Ready(t) if t.key == stale => {
                    panic!("stale tile was delivered")
                }
                _ => {}
            }
        }
        if saw_keep {
            break;
        }
    }
    assert!(saw_keep, "visible tile never arrived");
}

#[test]
fn open_and_tile_large_pdf() {
    let n = 40u32;
    let t0 = std::time::Instant::now();
    let doc = blank_document(n).unwrap();
    let build = t0.elapsed();
    let t1 = std::time::Instant::now();
    let bytes = doc.to_bytes().unwrap();
    let opened = Document::from_bytes(bytes, None).unwrap();
    let open = t1.elapsed();
    assert_eq!(opened.page_count(), n);
    eprintln!("large pdf build={build:?} open={open:?}");
    let Some(engine) = engine_or_skip() else {
        return;
    };
    engine.bind(&opened).unwrap();
    let t2 = std::time::Instant::now();
    let tile = engine
        .render_tile_blocking(TileKey::new(opened.id(), n - 1, 1.0, 0, 0))
        .unwrap();
    let tile_t = t2.elapsed();
    assert!(tile.width > 0 && tile.height > 0);
    eprintln!("large pdf tile={tile_t:?} {}x{}", tile.width, tile.height);
}
