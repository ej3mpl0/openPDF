use std::sync::{Arc, Mutex};

use openpdf_core::{
    init_i18n, tr, CertificateInfo, Error, Result, SignRequest, SignatureInfo, Signer,
    SignerUnavailableReason,
};
use openpdf_gtk::measure::{self, DEFAULT_MEASURE_PAGES};
use openpdf_gtk::GtkPinProvider;
use openpdf_sign::{Pkcs11Config, Pkcs11Signer};

/// Compose the real PKCS#11 signer with the GTK PIN dialog.
///
/// `openpdf-gtk` (the UI library) never depends on `openpdf-sign`. This binary
/// is the only place that links both. If no PKCS#11 module is installed,
/// signing is unavailable and the reader still starts.
fn try_load_signer() -> Result<Arc<dyn Signer>> {
    match Pkcs11Signer::new(Pkcs11Config::default(), GtkPinProvider) {
        Ok(signer) => Ok(Arc::new(signer)),
        Err(err) => {
            eprintln!("openpdf-gtk: {err}");
            match err {
                Error::SignerUnavailable { .. } => Err(err),
                _ => Err(Error::signer_unavailable(
                    SignerUnavailableReason::LoadFailed,
                )),
            }
        }
    }
}

/// PKCS#11 module load and cert enumeration happen when signing is opened,
/// not at process start (`--measure` never constructs this).
///
/// A failed load is **not** cached: the next sign attempt retries. Only a
/// successful `Pkcs11Signer` is held.
///
/// The loader (`load`) must never re-enter [`LazyPkcs11`] (`list_certificates`,
/// `sign`, or `verify`). That would deadlock on `inner` or recurse. Load
/// happens with the mutex released so the UI thread is not blocked on the
/// lock; the picker still runs this off the UI thread.
struct LazyPkcs11 {
    load: Box<dyn Fn() -> Result<Arc<dyn Signer>> + Send + Sync>,
    inner: Mutex<Option<Arc<dyn Signer>>>,
}

impl LazyPkcs11 {
    fn new<F>(load: F) -> Self
    where
        F: Fn() -> Result<Arc<dyn Signer>> + Send + Sync + 'static,
    {
        Self {
            load: Box::new(load),
            inner: Mutex::new(None),
        }
    }

    fn get(&self) -> Result<Arc<dyn Signer>> {
        {
            let slot = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(signer) = slot.clone() {
                return Ok(signer);
            }
        }
        // Lock is dropped. `load` must never re-enter LazyPkcs11.
        let loaded = (self.load)();
        match loaded {
            Ok(signer) => {
                let mut slot = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(existing) = slot.clone() {
                    return Ok(existing);
                }
                *slot = Some(signer.clone());
                Ok(signer)
            }
            Err(err) => Err(err),
        }
    }
}

impl Signer for LazyPkcs11 {
    fn list_certificates(&self) -> Result<Vec<CertificateInfo>> {
        self.get()?.list_certificates()
    }

    fn sign(&self, pdf: &[u8], request: &SignRequest) -> Result<Vec<u8>> {
        self.get()?.sign(pdf, request)
    }

    fn verify(&self, pdf: &[u8]) -> Result<Vec<SignatureInfo>> {
        // PAdES verify is CMS + ByteRange. It must not open a token or load
        // the PKCS#11 module — `get()` stays for list_certificates / sign.
        openpdf_sign::verify_pdf(pdf)
    }
}

/// Technical `--measure` mark list. Not a gettext msgid (ui_* names stay English).
const MEASURE_MARKS_HELP: &str = "\
All --measure marks are milliseconds from one process-start Instant \
(set first thing in main):
  startup_ms / ui_all_visible_ms  all visible page tiles + visible thumbs painted (draw path)
  ui_thumbs_done_ms               thumbnail strip meets the gate (MIN_THUMB_TILES)
  ui_first_tile_ms                first real page tile painted (DrawingArea draw func)
  ui_first_surface_ready_ms       first cairo ImageSurface created
  ui_first_tile_requested_ms      first request_tiles
  ui_first_tile_rendered_ms       render worker finished the first tile
  ui_first_frame_ms               first frame-clock paint
  ui_activate_ms                  GTK activate callback
  ui_window_present_ms            window.present()
  ui_window_build_ms              OpenPdfWindow::new returns
  ui_adw_init_ms                  adw::init + icon GResource
  ui_css_ms                       application CSS load
  ui_doc_load_start_ms            document open begins
  ui_doc_loaded_ms                Engine::open / Session::open returned
The first-tile → startup gap is remaining visible tiles plus thumbs.";

