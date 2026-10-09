//! PKCS#11 PAdES-B-B signatures for openPDF.
//!
//! This crate implements [`openpdf_core::Signer`]. The GTK UI must not depend
//! on it: the application binary constructs `Box<dyn Signer>` here and hands
//! that trait object to UI code that only sees `openpdf-core`.
//!
//! The PIN is requested via [`openpdf_core::PinProvider`] (implemented by the
//! UI) and zeroized after login. Core never stores it on [`SignRequest`].

mod cades;
mod pades;
mod pin;
mod token;

pub use pin::StaticPinProvider;
pub use token::{verify_pdf, Pkcs11Config, Pkcs11Signer};

pub use openpdf_core::{
    init_i18n, set_locale, tr, tr_args, CertificateInfo, CertificateUsage, CertificateValidity,
    PinProvider, SecretPin, SignRequest, SignatureInfo, Signer, VisibleAppearance,
};

#[cfg(test)]
mod tests;
