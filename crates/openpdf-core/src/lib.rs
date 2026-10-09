//! `openpdf-core` — document model, PDFium tiles, lopdf structure ops, and the `Signer` trait.
//!
//! GTK (`openpdf-gtk`) and signing (`openpdf-sign`) depend on this crate only.
//! They must never depend on each other.

mod command;
mod document;
mod error;
pub mod i18n;
mod ops;
mod outline;
mod paths;
mod render;
mod save;
mod signer;

pub use command::{Command, History};
pub use document::{Document, DocumentId, PageInfo};
pub use error::{Error, InvalidKind, Result, SignedOp};
pub use i18n::{
    available_locales, current_locale, detect_locale, init as init_i18n, set_locale, tr, tr_args,
};
pub use ops::{merge, merge_paths, parse_page_ranges, split, split_all, PageRange};
pub use outline::OutlineItem;
pub use paths::{ensure_app_dirs, recovery_dir, settings_dir};
pub use render::{
    page_pixel_size, quantize_zoom, tile_grid, tile_rect, Engine, Priority, Raster, SearchHit,
    Tile, TileCache, TileEvent, TileKey, TileRequest, DEFAULT_CACHE_BYTES, TILE_SIZE,
};
pub use signer::{
    CertificateInfo, CertificateUsage, CertificateValidity, SignRequest, SignatureInfo, Signer,
    VisibleAppearance,
};

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};

    pub fn corpus(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus")
            .join(name)
    }
}
