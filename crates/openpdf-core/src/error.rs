use std::fmt;
use std::io;

use crate::i18n::{tr, tr_args};
use crate::signer::{describe_unusable, CertUnusable};

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
    /// PKCS#11 login needs a PIN and none was supplied (empty or missing).
    PinRequired,
    /// The user dismissed the PIN dialog. The token was not touched.
    PinCancelled,
    /// `CKF_USER_PIN_LOCKED`: C_Login would fail; do not consume further tries.
    PinLocked,
    /// The PDF already has a signature; `op` would invalidate it.
    Signed {
        op: SignedOp,
    },
    /// The chosen certificate cannot sign (expired, not yet valid, auth-only, ...).
    /// Match the reason instead of parsing [`Display`].
    CertUnusable(CertUnusable),
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
            Error::Sign(m) => tr_args("sign: {detail}", &[("detail", &tr(m))]),
            Error::PinRequired => tr("PIN is required"),
            Error::PinCancelled => tr("PIN entry was cancelled"),
            Error::PinLocked => tr("Token PIN is locked"),
            Error::Signed { op } => tr_args(
                "document is signed; {op} would invalidate the signature (incremental save only)",
                &[("op", &tr(op.msgid()))],
            ),
            // describe_unusable already returns catalog text; do not tr() it again.
            Error::CertUnusable(reason) => describe_unusable(reason),
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
        assert_eq!(Error::PinRequired.to_string(), "PIN is required");
        assert_eq!(Error::PinCancelled.to_string(), "PIN entry was cancelled");
        assert_eq!(Error::PinLocked.to_string(), "Token PIN is locked");
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
        assert!(matches!(Error::PinRequired, Error::PinRequired));
        assert!(matches!(Error::PinCancelled, Error::PinCancelled));
        assert!(matches!(Error::PinLocked, Error::PinLocked));
    }

    #[test]
    fn cert_unusable_display_translates_once() {
        let expired = CertUnusable::Expired {
            not_after: "2020-01-01T00:00:00Z".into(),
        };
        let reasons = [
            expired.clone(),
            CertUnusable::NotYetValid {
                not_before: "2099-01-01T00:00:00Z".into(),
            },
            CertUnusable::AuthenticationOnly,
        ];
        crate::test_support::with_locale("en", || {
            assert_eq!(
                Error::CertUnusable(expired.clone()).to_string(),
                "Certificate has expired (valid until 2020-01-01T00:00:00Z)"
            );
            assert_eq!(
                Error::CertUnusable(CertUnusable::AuthenticationOnly).to_string(),
                "Authentication only"
            );
        });
        crate::test_support::with_locale("es", || {
            for r in &reasons {
                let shown = Error::CertUnusable(r.clone()).to_string();
                // Exactly describe_unusable: no "sign:" wrapper, no second tr().
                assert_eq!(shown, describe_unusable(r));
                assert!(!shown.starts_with(&tr_args("sign: {detail}", &[("detail", "")])));
            }
            let es = Error::CertUnusable(expired.clone()).to_string();
            assert_ne!(
                es,
                "Certificate has expired (valid until 2020-01-01T00:00:00Z)"
            );
            assert!(es.contains("2020-01-01T00:00:00Z"), "{es}");
        });
    }

    #[test]
    fn cert_unusable_is_matchable() {
        let err = Error::CertUnusable(CertUnusable::AuthenticationOnly);
        assert!(matches!(
            err,
            Error::CertUnusable(CertUnusable::AuthenticationOnly)
        ));
    }
}
