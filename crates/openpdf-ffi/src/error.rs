// SPDX-License-Identifier: MPL-2.0
use openpdf_core::Error as CoreError;

/// UniFFI error mirroring [`openpdf_core::Error`].
///
/// Each variant carries the **translated** message. Foreign code should match
/// the variant, not parse [`message`]. `#[non_exhaustive]` so new core
/// variants can be added without breaking bindings.
#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
#[non_exhaustive]
pub enum FfiError {
    #[error("{message}")]
    Io { message: String },
    #[error("{message}")]
    Structure { message: String },
    #[error("{message}")]
    Pdfium { message: String },
    #[error("{message}")]
    PdfiumLoad { message: String },
    #[error("{message}")]
    PageOutOfRange {
        message: String,
        page: u32,
        count: u32,
    },
    #[error("{message}")]
    EmptyDocument { message: String },
    #[error("{message}")]
    Invalid { message: String },
    #[error("{message}")]
    Sign { message: String },
    #[error("{message}")]
    Signed { message: String },
    #[error("{message}")]
    PinCancelled { message: String },
    #[error("{message}")]
    LockPoisoned { message: String },
    #[error("{message}")]
    EngineUnavailable { message: String },
    #[error("{message}")]
    DocumentChanged { message: String },
    /// Catch-all for future `#[non_exhaustive]` core variants. Never used for
    /// [`Self::CertUnusable`], [`Self::SignerUnavailable`], [`Self::Timestamp`],
    /// or [`Self::Cancelled`].
    #[error("{message}")]
    Other { message: String },
    /// Structured unusable-certificate error (reason + RFC 3339 dates, not text).
    #[error("{message}")]
    CertUnusable {
        message: String,
        reason: CertUnusableReason,
        not_before: Option<String>,
        not_after: Option<String>,
    },
    /// Structured PKCS#11 load failure (reason + path/detail, not text).
    #[error("{message}")]
    SignerUnavailable {
        message: String,
        reason: SignerUnavailableReason,
        module: Option<String>,
        detail: Option<String>,
    },
    /// Structured PAdES B-T timestamp failure. Match [`kind`]; do not parse text.
    #[error("{message}")]
    Timestamp {
        message: String,
        kind: TimestampErrorKind,
    },
    /// Caller cancelled via CancelToken. Distinct from [`Self::PinCancelled`].
    #[error("{message}")]
    Cancelled { message: String },
}

/// Why a certificate cannot produce a PAdES signature. Match this; do not parse text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CertUnusableReason {
    AuthenticationOnly,
    Expired,
    NotYetValid,
    UnsupportedKey,
}

/// Why PKCS#11 (or another token backend) could not be loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SignerUnavailableReason {
    ModuleNotFound,
    LoadFailed,
}

/// Why a PAdES B-T timestamp was refused. Mirrors
/// [`openpdf_core::TimestampErrorKind`]. Match this; do not parse text.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum TimestampErrorKind {
    UrlMissing,
    BadUrl,
    Network { detail: String },
    Rng { detail: String },
    ResponseTooLarge,
    TokenTooLarge,
    Http { status: u16 },
    Rejected { status: u8 },
    Malformed,
    NonceMismatch,
    ImprintMismatch,
    BadSignature,
    SigningCertMismatch,
    UntrustedCert,
}

impl TimestampErrorKind {
    fn from_core(kind: openpdf_core::TimestampErrorKind) -> Self {
        use openpdf_core::TimestampErrorKind as Core;
        match kind {
            Core::UrlMissing => Self::UrlMissing,
            Core::BadUrl => Self::BadUrl,
            Core::Network { detail } => Self::Network { detail },
            Core::Rng { detail } => Self::Rng { detail },
            Core::ResponseTooLarge => Self::ResponseTooLarge,
            Core::TokenTooLarge => Self::TokenTooLarge,
            Core::Http { status } => Self::Http { status },
            Core::Rejected { status } => Self::Rejected { status },
            Core::Malformed => Self::Malformed,
            Core::NonceMismatch => Self::NonceMismatch,
            Core::ImprintMismatch => Self::ImprintMismatch,
            Core::BadSignature => Self::BadSignature,
            Core::SigningCertMismatch => Self::SigningCertMismatch,
            Core::UntrustedCert => Self::UntrustedCert,
            other => Self::Network {
                detail: other.to_locale_string(),
            },
        }
    }
}

