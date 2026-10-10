//! `openpdf-core` — document model, PDFium tiles, lopdf structure ops, and the
//! `Signer` / `Verifier` traits.
//!
//! GTK (`openpdf-gtk`) and signing (`openpdf-sign`) depend on this crate only.
//! They must never depend on each other.
//!
//! The application binary (not the GTK UI crate) constructs `Box<dyn Signer>`
//! and `Box<dyn Verifier>` from `openpdf-sign` and hands them to the UI. The UI
//! implements [`PinProvider`] and formats certificates / verify results with
//! the helpers on this crate.

mod command;
mod convert;
mod document;
mod error;
pub mod i18n;
mod ops;
mod optimize;
mod outline;
mod overlay;
mod pages;
mod paths;
mod pin;
mod protect;
mod render;
mod save;
mod signer;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use command::{Command, History};
pub use convert::{
    blank_document, images_to_pdf, pdf_to_rasters, raster_to_png, red_rect_document,
};
pub use document::{Document, DocumentId, PageInfo};
pub use error::{
    describe_signer_unavailable, Error, InvalidKind, Result, SignedOp, SignerUnavailableReason,
};
pub use i18n::{
    available_locales, current_locale, detect_locale, init as init_i18n, set_locale, tr, tr_args,
};
pub use ops::{
    merge, merge_encryption_source, merge_paths, parse_page_ranges, split, split_all,
    EncryptionSourceKind, PageRange,
};
pub use optimize::{optimize, OptimizeOptions};
pub use outline::OutlineItem;
pub use overlay::{apply_overlay, Overlay, OverlayKind};
pub use pages::{crop_pages, extract, insert_pages, replace_pages, CropBox};
pub use paths::{ensure_app_dirs, recovery_dir, settings_dir};
pub use pin::{PinProvider, SecretPin};
pub use protect::{protect, sanitize_metadata, unprotect, PdfPermissions, ProtectOptions};
pub use render::{
    page_pixel_size, parse_tile_cache_bytes, quantize_zoom, tile_cache_bytes, tile_grid, tile_rect,
    viewport_cache_bytes, Engine, Priority, Raster, SearchHit, Tile, TileCache, TileEvent, TileKey,
    TileRequest, DEFAULT_CACHE_BYTES, MAX_CACHE_BYTES, MIN_CACHE_BYTES, TILE_SIZE,
};
pub use signer::{
    certificate_common_name, describe_certificate, describe_certificate_detail,
    format_verify_result, format_verify_results, pin_prompt_text, usage_label, CertificateInfo,
    CertificateUsage, CertificateValidity, SignRequest, SignatureInfo, Signer, Verifier,
    VisibleAppearance,
};
pub use zeroize::Zeroizing;
