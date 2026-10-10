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
    use std::sync::Arc;

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
        let secret = "ffi-secret-NEVER-ECHO-9f3a"; // test-only placeholder, not a real secret (allowlisted in .gitleaks.toml, #27)
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

    struct SleepPin {
        started: std::sync::Arc<std::sync::Barrier>,
        delay: std::time::Duration,
    }

    impl PinProvider for SleepPin {
        fn request_pin(&self, _token_label: String) -> Result<Arc<SecretPin>, FfiError> {
            self.started.wait();
            std::thread::sleep(self.delay);
            Err(FfiError::pin_cancelled())
        }
    }

    struct PinThenIdentity {
        pin: SleepPin,
    }

    impl openpdf_core::Signer for PinThenIdentity {
        fn list_certificates(&self) -> openpdf_core::Result<Vec<openpdf_core::CertificateInfo>> {
            Ok(Vec::new())
        }

        fn sign(
            &self,
            pdf: &[u8],
            _request: &openpdf_core::SignRequest,
        ) -> openpdf_core::Result<Vec<u8>> {
            match self.pin.request_pin("tok".into()) {
                Ok(_) => Ok(pdf.to_vec()),
                Err(FfiError::PinCancelled { .. }) => Err(openpdf_core::Error::PinCancelled),
                Err(e) => Err(openpdf_core::Error::Sign(e.to_string())),
            }
        }

        fn verify(&self, _pdf: &[u8]) -> openpdf_core::Result<Vec<openpdf_core::SignatureInfo>> {
            Ok(Vec::new())
        }
    }

    struct DelayedIdentitySigner {
        started: std::sync::Arc<std::sync::Barrier>,
        delay: std::time::Duration,
    }

    impl openpdf_core::Signer for DelayedIdentitySigner {
        fn list_certificates(&self) -> openpdf_core::Result<Vec<openpdf_core::CertificateInfo>> {
            Ok(Vec::new())
        }

        fn sign(
            &self,
            pdf: &[u8],
            _request: &openpdf_core::SignRequest,
        ) -> openpdf_core::Result<Vec<u8>> {
            self.started.wait();
            std::thread::sleep(self.delay);
            Ok(pdf.to_vec())
        }

        fn verify(&self, _pdf: &[u8]) -> openpdf_core::Result<Vec<openpdf_core::SignatureInfo>> {
            Ok(Vec::new())
        }
    }

    fn identity_request() -> openpdf_core::SignRequest {
        openpdf_core::SignRequest {
            cert_id: "test-cert".into(),
            reason: None,
            location: None,
            contact: None,
            appearance: None,
        }
    }

    #[test]
    fn sign_does_not_block_render_while_pin_provider_sleeps() {
        require_pdfium_in_ci();
        if !engine_available() {
            return;
        }
        let doc = PdfDocument::open(corpus_path("hello.pdf")).unwrap();
        let started = std::sync::Arc::new(std::sync::Barrier::new(2));
        let pin = SleepPin {
            started: started.clone(),
            delay: std::time::Duration::from_secs(2),
        };
        let sign_doc = doc.clone();
        let handle = std::thread::spawn(move || {
            sign_doc.sign_with_signer(&PinThenIdentity { pin }, &identity_request())
        });
        started.wait();
        let t0 = std::time::Instant::now();
        let tile = doc.render_tile(0, 1.0, 0, 0).expect("render during PIN");
        let elapsed = t0.elapsed();
        assert!(tile.width > 0 && tile.height > 0);
        assert!(
            elapsed < std::time::Duration::from_millis(1500),
            "render_tile blocked for {elapsed:?} while PinProvider slept"
        );
        let err = handle.join().expect("sign thread");
        assert!(
            matches!(err, Err(FfiError::PinCancelled { .. })),
            "got {err:?}"
        );
    }

    #[test]
    fn sign_fails_if_page_rotated_during_signing() {
        let doc = PdfDocument::open(corpus_path("hello.pdf")).unwrap();
        let before = doc.signer_bytes_hash().unwrap();
        let started = std::sync::Arc::new(std::sync::Barrier::new(2));
        let signer = DelayedIdentitySigner {
            started: started.clone(),
            delay: std::time::Duration::from_millis(400),
        };
        let sign_doc = doc.clone();
        let handle =
            std::thread::spawn(move || sign_doc.sign_with_signer(&signer, &identity_request()));
        started.wait();
        doc.rotate(0, 90).expect("rotate during sign");
        let err = handle.join().expect("sign thread");
        assert!(
            matches!(err, Err(FfiError::DocumentChanged { .. })),
            "got {err:?}"
        );
        assert_eq!(doc.pages().unwrap()[0].rotation, 90);
        assert!(!doc.is_signed().unwrap());
        assert_ne!(
            doc.signer_bytes_hash().unwrap(),
            before,
            "rotation must change signer bytes"
        );
    }

    #[test]
    fn annotation_add_undo_restores_signer_hash_then_sign_succeeds() {
        let doc = PdfDocument::open(corpus_path("hello.pdf")).unwrap();
        let before = doc.signer_bytes_hash().unwrap();
        let previous = doc
            .add_header_annotation("review note")
            .expect("annotation add");
        doc.restore_document(previous).expect("annotation undo");
        let after_undo = doc.signer_bytes_hash().unwrap();
        assert_eq!(
            before, after_undo,
            "assumption failed: annotation add+undo must restore the signer-byte SHA-256 (not a signature failure)"
        );

        let started = std::sync::Arc::new(std::sync::Barrier::new(2));
        let signer = DelayedIdentitySigner {
            started: started.clone(),
            delay: std::time::Duration::from_millis(400),
        };
        let sign_doc = doc.clone();
        let handle =
            std::thread::spawn(move || sign_doc.sign_with_signer(&signer, &identity_request()));
        started.wait();
        let previous = doc
            .add_header_annotation("review note")
            .expect("annotation add during sign");
        doc.restore_document(previous)
            .expect("annotation undo during sign");
        handle
            .join()
            .expect("sign thread")
            .expect("sign must succeed when add+undo restores the snapshot hash");
        assert_eq!(
            doc.signer_bytes_hash().unwrap(),
            before,
            "successful identity sign must keep the restored snapshot bytes"
        );
    }
}