fn main() {
    measure::mark_process_start();
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--measure") => {
            // Skip bindtextdomain / catalog I/O; measure always uses English msgids.
            let pages = args
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(DEFAULT_MEASURE_PAGES);
            std::process::exit(openpdf_gtk::run_measure(pages));
        }
        Some("-h") | Some("--help") => {
            init_i18n();
            eprintln!(
                "{}\n\n{}\n  openpdf-gtk [FILE]\n  openpdf-gtk --measure [PAGES]\n\n{}\n\n{MEASURE_MARKS_HELP}",
                tr("Fast, local PDF reader for Linux"),
                tr("Usage:"),
                tr("--measure opens the real libadwaita window (needs a display or xvfb-run), loads a multi-page PDF, waits until every visible page tile and every visible thumbnail has been painted, then prints startup time and peak RSS and exits non-zero if over the documented limits."),
            );
        }
        Some(path) => {
            init_i18n();
            openpdf_gtk::run(
                Some(path.into()),
                Box::new(LazyPkcs11::new(try_load_signer)),
            );
        }
        None => {
            init_i18n();
            openpdf_gtk::run(None, Box::new(LazyPkcs11::new(try_load_signer)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct StubSigner;

    impl Signer for StubSigner {
        fn list_certificates(&self) -> Result<Vec<CertificateInfo>> {
            Ok(Vec::new())
        }
        fn sign(&self, _pdf: &[u8], _request: &SignRequest) -> Result<Vec<u8>> {
            Ok(Vec::new())
        }
        fn verify(&self, _pdf: &[u8]) -> Result<Vec<SignatureInfo>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn failed_pkcs11_load_returns_translated_error() {
        let lazy = LazyPkcs11::new(|| {
            Err(Error::signer_unavailable(
                SignerUnavailableReason::ModuleNotFound,
            ))
        });
        let err = lazy.list_certificates().unwrap_err();
        assert!(matches!(
            err,
            Error::SignerUnavailable {
                reason: SignerUnavailableReason::ModuleNotFound
            }
        ));
        assert_eq!(err.to_string(), "PKCS#11 module not available");
        assert!(
            lazy.sign(b"%PDF", &SignRequest::default()).is_err(),
            "sign must not swallow a failed load"
        );
    }

    #[test]
    fn failed_pkcs11_load_is_retried() {
        let calls = Arc::new(AtomicU32::new(0));
        let lazy = LazyPkcs11::new({
            let calls = calls.clone();
            move || {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Err(Error::signer_unavailable(
                        SignerUnavailableReason::ModuleNotFound,
                    ))
                } else {
                    Ok(Arc::new(StubSigner) as Arc<dyn Signer>)
                }
            }
        });
        let err = lazy.list_certificates().unwrap_err();
        assert!(
            err.to_string().contains("PKCS#11 module not available"),
            "{err}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(lazy.list_certificates().unwrap().is_empty());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "failed load must not stick"
        );
        assert!(lazy.list_certificates().unwrap().is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 2, "success is cached");
    }

    #[test]
    fn verify_signed_pdf_does_not_load_pkcs11() {
        let calls = Arc::new(AtomicU32::new(0));
        let lazy = LazyPkcs11::new({
            let calls = calls.clone();
            move || {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(Error::signer_unavailable(
                    SignerUnavailableReason::ModuleNotFound,
                ))
            }
        });
        let pdf = stub_signed_pdf();
        let infos = lazy
            .verify(&pdf)
            .expect("verify must not need a PKCS#11 module");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "verify must not call the PKCS#11 loader"
        );
        assert!(
            !infos.is_empty(),
            "signed PDF must still produce SignatureInfo"
        );
        assert_eq!(infos[0].valid, Some(false));
    }

    /// Incremental /Type /Sig dictionary so verify runs without SoftHSM.
    fn stub_signed_pdf() -> Vec<u8> {
        use lopdf::{Dictionary, Document, IncrementalDocument, Object};
        let hello = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus/hello.pdf");
        let pdf = std::fs::read(&hello).expect("corpus/hello.pdf");
        let prev = Document::load_mem(&pdf).unwrap();
        let mut inc = IncrementalDocument::create_from(pdf, prev);
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"Sig".to_vec()));
        dict.set("Filter", Object::Name(b"Adobe.PPKLite".to_vec()));
        dict.set("SubFilter", Object::Name(b"adbe.pkcs7.detached".to_vec()));
        dict.set(
            "ByteRange",
            Object::Array(vec![0.into(), 0.into(), 0.into(), 0.into()]),
        );
        dict.set("Contents", Object::string_literal(""));
        inc.new_document.add_object(Object::Dictionary(dict));
        let mut out = Vec::new();
        inc.save_to(&mut out).unwrap();
        out
    }
}