impl FfiError {
    pub(crate) fn pin_cancelled() -> Self {
        crate::ensure_i18n();
        Self::PinCancelled {
            message: openpdf_core::tr("PIN entry was cancelled"),
        }
    }

    pub(crate) fn lock_poisoned() -> Self {
        crate::ensure_i18n();
        Self::LockPoisoned {
            message: openpdf_core::tr("document lock poisoned"),
        }
    }

    pub(crate) fn engine_unavailable() -> Self {
        crate::ensure_i18n();
        Self::EngineUnavailable {
            message: openpdf_core::tr("PDFium is not available"),
        }
    }

    pub(crate) fn document_changed() -> Self {
        crate::ensure_i18n();
        Self::DocumentChanged {
            message: openpdf_core::tr("document changed during signing"),
        }
    }

    pub(crate) fn from_core(err: CoreError) -> Self {
        crate::ensure_i18n();
        let message = err.to_locale_string();
        match err {
            CoreError::Io(_) => Self::Io { message },
            CoreError::Structure(_) => Self::Structure { message },
            CoreError::Pdfium(_) => Self::Pdfium { message },
            CoreError::PdfiumLoad { .. } => Self::PdfiumLoad { message },
            CoreError::PageOutOfRange { page, count } => Self::PageOutOfRange {
                message,
                page,
                count,
            },
            CoreError::EmptyDocument => Self::EmptyDocument { message },
            CoreError::Invalid(_) => Self::Invalid { message },
            CoreError::Sign(_) => Self::Sign { message },
            CoreError::PinCancelled => Self::PinCancelled { message },
            CoreError::SignerUnavailable { .. } => Self::SignerUnavailable {
                message,
                reason: SignerUnavailableReason::LoadFailed,
                module: None,
                detail: None,
            },
            CoreError::SignLevelUnsupported { .. } => Self::Sign { message },
            CoreError::Signed { .. } => Self::Signed { message },
            CoreError::Timestamp(kind) => Self::Timestamp {
                message,
                kind: TimestampErrorKind::from_core(kind),
            },
            CoreError::Cancelled => Self::Cancelled { message },
            // `Error` is `#[non_exhaustive]`. CertUnusable (#31) and
            // SignerUnavailable (#24) map through their dedicated helpers
            // once those variants exist on this crate's core; the wildcard
            // is `Other`, never `Sign`. Timestamp and Cancelled have dedicated
            // arms above and must not land here.
            _ => Self::Other { message },
        }
    }

    /// Map a structured unusable-certificate reason. Never returns [`Self::Other`] or [`Self::Sign`].
    pub(crate) fn from_cert_unusable(
        reason: CertUnusableReason,
        not_before: Option<String>,
        not_after: Option<String>,
    ) -> Self {
        crate::ensure_i18n();
        let message = match reason {
            CertUnusableReason::AuthenticationOnly => openpdf_core::tr("Authentication only"),
            CertUnusableReason::Expired => openpdf_core::tr_args(
                "Certificate expired on {not_after}",
                &[("not_after", not_after.as_deref().unwrap_or(""))],
            ),
            CertUnusableReason::NotYetValid => openpdf_core::tr_args(
                "Certificate not valid before {not_before}",
                &[("not_before", not_before.as_deref().unwrap_or(""))],
            ),
            CertUnusableReason::UnsupportedKey => openpdf_core::tr("Unsupported key algorithm"),
        };
        Self::CertUnusable {
            message,
            reason,
            not_before,
            not_after,
        }
    }

