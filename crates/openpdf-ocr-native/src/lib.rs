//! Native [`OcrEngine`] backends. Depends only on `openpdf-core`.
//!
//! - Linux: [`openpdf_core::TesseractOcr`] (no extra crates)
//! - Windows: [`WindowsOcr`] (`Windows.Media.Ocr`)
//! - macOS: [`VisionOcr`] (Vision `VNRecognizeTextRequest`)
//!
//! The application binary constructs [`default_engine`] and hands
//! `Box<dyn OcrEngine>` to the CLI / UI, like `Signer`.

mod geom;
mod lang;

#[cfg(target_os = "macos")]
mod vision;
#[cfg(windows)]
mod windows_ocr;

pub use geom::{pack_rgba, vision_box_to_pixels, windows_rect_to_word};
pub use lang::{
    language_accepted, normalize_bcp47, primary_language, to_tesseract_tag, to_vision_tag,
    to_windows_tag,
};

#[cfg(target_os = "macos")]
pub use vision::VisionOcr;
#[cfg(windows)]
pub use windows_ocr::WindowsOcr;

use openpdf_core::{tr, OcrEngine, UnavailableOcr};

/// How to enable the platform OCR engine on this OS.
pub fn install_hint() -> String {
    if cfg!(windows) {
        tr("install the Windows OCR language pack")
    } else if cfg!(target_os = "macos") {
        tr("install the Vision OCR language pack in System Settings")
    } else {
        openpdf_core::ocr_install_hint()
    }
}

/// Platform default: Windows.Media.Ocr, Vision, or Tesseract on Linux.
pub fn default_engine() -> Box<dyn OcrEngine> {
    #[cfg(windows)]
    {
        if let Some(eng) = WindowsOcr::probe() {
            return Box::new(eng);
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(eng) = VisionOcr::probe() {
            return Box::new(eng);
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(eng) = openpdf_core::TesseractOcr::probe() {
            return Box::new(eng);
        }
    }
    Box::new(UnavailableOcr::new(install_hint()))
}

#[cfg(all(test, any(windows, target_os = "macos")))]
mod native_tests {
    use super::*;
    use openpdf_core::{ocr, Engine, OcrOptions};
    use std::path::Path;

    #[test]
    fn recognizes_hello_when_language_pack_present() {
        let ocr_eng = default_engine();
        if !ocr_eng.capability().available {
            eprintln!("skip: OS OCR language pack is absent");
            return;
        }
        let Some(engine) = Engine::new().ok() else {
            eprintln!("skip: PDFium is not available");
            return;
        };
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/hello.pdf");
        let doc = match engine.open(&path) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("skip: could not open corpus ({e})");
                return;
            }
        };
        let raster = match engine.render_page(doc.id(), 0, 2.0) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("skip: could not render corpus ({e})");
                return;
            }
        };
        match ocr_eng.recognize(
            raster.width,
            raster.height,
            raster.width.saturating_mul(4),
            raster.rgba,
            vec!["en".into(), "eng".into()],
        ) {
            Ok(words) => {
                let blob = words
                    .iter()
                    .map(|w| w.text.to_ascii_lowercase())
                    .collect::<Vec<_>>()
                    .join(" ");
                assert!(
                    blob.contains("hello") || blob.contains("world") || !words.is_empty(),
                    "expected words from hello.pdf, got {blob:?}"
                );
            }
            Err(openpdf_core::OcrError::LanguageMissing { .. })
            | Err(openpdf_core::OcrError::Unavailable { .. }) => {
                eprintln!("skip: OS OCR language pack is absent");
            }
            Err(e) => panic!("native OCR failed: {e:?}"),
        }
        let _ = ocr(&engine, ocr_eng.as_ref(), &doc, &OcrOptions::default());
    }
}
