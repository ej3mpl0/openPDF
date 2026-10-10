//! PKCS#11 PAdES-B-B and B-T signatures for openPDF.
//!
//! B-T adds an RFC 3161 signature timestamp. The TSA URL must be set on the
//! request or in [`Pkcs11Config`]; there is no default server.
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
mod tsp;

pub use pin::StaticPinProvider;
pub use token::{verify_pdf, verify_pdf_with_tsa_trust, Pkcs11Config, Pkcs11Signer};
pub use tsp::DEFAULT_TSA_TIMEOUT;

pub use openpdf_core::{
    init_i18n, set_locale, tr, tr_args, CertificateInfo, CertificateUsage, CertificateValidity,
    PadesLevel, PinProvider, SecretPin, SignRequest, SignatureInfo, Signer, TimestampErrorKind,
    TimestampInfo, VisibleAppearance,
};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tsa_tests;
