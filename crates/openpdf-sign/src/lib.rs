//! PAdES-B-B signatures for openPDF.
//!
//! This crate implements [`openpdf_core::Signer`]. The GTK UI must not depend
//! on it: the application binary constructs `Box<dyn Signer>` here and hands
//! that trait object to UI code that only sees `openpdf-core`.
//!
//! Backends only enumerate certificates and produce a raw signature over the
//! CMS signed-attributes digest. PKCS#11 is available on Linux, Windows, and
//! macOS. Windows uses the CurrentUser\\MY store (CNG/NCrypt); macOS uses the
//! Keychain. [`default_signer`] combines the native store with PKCS#11.
//!
//! The PIN is requested via [`openpdf_core::PinProvider`] for PKCS#11 only and
//! zeroized after login. Native backends let the OS own smart-card / Keychain
//! prompts. Core never stores a PIN on [`SignRequest`].

mod backend;
mod cades;
mod cert;
mod pades;
mod pin;
mod token;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub use backend::{default_signer, CompositeSigner};
pub use pin::StaticPinProvider;
pub use token::{verify_pdf, Pkcs11Config, Pkcs11Signer};

#[cfg(target_os = "macos")]
pub use macos::KeychainSigner;
#[cfg(windows)]
pub use windows::WindowsStoreSigner;

pub use openpdf_core::{
    init_i18n, set_locale, tr, tr_args, CertUnusable, CertificateInfo, CertificateUsage,
    CertificateValidity, PinProvider, SecretPin, SignRequest, SignatureInfo, Signer,
    TokenPinStatus, VisibleAppearance,
};

#[cfg(test)]
mod tests;
