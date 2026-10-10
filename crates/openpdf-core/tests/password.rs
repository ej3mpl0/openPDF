//! Opening, rendering and unprotecting password-protected PDFs.

use openpdf_core::{
    crop_pages, extract, merge, optimize, parse_page_ranges, protect, tile_grid, unprotect,
    CropBox, Document, Engine, Error, OptimizeOptions, ProtectOptions, TileKey, TILE_SIZE,
};
use zeroize::Zeroizing;

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

fn locked_bytes() -> Vec<u8> {
    let doc = Document::open(corpus("hello.pdf")).unwrap();
    let opts = ProtectOptions::new("user-secret", "owner-secret").unwrap();
    protect(&doc, &opts).unwrap().to_bytes().unwrap()
}

fn pw(s: &str) -> Zeroizing<String> {
    Zeroizing::new(s.to_string())
}

#[test]
fn no_password_is_password_required() {
    let err = Document::from_bytes(locked_bytes(), None).unwrap_err();
    assert!(matches!(err, Error::PasswordRequired), "{err:?}");
}

#[test]
fn wrong_password_is_wrong_password() {
    let err = Document::from_bytes_with_password(locked_bytes(), None, pw("nope")).unwrap_err();
    assert!(matches!(err, Error::WrongPassword), "{err:?}");
}

#[test]
fn user_and_owner_passwords_open_and_unprotect() {
    for password in ["user-secret", "owner-secret"] {
        let doc = Document::from_bytes_with_password(locked_bytes(), None, pw(password)).unwrap();
        assert!(doc.is_encrypted());
        assert_eq!(doc.page_count(), 1);
        assert!(matches!(unprotect(&doc, "nope"), Err(Error::WrongPassword)));
        let open = unprotect(&doc, password).unwrap();
        assert!(!open.is_encrypted());
        let reopened = Document::from_bytes(open.to_bytes().unwrap(), None).unwrap();
        assert_eq!(reopened.page_count(), 1);
    }
}

#[test]
fn edited_protected_doc_stays_encrypted_on_save() {
    let mut doc =
        Document::from_bytes_with_password(locked_bytes(), None, pw("user-secret")).unwrap();
    doc.rotate_page(0, 90).unwrap();
    for bytes in [doc.to_bytes().unwrap(), doc.to_incremental_bytes().unwrap()] {
        assert!(matches!(
            Document::from_bytes(bytes.clone(), None),
            Err(Error::PasswordRequired)
        ));
        let again = Document::from_bytes_with_password(bytes, None, pw("user-secret")).unwrap();
        assert_eq!(again.page(0).unwrap().rotation, 90);
    }
}

#[test]
fn engine_renders_protected_pdf_opened_with_password() {
    let Some(engine) = engine_or_skip() else {
        return;
    };
    let doc = Document::from_bytes_with_password(locked_bytes(), None, pw("user-secret")).unwrap();
    engine.bind(&doc).unwrap();
    let full = engine.render_page(doc.id(), 0, 1.0).unwrap();
    let (cols, rows) = tile_grid(full.width, full.height, TILE_SIZE);
    let mut dark = 0usize;
    for ty in 0..rows {
        for tx in 0..cols {
            let tile = engine
                .render_tile_blocking(TileKey::new(doc.id(), 0, 1.0, tx, ty))
                .unwrap();
            dark += tile.rgba.chunks(4).filter(|p| p[0] < 128).count();
        }
    }
    assert!(dark > 0, "protected page rendered blank");
    // Dirty documents rebind from re-encrypted bytes with the same password.
    let mut edited = doc.clone();
    edited.rotate_page(0, 90).unwrap();
    engine.refresh(&edited).unwrap();
}

fn assert_locked(name: &str, bytes: Vec<u8>) {
    assert!(
        bytes.windows(8).any(|w| w == b"/Encrypt"),
        "{name}: output has no /Encrypt"
    );
    assert!(
        matches!(
            Document::from_bytes(bytes.clone(), None),
            Err(Error::PasswordRequired)
        ),
        "{name}: opened without a password"
    );
    let doc = Document::from_bytes_with_password(bytes, None, pw("user-secret"))
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    assert!(doc.is_encrypted(), "{name}");
}

#[test]
fn derived_documents_stay_encrypted() {
    let bytes = std::fs::read(corpus("two_page.pdf")).unwrap();
    let plain = Document::from_bytes(bytes, None).unwrap();
    let opts = ProtectOptions::new("user-secret", "owner-secret").unwrap();
    let locked = protect(&plain, &opts).unwrap().to_bytes().unwrap();
    let doc = Document::from_bytes_with_password(locked, None, pw("user-secret")).unwrap();

    let optimized = optimize(&doc, &OptimizeOptions::default()).unwrap();
    assert_locked("optimize", optimized.to_bytes().unwrap());

    let all = parse_page_ranges("1-2", doc.page_count()).unwrap();
    let crop = CropBox {
        llx: 10.0,
        lly: 10.0,
        urx: 200.0,
        ury: 200.0,
    };
    assert_locked(
        "crop",
        crop_pages(&doc, &all, crop).unwrap().to_bytes().unwrap(),
    );

    let one = parse_page_ranges("2", doc.page_count()).unwrap();
    assert_locked(
        "extract one range",
        extract(&doc, &one).unwrap().to_bytes().unwrap(),
    );
    // Two ranges go through merge, which builds a brand-new document.
    let two = parse_page_ranges("1,2", doc.page_count()).unwrap();
    let extracted = extract(&doc, &two).unwrap();
    assert_eq!(extracted.page_count(), 2);
    assert_locked("extract two ranges", extracted.to_bytes().unwrap());
    assert_locked(
        "merge",
        merge(&[doc.clone(), plain]).unwrap().to_bytes().unwrap(),
    );
}
