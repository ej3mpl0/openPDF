use std::fmt;
use std::io;

use crate::i18n::{tr, tr_args};

/// Crate-wide error type. Variants are stable so the GTK UI can localize them
/// itself; [`Display`] uses the shared gettext catalog.
///
/// `#[non_exhaustive]` so additive variants (`PinCancelled`, …) do not break
/// downstream `match` expressions that include a wildcard arm.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    Io(io::Error),
    Structure(String),
    Pdfium(String),
    PdfiumLoad {
        last: String,
        system: String,
    },
    PageOutOfRange {
        page: u32,
        count: u32,
    },
    EmptyDocument,
    Invalid(InvalidKind),
    Sign(String),
    /// PKCS#11 (or another token backend) could not be loaded.
    SignerUnavailable {
        reason: SignerUnavailableReason,
    },
    /// The user dismissed the PIN dialog or submitted an empty field.
    /// Unit variant — no payload. The token was not touched. UIs treat
    /// this as a silent cancel. The CLI prints a neutral status, not an error.
    PinCancelled,
    /// [`crate::CancelToken`] fired (Cancel before PIN, before TSA, or while
    /// waiting). Unit variant — not a failure. UIs stay silent and leave the
    /// document untouched. The CLI prints a neutral status, not an error.
    SignCancelled,
    /// The signer backend cannot produce this PAdES level yet. Never downgraded.
    SignLevelUnsupported {
        level: crate::signer::PadesLevel,
    },
    /// PAdES B-T: the RFC 3161 timestamp could not be obtained or did not
    /// validate. The signature is never downgraded to B-B.
    Timestamp(TimestampErrorKind),
    /// The PDF already has a signature; `op` would invalidate it.
    Signed {
        op: SignedOp,
    },
}

/// Why a PAdES B-T timestamp was refused. Match this instead of parsing [`Display`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TimestampErrorKind {
    /// B-T was requested but no TSA URL is configured (there is no default).
    UrlMissing,
    /// The TSA URL is not an `http://` or `https://` URL.
    BadUrl,
    /// The TSA could not be reached (DNS, TLS, timeout, ...).
    Network { detail: String },
    /// The TSA answered with a non-success HTTP status.
    Http { status: u16 },
    /// The TSA answered, but did not grant the timestamp (PKIStatus).
    Rejected { status: u8 },
    /// The response or token is not valid RFC 3161 DER.
    Malformed,
    /// The token's nonce differs from the one we sent.
    NonceMismatch,
    /// The token's messageImprint is not the hash of our signature value.
    ImprintMismatch,
    /// The token's CMS signature does not verify.
    BadSignature,
    /// The ESS signing-certificate attribute does not bind the TSA certificate.
    SigningCertMismatch,
    /// The TSA certificate does not chain to a trusted root, is not valid at
    /// the timestamp time, or lacks the timeStamping key purpose.
    UntrustedCert,
}

impl TimestampErrorKind {
    pub fn to_locale_string(&self) -> String {
        match self {
            TimestampErrorKind::UrlMissing => {
                tr("PAdES-B-T needs a timestamp server (TSA) URL; none is configured")
            }
            TimestampErrorKind::BadUrl => {
                tr("the timestamp server URL must start with http:// or https://")
            }
            TimestampErrorKind::Network { detail } => tr_args(
                "could not reach the timestamp server: {detail}",
                &[("detail", detail)],
            ),
            TimestampErrorKind::Http { status } => tr_args(
                "the timestamp server answered HTTP {status}",
                &[("status", &status.to_string())],
            ),
            TimestampErrorKind::Rejected { status } => tr_args(
                "the timestamp server refused the request (status {status})",
                &[("status", &status.to_string())],
            ),
            TimestampErrorKind::Malformed => tr("the timestamp server sent a malformed response"),
            TimestampErrorKind::NonceMismatch => {
                tr("the timestamp does not answer our request (nonce mismatch)")
            }
            TimestampErrorKind::ImprintMismatch => {
                tr("the timestamp does not cover this signature (hash mismatch)")
            }
            TimestampErrorKind::BadSignature => tr("the timestamp signature is not valid"),
            TimestampErrorKind::SigningCertMismatch => {
                tr("the timestamp is not bound to the TSA certificate (ESS signing-certificate)")
            }
            TimestampErrorKind::UntrustedCert => {
                tr("the timestamp server certificate is not trusted")
            }
        }
    }
}

