// SPDX-License-Identifier: MPL-2.0
//! UniFFI bindings for openPDF — the native macOS (Swift) and Windows (C#) entry point.
//!
//! This crate wraps [`openpdf_core`] and [`openpdf_sign`]. Foreign code never
//! implements [`openpdf_core::Signer`]; listing certificates and signing stay
//! in Rust. The PIN is a UniFFI callback ([`pin::PinProvider`]) that returns
//! [`pin::SecretPin`]. Cancel or empty is [`FfiError::PinCancelled`] — Rust
//! never sends an empty PIN to the token. Passwords are
//! [`zeroize::Zeroizing`] and are never logged.
//!
//! All values that cross the FFI boundary are owned (`String`, `Vec<u8>`,
//! records). Do not add borrowed `&str` / `&[u8]` to the exported surface.
//!
//! # Binding generation
//!
//! Build the cdylib, then generate Swift with this crate's `uniffi-bindgen`
//! and C# with [`uniffi-bindgen-cs`](https://github.com/NordSecurity/uniffi-bindgen-cs)
//! **v0.10.0+v0.29.4** (matches UniFFI 0.29.4):
//!
//! ```bash
//! cargo build -p openpdf-ffi --release
//! cargo run -p openpdf-ffi --features bindgen --bin uniffi-bindgen -- \
//!     generate --library target/release/libopenpdf_ffi.so \
//!     --language swift --out-dir bindings/swift
//! cargo install uniffi-bindgen-cs \
//!     --git https://github.com/NordSecurity/uniffi-bindgen-cs \
//!     --tag v0.10.0+v0.29.4
//! uniffi-bindgen-cs --library --out-dir bindings/csharp \
//!     target/release/libopenpdf_ffi.so
//! ```
//!
//! Linux is enough for generation; compiling the Swift/C# output is a
//! platform-UI concern.
//!
//! # TODO(ocr): foreign `OcrEngine` callback (PR #10)
//!
//! Core on `main` has **no** `OcrEngine` trait. When
//! [PR #10](https://github.com/ej3mpl0/openPDF/pull/10) (`core-portable`)
//! lands, expose a UniFFI callback that matches that trait **exactly** — do
//! not invent a diverging fake trait now:
//!
//! ```text
//! trait OcrEngine: Send + Sync {
//!     fn capability(&self) -> OcrCapability;
//!     fn languages(&self) -> Vec<String>;
//!     fn recognize(
//!         &self,
//!         width: u32,
//!         height: u32,
//!         stride: u32,
//!         rgba: Vec<u8>,
//!         languages: Vec<String>,
//!     ) -> Result<Vec<OcrWord>, OcrError>;
//! }
//!
//! OcrWord { text, x, y, w, h, confidence }
//! OcrError {
//!     Unavailable { hint },
//!     LanguageMissing { language, installed, hint },
//!     Failed { message },
//! }
//! ```
//!
//! `rgba` is owned so a foreign recognizer can keep the buffer. Do not compile
//! against #10 until it is merged.

uniffi::setup_scaffolding!("openpdf");

mod document;
mod error;
mod i18n;
mod pin;
mod types;

pub use document::{images_to_pdf_paths, list_certificates, tile_size, OpenPdf, PdfDocument};
pub use error::FfiError;
pub use i18n::{describe_pin_status, pin_prompt, translate};
pub use pin::{PinProvider, SecretPin};
pub use types::{
    Certificate, OutlineItem, PageInfo, PageRange, PageRaster, Permissions, RasterTile, SearchHit,
};

use std::sync::Once;