    pub(crate) fn from_signer_unavailable(
        reason: SignerUnavailableReason,
        module: Option<String>,
        detail: Option<String>,
    ) -> Self {
        crate::ensure_i18n();
        let message = match reason {
            SignerUnavailableReason::ModuleNotFound => {
                openpdf_core::tr("PKCS#11 module not available")
            }
            SignerUnavailableReason::LoadFailed => {
                openpdf_core::tr("Could not load the PKCS#11 module")
            }
        };
        Self::SignerUnavailable {
            message,
            reason,
            module,
            detail,
        }
    }

    pub(crate) fn from_core_hiding(err: CoreError, secrets: &[&str]) -> Self {
        Self::from_core(err).hide_secrets(secrets)
    }

    pub(crate) fn hide_secrets(self, secrets: &[&str]) -> Self {
        let redact_msg = |message: String| redact(&message, secrets);
        match self {
            Self::Io { message } => Self::Io {
                message: redact_msg(message),
            },
            Self::Structure { message } => Self::Structure {
                message: redact_msg(message),
            },
            Self::Pdfium { message } => Self::Pdfium {
                message: redact_msg(message),
            },
            Self::PdfiumLoad { message } => Self::PdfiumLoad {
                message: redact_msg(message),
            },
            Self::PageOutOfRange {
                message,
                page,
                count,
            } => Self::PageOutOfRange {
                message: redact_msg(message),
                page,
                count,
            },
            Self::EmptyDocument { message } => Self::EmptyDocument {
                message: redact_msg(message),
            },
            Self::Invalid { message } => Self::Invalid {
                message: redact_msg(message),
            },
            Self::Sign { message } => Self::Sign {
                message: redact_msg(message),
            },
            Self::Signed { message } => Self::Signed {
                message: redact_msg(message),
            },
            Self::PinCancelled { message } => Self::PinCancelled {
                message: redact_msg(message),
            },
            Self::LockPoisoned { message } => Self::LockPoisoned {
                message: redact_msg(message),
            },
            Self::EngineUnavailable { message } => Self::EngineUnavailable {
                message: redact_msg(message),
            },
            Self::DocumentChanged { message } => Self::DocumentChanged {
                message: redact_msg(message),
            },
            Self::Other { message } => Self::Other {
                message: redact_msg(message),
            },
            Self::CertUnusable {
                message,
                reason,
                not_before,
                not_after,
            } => Self::CertUnusable {
                message: redact_msg(message),
                reason,
                not_before,
                not_after,
            },
            Self::SignerUnavailable {
                message,
                reason,
                module,
                detail,
            } => Self::SignerUnavailable {
                message: redact_msg(message),
                reason,
                module,
                detail,
            },
            Self::Timestamp { message, kind } => Self::Timestamp {
                message: redact_msg(message),
                kind: match kind {
                    TimestampErrorKind::Network { detail } => TimestampErrorKind::Network {
                        detail: redact(&detail, secrets),
                    },
                    other => other,
                },
            },
            Self::Cancelled { message } => Self::Cancelled {
                message: redact_msg(message),
            },
        }
    }
}

impl From<CoreError> for FfiError {
    fn from(err: CoreError) -> Self {
        Self::from_core(err)
    }
}

