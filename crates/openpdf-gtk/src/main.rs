//! GTK UI for openPDF.
//!
//! This crate depends only on `openpdf-core`. It must never depend on
//! `openpdf-sign`. The application binary wires signing:
//!
//! ```ignore
//! use openpdf_core::{PinProvider, Signer};
//!
//! fn main() {
//!     openpdf_core::init_i18n();
//!     let pin = /* GTK dialog implementing PinProvider */;
//!     let signer: Box<dyn Signer> = openpdf_sign::default_signer(pin)?;
//!     // hand `signer` to UI modules; they list certs, call `pin_prompt_text`,
//!     // `describe_certificate`, and `format_verify_results` from core
//! }
//! ```

fn main() {}