pub(crate) fn ensure_i18n() {
    static ONCE: Once = Once::new();
    ONCE.call_once(openpdf_core::init_i18n);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn corpus(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus")
            .join(name)
    }

    fn corpus_path(name: &str) -> String {
        corpus(name).to_string_lossy().into_owned()
    }

    fn engine_available() -> bool {
        openpdf_core::Engine::new().is_ok()
    }

    fn require_pdfium_in_ci() {
        if std::env::var_os("CI").is_some() && !engine_available() {
            panic!("PDFium required in CI");
        }
    }

    #[test]
    fn open_hello_page_count() {
        let doc = PdfDocument::open(corpus_path("hello.pdf")).unwrap();
        assert_eq!(doc.page_count().unwrap(), 1);
        let pages = doc.pages().unwrap();
        assert_eq!(pages.len(), 1);
        assert!((pages[0].width_pt - 612.0).abs() < 0.1);
        assert!(!doc.is_signed().unwrap());
    }

    #[test]
    fn open_via_library_handle() {
        let lib = OpenPdf::new();
        let two = lib.open(corpus_path("two_page.pdf")).unwrap();
        assert_eq!(two.page_count().unwrap(), 2);
    }

    #[test]
    fn outline_fixture() {
        let doc = PdfDocument::open(corpus_path("outline.pdf")).unwrap();
        let outline = doc.outline().unwrap();
        assert_eq!(outline.len(), 2);
        assert_eq!(outline[0].title, "Chapter 1");
        assert_eq!(outline[0].page, Some(0));
        assert_eq!(outline[1].children.len(), 0);
    }

    #[test]
    fn rotate_and_undo() {
        let doc = PdfDocument::open(corpus_path("hello.pdf")).unwrap();
        doc.rotate(0, 90).unwrap();
        assert_eq!(doc.pages().unwrap()[0].rotation, 90);
        assert!(doc.can_undo().unwrap());
        assert!(doc.undo().unwrap());
        assert_eq!(doc.pages().unwrap()[0].rotation, 0);
        assert!(doc.redo().unwrap());
        assert_eq!(doc.pages().unwrap()[0].rotation, 90);
    }

    #[test]
    fn extract_and_optimize() {
        let two = PdfDocument::open(corpus_path("two_page.pdf")).unwrap();
        let extracted = two.extract(vec![PageRange { start: 0, end: 0 }]).unwrap();
        assert_eq!(extracted.page_count().unwrap(), 1);

        let hello = PdfDocument::open(corpus_path("hello.pdf")).unwrap();
        let optimized = hello.optimize(Some(75), true, true).unwrap();
        assert_eq!(optimized.page_count().unwrap(), 1);
    }

    #[test]
    fn render_tile_if_pdfium() {
        require_pdfium_in_ci();
        if !engine_available() {
            return;
        }
        let doc = PdfDocument::open(corpus_path("hello.pdf")).unwrap();
        let tile = doc.render_tile(0, 1.0, 0, 0).unwrap();
        assert!(tile.width > 0 && tile.height > 0);
        assert_eq!(tile.rgba.len(), (tile.width * tile.height * 4) as usize);
        assert_eq!(tile_size(), 256);
    }

    #[test]
    fn text_and_search_if_pdfium() {
        require_pdfium_in_ci();
        if !engine_available() {
            return;
        }
        let doc = PdfDocument::open(corpus_path("two_page.pdf")).unwrap();
        let text0 = doc.page_text(0).unwrap();
        assert!(
            text0.contains("ONE") || text0.to_lowercase().contains("one"),
            "extracted: {text0:?}"
        );
        let hits = doc.search("TWO".into()).unwrap();
        assert!(hits.iter().any(|h| h.page == 1), "hits: {hits:?}");
    }

    #[test]
    fn password_errors_do_not_echo_secrets() {
        let secret = "ffi-secret-NEVER-ECHO-9f3a";
        let doc = PdfDocument::open(corpus_path("hello.pdf")).unwrap();

        let err = match doc.unprotect(secret.to_string()) {
            Err(e) => e,
            Ok(_) => panic!("unprotect on a clear PDF"),
        };
        let text = err.to_string();
        assert!(
            !text.contains(secret),
            "unprotect error echoed the password"
        );

        let err = match doc.protect(String::new(), String::new(), None) {
            Err(e) => e,
            Ok(_) => panic!("empty passwords"),
        };
        let text = err.to_string();
        assert!(!text.contains(secret));

        let locked = doc
            .protect(secret.to_string(), secret.to_string(), None)
            .unwrap();
        let err = match locked.unprotect("wrong-password-ALSO-SECRET".into()) {
            Err(e) => e,
            Ok(_) => panic!("wrong password"),
        };
        let text = err.to_string();
        assert!(!text.contains(secret));
        assert!(!text.contains("wrong-password-ALSO-SECRET"));
        assert!(matches!(err, FfiError::Invalid { .. }));
    }

    #[test]
    fn insert_from_same_document_does_not_deadlock() {
        let doc = PdfDocument::open(corpus_path("two_page.pdf")).unwrap();
        let inserted = doc
            .insert_pages(2, doc.clone(), vec![PageRange { start: 0, end: 0 }])
            .expect("self-insert");
        assert_eq!(inserted.page_count().unwrap(), 3);
    }
}