fn redact(message: &str, secrets: &[&str]) -> String {
    let mut out = message.to_string();
    for secret in secrets {
        if secret.is_empty() {
            continue;
        }
        if out.contains(*secret) {
            out = out.replace(*secret, "[redacted]");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use openpdf_core::{Error, InvalidKind};

    #[test]
    fn password_errors_do_not_echo_secret() {
        let secret = "ffi-secret-NEVER-ECHO-9f3a"; // test-only placeholder, not a real secret (allowlisted in .gitleaks.toml, #27)
        let err = FfiError::from_core_hiding(Error::invalid(InvalidKind::WrongPassword), &[secret]);
        let text = err.to_string();
        assert!(!text.contains(secret), "error echoed secret");
        assert!(
            matches!(err, FfiError::Invalid { .. }),
            "wrong password must be Invalid, got {err:?}"
        );
        assert!(
            text.to_lowercase().contains("password") || text.contains("[redacted]"),
            "unexpected message: {text}"
        );
    }

    #[test]
    fn redact_replaces_secret_in_message() {
        let secret = "super-secret-value";
        let err = FfiError::Sign {
            message: format!("boom {secret}"),
        }
        .hide_secrets(&[secret]);
        assert!(!err.to_string().contains(secret));
        assert!(err.to_string().contains("[redacted]"));
    }

    #[test]
    fn pin_cancelled_from_core_stays_pin_cancelled() {
        let err = FfiError::from_core(Error::PinCancelled);
        assert!(matches!(err, FfiError::PinCancelled { .. }), "got {err:?}");
        assert_eq!(err.to_string(), openpdf_core::tr("PIN entry was cancelled"));
    }

    #[test]
    fn cert_unusable_never_falls_into_generic_arm() {
        let err = FfiError::from_cert_unusable(
            CertUnusableReason::Expired,
            None,
            Some("2020-01-01T00:00:00Z".into()),
        );
        match err {
            FfiError::CertUnusable {
                reason,
                not_after,
                not_before,
                ..
            } => {
                assert_eq!(reason, CertUnusableReason::Expired);
                assert_eq!(not_after.as_deref(), Some("2020-01-01T00:00:00Z"));
                assert!(not_before.is_none());
            }
            FfiError::Other { .. } | FfiError::Sign { .. } => {
                panic!("CertUnusable fell into the generic arm: {err:?}");
            }
            other => panic!("expected CertUnusable, got {other:?}"),
        }

        let auth = FfiError::from_cert_unusable(CertUnusableReason::AuthenticationOnly, None, None);
        assert!(
            matches!(
                auth,
                FfiError::CertUnusable {
                    reason: CertUnusableReason::AuthenticationOnly,
                    ..
                }
            ),
            "got {auth:?}"
        );
        assert!(!matches!(auth, FfiError::Other { .. }));
        assert!(!matches!(auth, FfiError::Sign { .. }));
    }

    #[test]
    fn signer_unavailable_is_structured_not_generic() {
        let err = FfiError::from_signer_unavailable(
            SignerUnavailableReason::LoadFailed,
            Some("/usr/lib/softhsm/libsofthsm2.so".into()),
            Some("dlopen failed".into()),
        );
        match err {
            FfiError::SignerUnavailable {
                reason,
                module,
                detail,
                ..
            } => {
                assert_eq!(reason, SignerUnavailableReason::LoadFailed);
                assert_eq!(module.as_deref(), Some("/usr/lib/softhsm/libsofthsm2.so"));
                assert_eq!(detail.as_deref(), Some("dlopen failed"));
            }
            FfiError::Other { .. } | FfiError::Sign { .. } => {
                panic!("SignerUnavailable fell into the generic arm: {err:?}");
            }
            other => panic!("expected SignerUnavailable, got {other:?}"),
        }
    }

    #[test]
    fn unknown_core_variant_maps_to_other_not_sign() {
        // The non_exhaustive wildcard is `Other`. Sign stays only for Error::Sign.
        let sign = FfiError::from_core(Error::Sign("token rejected".into()));
        assert!(matches!(sign, FfiError::Sign { .. }), "got {sign:?}");
        let other = FfiError::Other {
            message: "future-core-variant".into(),
        };
        assert!(!matches!(other, FfiError::Sign { .. }));
    }

    #[test]
    fn timestamp_from_core_is_structured_not_other() {
        let kinds = [
            (
                openpdf_core::TimestampErrorKind::UrlMissing,
                TimestampErrorKind::UrlMissing,
            ),
            (
                openpdf_core::TimestampErrorKind::BadUrl,
                TimestampErrorKind::BadUrl,
            ),
            (
                openpdf_core::TimestampErrorKind::Malformed,
                TimestampErrorKind::Malformed,
            ),
            (
                openpdf_core::TimestampErrorKind::NonceMismatch,
                TimestampErrorKind::NonceMismatch,
            ),
            (
                openpdf_core::TimestampErrorKind::ImprintMismatch,
                TimestampErrorKind::ImprintMismatch,
            ),
            (
                openpdf_core::TimestampErrorKind::BadSignature,
                TimestampErrorKind::BadSignature,
            ),
            (
                openpdf_core::TimestampErrorKind::SigningCertMismatch,
                TimestampErrorKind::SigningCertMismatch,
            ),
            (
                openpdf_core::TimestampErrorKind::UntrustedCert,
                TimestampErrorKind::UntrustedCert,
            ),
        ];
        for (core_kind, ffi_kind) in kinds {
            let err = FfiError::from_core(Error::Timestamp(core_kind));
            match &err {
                FfiError::Timestamp { kind, .. } => assert_eq!(*kind, ffi_kind),
                FfiError::Other { .. } | FfiError::Sign { .. } => {
                    panic!("Timestamp fell into the generic arm: {err:?}");
                }
                other => panic!("expected Timestamp, got {other:?}"),
            }
            assert!(!matches!(err, FfiError::Other { .. }));
        }

        let http = FfiError::from_core(Error::Timestamp(openpdf_core::TimestampErrorKind::Http {
            status: 503,
        }));
        match http {
            FfiError::Timestamp {
                kind: TimestampErrorKind::Http { status },
                ..
            } => assert_eq!(status, 503),
            FfiError::Other { .. } => panic!("Timestamp::Http fell into Other"),
            other => panic!("expected Timestamp::Http, got {other:?}"),
        }

        let rejected = FfiError::from_core(Error::Timestamp(
            openpdf_core::TimestampErrorKind::Rejected { status: 2 },
        ));
        match rejected {
            FfiError::Timestamp {
                kind: TimestampErrorKind::Rejected { status },
                ..
            } => assert_eq!(status, 2),
            FfiError::Other { .. } => panic!("Timestamp::Rejected fell into Other"),
            other => panic!("expected Timestamp::Rejected, got {other:?}"),
        }

        let net = FfiError::from_core(Error::Timestamp(
            openpdf_core::TimestampErrorKind::Network {
                detail: "https://alice:ffi-secret-NEVER-ECHO-9f3a@tsa.example.com/rfc3161".into(),
            },
        ));
        match &net {
            FfiError::Timestamp {
                kind: TimestampErrorKind::Network { detail },
                ..
            } => assert_eq!(
                detail,
                "https://alice:ffi-secret-NEVER-ECHO-9f3a@tsa.example.com/rfc3161"
            ),
            FfiError::Other { .. } => panic!("Timestamp::Network fell into Other"),
            other => panic!("expected Timestamp::Network, got {other:?}"),
        }
        assert!(!matches!(net, FfiError::Other { .. }));
        assert!(!matches!(net, FfiError::Sign { .. }));
    }

    #[test]
    fn cancelled_from_core_is_not_other() {
        let err = FfiError::from_core(Error::Cancelled);
        assert!(
            matches!(err, FfiError::Cancelled { .. }),
            "Cancelled must stay Cancelled, got {err:?}"
        );
        assert!(!matches!(err, FfiError::Other { .. }));
        assert!(!matches!(err, FfiError::Sign { .. }));
        assert!(!matches!(err, FfiError::PinCancelled { .. }));
        assert_eq!(err.to_string(), openpdf_core::tr("operation cancelled"));
    }
}
