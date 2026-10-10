//! UI strings. Call sites pass **English** msgids; Spanish lives in `po/es.po`.
//!
//! Locale follows `openpdf-core` (GNU gettext): `LANGUAGE` (colon-separated
//! priority list) first, then `LC_ALL` / `LC_MESSAGES` / `LANG`. `LANGUAGE` is
//! ignored when the effective LC locale is `C` or `POSIX`. Missing keys fall
//! back to English. Preferences may pin `en` / `es` or use `auto`.
//!
//! ```ignore
//! t(lang, "Open")
//! format_core_error(&Error::signed(SignedOp::Rotate))
//! ```

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use openpdf_core::i18n::Catalog;
use openpdf_core::{
    describe_signer_unavailable, detect_locale, init_i18n, set_locale, tr, tr_args, Error,
    InvalidKind, SignedOp,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Es,
}

impl Lang {
    pub fn parse(code: &str) -> Self {
        let tag = code.replace('-', "_").to_ascii_lowercase();
        if tag == "es" || tag.starts_with("es_") {
            Self::Es
        } else {
            Self::En
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Es => "es",
        }
    }
}

static ES: OnceLock<Catalog> = OnceLock::new();
static INTERN: Mutex<Option<HashMap<String, &'static str>>> = Mutex::new(None);

fn es_catalog() -> &'static Catalog {
    ES.get_or_init(|| Catalog::parse(include_str!("../../../po/es.po")))
}

fn intern(s: &str) -> &'static str {
    let mut guard = INTERN.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(v) = map.get(s) {
        return v;
    }
    let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
    map.insert(s.to_string(), leaked);
    leaked
}

/// Look up `msgid` (English source string). English returns the msgid; Spanish
/// reads the shared `po/es.po` catalog and falls back to English if missing.
pub fn t(lang: Lang, msgid: &'static str) -> &'static str {
    match lang {
        Lang::En => msgid,
        Lang::Es => {
            let s = es_catalog().translate(msgid);
            if s == msgid {
                msgid
            } else {
                intern(s)
            }
        }
    }
}

/// Load the shared catalog from env, then apply a settings override.
///
/// `auto` / empty follow `detect_locale()`. Anything else is passed to
/// [`set_locale`].
pub fn apply_locale(language: &str) {
    init_i18n();
    if language.is_empty() || language == "auto" {
        set_locale(&detect_locale());
    } else {
        set_locale(language);
    }
}

/// Map a structured core [`Error`] to a translated UI message.
///
/// Matches `Error` / `InvalidKind` / `SignedOp`. Does not parse [`Display`].
pub fn format_core_error(err: &Error) -> String {
    match err {
        Error::Io(e) => tr_args(
            "Could not read the file: {detail}",
            &[("detail", &e.to_string())],
        ),
        Error::Structure(m) => tr_args("Could not read this PDF: {detail}", &[("detail", &tr(m))]),
        Error::Pdfium(m) => tr_args("Could not render the page: {detail}", &[("detail", &tr(m))]),
        Error::PdfiumLoad { last, system } => tr_args(
            "could not load libpdfium (last: {last}, system: {system})",
            &[("last", last), ("system", system)],
        ),
        Error::PageOutOfRange { page, count } => tr_args(
            "page {page} is out of range (0..{count})",
            &[("page", &page.to_string()), ("count", &count.to_string())],
        ),
        Error::EmptyDocument => tr("document has no pages"),
        Error::Invalid(kind) => format_invalid_kind(kind),
        Error::Sign(m) => tr_args(
            "Could not sign the document: {detail}",
            &[("detail", &tr(m))],
        ),
        Error::Signed { op } => format_signed_op(*op),
        Error::SignerUnavailable { reason } => describe_signer_unavailable(reason),
    }
}

fn format_invalid_kind(kind: &InvalidKind) -> String {
    match kind {
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

fn format_signed_op(op: SignedOp) -> String {
    tr_args(
        "This document is signed; {op} is disabled.",
        &[("op", &tr(op.msgid()))],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_msgid_is_the_source_string() {
        assert_eq!(t(Lang::En, "Open"), "Open");
        assert_eq!(Lang::parse("es"), Lang::Es);
        assert_eq!(Lang::parse("es_ES.UTF-8"), Lang::Es);
        assert_eq!(Lang::parse("en_US"), Lang::En);
        assert_eq!(Lang::parse("auto"), Lang::En);
    }

    #[test]
    fn spanish_comes_from_shared_po() {
        let cat = Catalog::parse(include_str!("../../../po/es.po"));
        assert_ne!(t(Lang::Es, "Open"), "Open");
        assert_eq!(t(Lang::Es, "Open"), cat.translate("Open"));
        assert_eq!(t(Lang::Es, "Preferences"), cat.translate("Preferences"));
        assert_eq!(t(Lang::Es, "not a real msgid"), "not a real msgid");
    }

    #[test]
    fn maps_signed_and_invalid_variants() {
        let _g = openpdf_core::test_support::locale_test_guard();
        set_locale("en");
        let signed = Error::signed(SignedOp::Rotate);
        let msg = format_core_error(&signed);
        assert_eq!(msg, "This document is signed; rotate is disabled.");
        assert!(matches!(
            signed,
            Error::Signed {
                op: SignedOp::Rotate
            }
        ));

        let invalid = Error::invalid(InvalidKind::CertIdRequired);
        assert_eq!(
            format_core_error(&invalid),
            "SignRequest.cert_id is required"
        );

        set_locale("es");
        let es = format_core_error(&signed);
        assert_ne!(es, msg);
        assert!(es.contains(&tr("rotate")), "{es}");
        assert!(!es.contains("rotate"), "{es}");
        set_locale("en");
    }

    #[test]
    fn maps_empty_document_without_display() {
        let _g = openpdf_core::test_support::locale_test_guard();
        set_locale("en");
        let err = Error::EmptyDocument;
        assert_eq!(format_core_error(&err), "document has no pages");
        set_locale("es");
        assert_eq!(format_core_error(&err), tr("document has no pages"));
        assert_ne!(format_core_error(&err), "document has no pages");
        assert!(matches!(err, Error::EmptyDocument));
        set_locale("en");
    }

    #[test]
    fn signer_unavailable_uses_core_describe() {
        let _g = openpdf_core::test_support::locale_test_guard();
        set_locale("en");
        let err = Error::signer_unavailable(openpdf_core::SignerUnavailableReason::ModuleNotFound);
        assert_eq!(format_core_error(&err), "PKCS#11 module not available");
        assert_eq!(
            format_core_error(&err),
            describe_signer_unavailable(&openpdf_core::SignerUnavailableReason::ModuleNotFound)
        );
        set_locale("es");
        assert_eq!(
            format_core_error(&err),
            describe_signer_unavailable(&openpdf_core::SignerUnavailableReason::ModuleNotFound)
        );
        assert_ne!(format_core_error(&err), "PKCS#11 module not available");
        set_locale("en");
    }
}
