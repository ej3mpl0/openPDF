use std::fmt;
use std::io;

use crate::i18n::{tr, tr_args};

/// Crate-wide error type. Variants are stable so the GTK UI can localize them
/// itself; [`Display`] uses the shared gettext catalog.
#[derive(Debug)]
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
    /// The signer backend cannot produce this PAdES level yet. Never downgraded.
    SignLevelUnsupported {
        level: crate::signer::PadesLevel,
    },
    /// PAdES B-T: the RFC 3161 timestamp could not be obtained or did not
    /// validate. The signature is never downgraded to B-B.
    Timestamp(TimestampErrorKind),
    /// The caller cancelled the operation through a [`crate::CancelToken`].
    /// Not a failure: the UI should treat it as a no-op. Nothing was written.
    Cancelled,
    /// The PDF already has a signature; `op` would invalidate it.
    Signed {
        op: SignedOp,
    },
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
    /// `true` for [`Error::Cancelled`]: the user aborted, do not show an error.
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Error::Cancelled)
    }

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
            Error::Cancelled => tr("operation cancelled"),
            Error::Sign(m) => tr_args("sign: {detail}", &[("detail", &tr(m))]),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{set_locale, tr};

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
        let err = Error::signed(SignedOp::Rotate);
        assert!(matches!(
            err,
            Error::Signed {
                op: SignedOp::Rotate
            }
        ));
        let err = Error::invalid(InvalidKind::CertIdRequired);
        assert!(matches!(err, Error::Invalid(InvalidKind::CertIdRequired)));
    }
}
