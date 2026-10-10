//! Opening, rendering and unprotecting password-protected PDFs.

use openpdf_core::{
    crop_pages, extract, merge, optimize, parse_page_ranges, protect, tile_grid, unprotect,
    CropBox, Document, Engine, Error, OptimizeOptions, PdfPermissions, ProtectOptions, TileKey,
    TILE_SIZE,
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

/// `CI` counts as set only when non-empty and not `false` / `0` (any case).
fn ci_enabled(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        None | Some("") => false,
        Some(v) => !v.eq_ignore_ascii_case("false") && v != "0",
    }
}

#[test]
fn ci_env_parsing() {
    for off in [
        None,
        Some(""),
        Some("  "),
        Some("0"),
        Some("false"),
        Some("FALSE"),
        Some("False"),
    ] {
        assert!(!ci_enabled(off), "{off:?}");
    }
    for on in [
        Some("1"),
        Some("true"),
        Some("TRUE"),
        Some("yes"),
        Some("github"),
    ] {
        assert!(ci_enabled(on), "{on:?}");
    }
}

/// Run qpdf; `None` (skip) only when it is not installed and CI is unset.
/// In CI a missing qpdf is a test failure.
fn run_qpdf(args: &[&str], bytes: &[u8]) -> Option<std::process::Output> {
    let path = std::env::temp_dir().join(format!(
        "openpdf-qpdf-{}-{:?}.pdf",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&path, bytes).unwrap();
    // OPENPDF_QPDF overrides the binary (e.g. to check the CI failure path).
    let qpdf = std::env::var("OPENPDF_QPDF").unwrap_or_else(|_| "qpdf".into());
    let out = std::process::Command::new(qpdf)
        .args(args)
        .arg(&path)
        .output();
    std::fs::remove_file(&path).ok();
    match out {
        Ok(o) => Some(o),
        Err(e) if ci_enabled(std::env::var("CI").ok().as_deref()) => {
            panic!("qpdf required in CI: {e}")
        }
        Err(e) => {
            eprintln!("skipping qpdf checks: {e}");
            None
        }
    }
}

/// qpdf's own verdict, when qpdf is installed: `--requires-password`
/// exits 0 = needs a password, 2 = not encrypted, 3 = encrypted, no password.
fn qpdf_requires_password(bytes: &[u8]) -> Option<i32> {
    run_qpdf(&["--requires-password"], bytes).and_then(|o| o.status.code())
}

/// `qpdf --show-encryption` (algorithm, R, P and permission lines), if qpdf is installed.
fn qpdf_encryption_with(bytes: &[u8], password: &str) -> Option<String> {
    let arg = format!("--password={password}");
    run_qpdf(&["--show-encryption", &arg], bytes)
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

fn qpdf_encryption(bytes: &[u8]) -> Option<String> {
    run_qpdf(&["--show-encryption"], bytes).map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// `want`: qpdf --show-encryption of the source (algorithm, R, P, methods).
fn assert_locked(want: &Option<String>, name: &str, bytes: Vec<u8>) {
    if let Some(want) = want {
        assert_eq!(
            qpdf_encryption_with(&bytes, "user-secret").as_ref(),
            Some(want),
            "{name}: qpdf --show-encryption differs from the source"
        );
    }
    let parsed = lopdf::Document::load_mem(&bytes).unwrap();
    assert!(
        parsed.get_encrypted().is_ok() || parsed.was_encrypted(),
        "{name}: output is not encrypted"
    );
    if let Some(code) = qpdf_requires_password(&bytes) {
        assert_eq!(code, 0, "{name}: qpdf says no password is required");
    }
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
    let want = qpdf_encryption_with(&locked, "user-secret");
    if let Some(w) = &want {
        assert!(w.contains("R = 6"), "{w}");
    }
    let doc = Document::from_bytes_with_password(locked, None, pw("user-secret")).unwrap();

    let optimized = optimize(&doc, &OptimizeOptions::default()).unwrap();
    assert_locked(&want, "optimize", optimized.to_bytes().unwrap());

    let all = parse_page_ranges("1-2", doc.page_count()).unwrap();
    let crop = CropBox {
        llx: 10.0,
        lly: 10.0,
        urx: 200.0,
        ury: 200.0,
    };
    assert_locked(
        &want,
        "crop",
        crop_pages(&doc, &all, crop).unwrap().to_bytes().unwrap(),
    );

    let one = parse_page_ranges("2", doc.page_count()).unwrap();
    assert_locked(
        &want,
        "extract one range",
        extract(&doc, &one).unwrap().to_bytes().unwrap(),
    );
    // Two ranges go through merge, which builds a brand-new document.
    let two = parse_page_ranges("1,2", doc.page_count()).unwrap();
    let extracted = extract(&doc, &two).unwrap();
    assert_eq!(extracted.page_count(), 2);
    assert_locked(&want, "extract two ranges", extracted.to_bytes().unwrap());
    assert_locked(
        &want,
        "merge",
        merge(&[doc.clone(), plain]).unwrap().to_bytes().unwrap(),
    );
}

#[test]
fn owner_only_pdf_merge_and_extract_stay_encrypted_without_password() {
    let plain = Document::open(corpus("two_page.pdf")).unwrap();
    let mut opts = ProtectOptions::new("", "owner-secret").unwrap();
    opts.permissions = PdfPermissions::parse_list("print").unwrap();
    let src_bytes = protect(&plain, &opts).unwrap().to_bytes().unwrap();
    if let Some(code) = qpdf_requires_password(&src_bytes) {
        assert_eq!(code, 3, "source: encrypted, no user password");
    }
    let src_qpdf = qpdf_encryption(&src_bytes);
    if let Some(info) = &src_qpdf {
        assert!(info.contains("modify other: not allowed"), "{info}");
    }
    let src = Document::from_bytes(src_bytes, None).unwrap();
    let two = parse_page_ranges("1,2", src.page_count()).unwrap();
    for (name, out) in [
        ("merge", merge(&[plain.clone(), src.clone()]).unwrap()),
        ("extract two ranges", extract(&src, &two).unwrap()),
    ] {
        let bytes = out.to_bytes().unwrap();
        if let Some(code) = qpdf_requires_password(&bytes) {
            assert_eq!(
                code, 3,
                "{name}: qpdf must see encryption without user password"
            );
        }
        let doc = Document::from_bytes(bytes, None).unwrap();
        assert!(doc.is_encrypted(), "{name}: lost encryption");
    }
}
