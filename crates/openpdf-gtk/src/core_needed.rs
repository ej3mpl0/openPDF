// SPDX-License-Identifier: MPL-2.0
//! What `openpdf-gtk` uses from the workspace `openpdf-core` crate.
//!
//! This crate depends on `openpdf-core` through the workspace (never a path
//! into another branch) and never depends on `openpdf-sign`.
//!
//! ## Used from core
//!
//! `Engine` (open, bind, refresh, single-thread tile queue, 64 MiB LRU, text,
//! search), `Document` (page size, outline, rotate/delete/move, undo,
//! incremental save, [`Document::is_signed`] / [`Document::is_read_only`] /
//! [`Document::ensure_editable`]), `Signer::list_certificates`,
//! `SignRequest.cert_id`, `Document::apply_signer`, `PinProvider` /
//! `SecretPin`, `pin_prompt_text` / `describe_certificate` /
//! `format_verify_results`, XDG `settings_dir` / `recovery_dir`.
//!
//! Signed documents are append-only: the UI disables rotate / delete / move /
//! undo from `is_signed()` / `is_read_only()`. Core refuses those ops with
//! `Error::Signed`.
//!
//! The PIN is not a field on `SignRequest` or `CertificateInfo`. GTK implements
//! [`crate::GtkPinProvider`] and receives `Box<dyn Signer>` from the
//! `openpdf-gtk` binary (`openpdf-app`).
//!
//! Optional later: bookmarks live-update after page moves; comment / organize
//! / protect / edit tools.

/// Marker so the module is referenced from docs and `cargo test`.
pub const CORE_API_VERSION_EXPECTED: &str = "0.1-tiles+signer-certs+is_signed+pin";

#[cfg(test)]
mod tests {
    use super::*;
    use openpdf_core::{Document, PinProvider};

    #[test]
    fn expected_api_names_the_merged_core() {
        let _ = CORE_API_VERSION_EXPECTED;
        let _ = std::any::type_name::<Document>();
        let _ = std::any::type_name::<dyn PinProvider>();
    }
}