/// Why [`Error::SignerUnavailable`] fired. Match this; do not parse [`Display`].
///
/// `#[non_exhaustive]` so later reasons (pinpad, slot, …) do not break
/// downstream `match` expressions that include a wildcard arm.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignerUnavailableReason {
    /// No PKCS#11 module on the search path / `OPENPDF_PKCS11_MODULE`.
    ModuleNotFound,
    /// A module path was found but `dlopen` / cryptoki load failed.
    LoadFailed {
        /// Path that failed (`OPENPDF_PKCS11_MODULE` or a searched default).
        module: String,
        /// System/loader error text (not localized).
        detail: String,
    },
}

impl SignerUnavailableReason {
    pub fn load_failed(module: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::LoadFailed {
            module: module.into(),
            detail: detail.into(),
        }
    }
}

/// User-facing invalid-input cases. Match this instead of parsing [`Display`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvalidKind {
    CertIdRequired,
    RangeInverted { start: u32, end: u32 },
    PageNumbersOneBased,
    NoPageRanges,
    NotAPageNumber { value: String },
    DocumentNotBound,
    CropInverted,
    PasswordRequired,
    WrongPassword,
    NotEncrypted,
    AlreadyEncrypted,
    NoInputImages,
    EmptyOverlayText,
    UnknownPermission { value: String },
}

/// Destructive operation blocked on a signed document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignedOp {
    Edit,
    Rotate,
    Delete,
    Move,
    Undo,
    Redo,
    Rewrite,
}

impl SignedOp {
    pub fn msgid(self) -> &'static str {
        match self {
            SignedOp::Edit => "edit",
            SignedOp::Rotate => "rotate",
            SignedOp::Delete => "delete",
            SignedOp::Move => "move",
            SignedOp::Undo => "undo",
            SignedOp::Redo => "redo",
            SignedOp::Rewrite => "rewrite",
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_locale_string())
    }
}

impl Error {
    /// Localized message for the active catalog. The UI may ignore this and
    /// match the variant instead.
    pub fn to_locale_string(&self) -> String {
        match self {
            Error::Io(e) => tr_args("I/O error: {detail}", &[("detail", &e.to_string())]),
            Error::Structure(m) => tr_args("PDF structure: {detail}", &[("detail", &tr(m))]),
            Error::Pdfium(m) => tr_args("PDFium: {detail}", &[("detail", &tr(m))]),
            Error::PdfiumLoad { last, system } => tr_args(
                "could not load libpdfium (last: {last}, system: {system})",
                &[("last", last), ("system", system)],
            ),
            Error::PageOutOfRange { page, count } => tr_args(
                "page {page} is out of range (0..{count})",
                &[("page", &page.to_string()), ("count", &count.to_string())],
            ),
            Error::EmptyDocument => tr("document has no pages"),
            Error::Invalid(kind) => kind.to_locale_string(),
            Error::SignLevelUnsupported { level } => tr_args(
                "{level} signatures are not supported yet",
                &[("level", level.label())],
            ),
            Error::Timestamp(kind) => tr_args(
                "timestamp: {detail}",
                &[("detail", &kind.to_locale_string())],
            ),
            Error::Sign(m) => tr_args("sign: {detail}", &[("detail", &tr(m))]),
            Error::SignerUnavailable { reason } => describe_signer_unavailable(reason),
            Error::PinCancelled => tr("PIN entry was cancelled"),
            Error::SignCancelled => tr("Signing was cancelled"),
            Error::Signed { op } => tr_args(
                "document is signed; {op} would invalidate the signature (incremental save only)",
                &[("op", &tr(op.msgid()))],
            ),
        }
    }
}

