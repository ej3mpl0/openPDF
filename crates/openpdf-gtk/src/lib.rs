// SPDX-License-Identifier: MPL-2.0
//! GTK frontend for openPDF.
//!
//! Rendering and document I/O go through `openpdf-core`. This crate never
//! depends on `openpdf-sign`. The `openpdf-gtk` binary (package `openpdf-app`)
//! constructs `Pkcs11Signer` / the token-less `Verifier` and injects
//! `Box<dyn Signer>` plus `Box<dyn Verifier>`. A
//! [`backend::MockDocument`] remains for unit tests when PDFium is missing.

/// Desktop / D-Bus / AppStream id. Matches `data/io.github.ej3mpl0.openpdf.*`
/// and the Flatpak id in PR #6. Also the GSettings schema id and GResource
/// prefix (`/io/github/ej3mpl0/openpdf`) if those are added later.
pub const APP_ID: &str = "io.github.ej3mpl0.openpdf";

/// Separate id so `--measure` does not take over the session D-Bus name.
pub const MEASURE_APP_ID: &str = "io.github.ej3mpl0.openpdf.Measure";

pub mod backend;
pub mod core_needed;
pub mod i18n;
pub mod measure;
pub mod pin;
pub mod session;
pub mod settings;
pub mod sign;
pub mod tiles;
pub mod ui;

pub use backend::{BackendError, DocumentBackend, MockDocument, PageSize, TileBitmap, TileRequest};
pub use i18n::Lang;
pub use pin::GtkPinProvider;
pub use session::{Opened, Session};
pub use settings::{Settings, Theme, ZoomMode};
pub use tiles::{
    layout_continuous, layout_single, visible_tiles, PageLayout, PageRect, TileCache, TileKey,
    DEFAULT_TILE_SIZE,
};
pub use ui::{run, run_measure};

#[cfg(test)]
mod tests {
    #[test]
    fn app_id_matches_desktop_metainfo_and_icon() {
        assert_eq!(super::APP_ID, "io.github.ej3mpl0.openpdf");
        assert_eq!(super::MEASURE_APP_ID, "io.github.ej3mpl0.openpdf.Measure");
        let desktop = include_str!("../../../data/io.github.ej3mpl0.openpdf.desktop");
        assert!(desktop.contains(&format!("Icon={}", super::APP_ID)));
        assert!(desktop.contains(&format!("StartupWMClass={}", super::APP_ID)));
        let meta = include_str!("../../../data/io.github.ej3mpl0.openpdf.metainfo.xml");
        assert!(meta.contains(&format!("<id>{}</id>", super::APP_ID)));
        assert!(meta.contains(&format!("<icon type=\"stock\">{}</icon>", super::APP_ID)));
        let icon =
            include_str!("../../../data/icons/hicolor/scalable/apps/io.github.ej3mpl0.openpdf.svg");
        assert!(icon.contains("<svg"));
    }
}
