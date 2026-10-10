//! `openpdf-core` — document model, PDFium tiles, lopdf structure ops, and the
//! `Signer` / `OcrEngine` traits.
//!
//! One repo, native UI per platform (Linux GTK now; Windows and macOS later)
//! sharing this crate, the CLI, and signing.
//!
//! GTK (`openpdf-gtk`) and signing (`openpdf-sign`) depend on this crate only.
//! They must never depend on each other.
//!
//! The application binary (not the GTK UI crate) constructs `Box<dyn Signer>`
//! from `openpdf-sign` and `Box<dyn OcrEngine>` (Tesseract on Linux; platform
//! OCR later) and hands those trait objects to the UI. The UI implements
//! [`PinProvider`] and formats certificates / verify results with the helpers
//! on this crate.

mod command;
mod compare;
mod convert;
mod dll;
mod document;
mod error;
pub mod i18n;
mod ocr;
mod ops;
mod optimize;
mod outline;
mod overlay;
mod pages;
mod paths;
mod pin;
mod protect;
mod redact;
mod render;
mod save;
mod signer;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use command::{Command, History};
pub use compare::{compare_pdfs, CompareReport, PageDiff};
pub use convert::{blank_document, images_to_pdf, pdf_to_rasters, raster_to_png};
pub use document::{Document, DocumentId, PageInfo};
pub use error::{Error, InvalidKind, Result, SignedOp};
pub use i18n::{
    available_locales, current_locale, detect_locale, init as init_i18n, set_locale, tr, tr_args,
};

/// Call this first from the app / CLI / measure binary.
/// Hardens Windows DLL search (no CWD) and loads the gettext catalog.
pub fn init() {
    dll::harden();
    i18n::init();
}
#[cfg(target_os = "linux")]
pub use ocr::TesseractOcr;
pub use ocr::{
    default_ocr_engine, ocr, ocr_capability, ocr_install_hint, OcrBackend, OcrCapability,
    OcrEngine, OcrError, OcrImage, OcrOptions, OcrWord, UnavailableOcr,
};
pub use ops::{merge, merge_paths, parse_page_ranges, split, split_all, PageRange};
pub use optimize::{optimize, OptimizeOptions};
pub use outline::OutlineItem;
pub use overlay::{apply_overlay, Overlay, OverlayKind};
pub use pages::{crop_pages, extract, insert_pages, replace_pages, CropBox};
pub use paths::{ensure_app_dirs, recovery_dir, settings_dir};
pub use pin::{PinProvider, SecretPin};
pub use protect::{protect, sanitize_metadata, unprotect, PdfPermissions, ProtectOptions};
pub use redact::{redact, RedactBox};
pub use render::{
    font_warmup_ms, page_pixel_size, pdfium_library_name, quantize_zoom, tile_grid, tile_rect,
    Engine, Priority, Raster, SearchHit, Tile, TileCache, TileEvent, TileKey, TileRequest,
    DEFAULT_CACHE_BYTES, TILE_SIZE,
};
pub use signer::{
    describe_certificate, format_verify_result, format_verify_results, pin_prompt_text,
    usage_label, CertificateInfo, CertificateUsage, CertificateValidity, SignRequest,
    SignatureInfo, Signer, VisibleAppearance,
};