impl InvalidKind {
    pub fn to_locale_string(&self) -> String {
        match self {
            InvalidKind::CertIdRequired => tr("SignRequest.cert_id is required"),
            InvalidKind::RangeInverted { start, end } => tr_args(
                "range {start}-{end} is inverted",
                &[("start", &start.to_string()), ("end", &end.to_string())],
            ),
            InvalidKind::PageNumbersOneBased => tr("page numbers are 1-based"),
            InvalidKind::NoPageRanges => tr("no page ranges"),
            InvalidKind::NotAPageNumber { value } => {
                tr_args("not a page number: {value}", &[("value", value)])
            }
            InvalidKind::DocumentNotBound => tr("document is not bound to the render engine"),
            InvalidKind::CropInverted => tr("crop box is inverted"),
            InvalidKind::PasswordRequired => tr("password is required"),
            InvalidKind::WrongPassword => tr("wrong password"),
            InvalidKind::NotEncrypted => tr("document is not encrypted"),
            InvalidKind::AlreadyEncrypted => tr("document is already encrypted"),
            InvalidKind::NoInputImages => tr("no input images"),
            InvalidKind::EmptyOverlayText => tr("overlay text is empty"),
            InvalidKind::UnknownPermission { value } => {
                tr_args("unknown permission: {value}", &[("value", value)])
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Error::Io(value)
    }
}

impl From<lopdf::Error> for Error {
    fn from(value: lopdf::Error) -> Self {
        Error::Structure(value.to_string())
    }
}

impl Error {
    pub fn structure(msg: impl Into<String>) -> Self {
        Error::Structure(msg.into())
    }

    pub fn pdfium(msg: impl Into<String>) -> Self {
        Error::Pdfium(msg.into())
    }

    pub fn pdfium_load(last: impl Into<String>, system: impl Into<String>) -> Self {
        Error::PdfiumLoad {
            last: last.into(),
            system: system.into(),
        }
    }

    pub fn invalid(kind: InvalidKind) -> Self {
        Error::Invalid(kind)
    }

    pub fn signed(op: SignedOp) -> Self {
        Error::Signed { op }
    }

    pub fn signer_unavailable(reason: SignerUnavailableReason) -> Self {
        Error::SignerUnavailable { reason }
    }

    /// PIN dismiss or [`crate::CancelToken`]. Not a failure — stay silent.
    pub fn is_silent_cancel(&self) -> bool {
        matches!(self, Error::PinCancelled | Error::SignCancelled)
    }
}

/// Translated text for [`Error::SignerUnavailable`]. The UI must not build this.
pub fn describe_signer_unavailable(reason: &SignerUnavailableReason) -> String {
    match reason {
        SignerUnavailableReason::ModuleNotFound => tr("PKCS#11 module not available"),
        SignerUnavailableReason::LoadFailed { module, detail } => tr_args(
            "Could not load the PKCS#11 module {module}: {detail}",
            &[("module", module), ("detail", detail)],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{set_locale, tr, tr_args};

    #[test]
    fn display_follows_locale() {
        let _g = crate::test_support::locale_test_guard();
        set_locale("en");
        assert_eq!(Error::EmptyDocument.to_string(), "document has no pages");
        set_locale("es");
        assert_eq!(
            Error::EmptyDocument.to_string(),
            tr("document has no pages")
        );
        assert_ne!(Error::EmptyDocument.to_string(), "document has no pages");
        set_locale("en");
    }

    #[test]
    fn ui_can_match_structured_errors() {
        let _g = crate::test_support::locale_test_guard();
        let err = Error::signed(SignedOp::Rotate);
        assert!(matches!(
            err,
            Error::Signed {
                op: SignedOp::Rotate
            }
        ));
        let err = Error::invalid(InvalidKind::CertIdRequired);
        assert!(matches!(err, Error::Invalid(InvalidKind::CertIdRequired)));
        let err = Error::signer_unavailable(SignerUnavailableReason::ModuleNotFound);
        assert!(matches!(
            err,
            Error::SignerUnavailable {
                reason: SignerUnavailableReason::ModuleNotFound
            }
        ));
        set_locale("en");
        assert_eq!(err.to_string(), "PKCS#11 module not available");
        assert_eq!(
            describe_signer_unavailable(&SignerUnavailableReason::load_failed(
                "/usr/lib/softhsm/libsofthsm2.so",
                "libfoo.so: cannot open shared object file",
            )),
            "Could not load the PKCS#11 module /usr/lib/softhsm/libsofthsm2.so: libfoo.so: cannot open shared object file"
        );
        set_locale("es");
        assert_ne!(
            describe_signer_unavailable(&SignerUnavailableReason::ModuleNotFound),
            "PKCS#11 module not available"
        );
        set_locale("en");
        assert!(matches!(Error::PinCancelled, Error::PinCancelled));
    }

    #[test]
    fn pin_cancelled_is_structured_not_sign() {
        let _g = crate::test_support::locale_test_guard();
        set_locale("en");
        // Unit variant: no payload. `Error::PinCancelled` (not `PinCancelled(_)`).
        let err = Error::PinCancelled;
        assert!(matches!(err, Error::PinCancelled));
        assert_eq!(err.to_string(), "PIN entry was cancelled");
        assert!(!matches!(err, Error::Sign(_)));
        assert!(err.is_silent_cancel());
        set_locale("es");
        assert_eq!(
            Error::PinCancelled.to_string(),
            tr("PIN entry was cancelled")
        );
        assert_ne!(Error::PinCancelled.to_string(), "PIN entry was cancelled");
        set_locale("en");
        let cancel = Error::SignCancelled;
        assert!(matches!(cancel, Error::SignCancelled));
        assert!(cancel.is_silent_cancel());
        assert!(!matches!(cancel, Error::Sign(_)));
        assert_eq!(cancel.to_string(), "Signing was cancelled");
        set_locale("es");
        assert_eq!(
            Error::SignCancelled.to_string(),
            tr("Signing was cancelled")
        );
        assert_ne!(Error::SignCancelled.to_string(), "Signing was cancelled");
        set_locale("en");
    }

    #[test]
    fn load_failed_carries_module_path_and_system_detail() {
        let _g = crate::test_support::locale_test_guard();
        let reason = SignerUnavailableReason::load_failed(
            "/usr/lib/softhsm/libsofthsm2.so",
            "libfoo.so: cannot open shared object file",
        );
        assert!(matches!(
            reason,
            SignerUnavailableReason::LoadFailed {
                ref module,
                ref detail,
            } if module == "/usr/lib/softhsm/libsofthsm2.so"
                && detail == "libfoo.so: cannot open shared object file"
        ));
        let err = Error::signer_unavailable(reason.clone());
        set_locale("en");
        let en = describe_signer_unavailable(&reason);
        assert_eq!(
            en,
            "Could not load the PKCS#11 module /usr/lib/softhsm/libsofthsm2.so: libfoo.so: cannot open shared object file"
        );
        assert_eq!(err.to_string(), en);
        set_locale("es");
        let es = describe_signer_unavailable(&reason);
        assert!(
            es.contains("/usr/lib/softhsm/libsofthsm2.so"),
            "GTK error must keep the module path: {es}"
        );
        assert!(
            es.contains("libfoo.so: cannot open shared object file"),
            "GTK error must keep the system detail: {es}"
        );
        assert_eq!(
            es,
            tr_args(
                "Could not load the PKCS#11 module {module}: {detail}",
                &[
                    ("module", "/usr/lib/softhsm/libsofthsm2.so"),
                    ("detail", "libfoo.so: cannot open shared object file"),
                ]
            )
        );
        assert_ne!(es, en);
        set_locale("en");
    }
}
