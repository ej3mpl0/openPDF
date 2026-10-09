//! PKCS#11 PAdES-B-B signatures for openPDF.
//!
//! GTK and this crate talk only through [`openpdf_core::Signer`]. The PIN is
//! requested via [`PinProvider`] and zeroized after login; it never crosses
//! `openpdf-core`.

mod cades;
mod i18n;
mod pades;
mod pin;
mod token;

pub use i18n::{
    describe_certificate, format_verify_result, format_verify_results, gettext, gettext_f,
    pin_prompt_text, usage_label,
};
pub use openpdf_core::{init_i18n, set_locale, tr, tr_args};
pub use pin::{PinProvider, SecretPin, StaticPinProvider};
pub use token::{verify_pdf, Pkcs11Config, Pkcs11Signer};

pub use openpdf_core::{
    CertificateInfo, CertificateUsage, CertificateValidity, SignRequest, SignatureInfo, Signer,
    VisibleAppearance,
};

#[cfg(test)]
mod tests;
