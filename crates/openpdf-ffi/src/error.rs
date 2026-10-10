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
            CoreError::Signed { .. } => Self::Signed { message },
            // `Error` is `#[non_exhaustive]`; unknown variants still carry
            // the translated message so a UI can show *something*.
            _ => Self::Sign { message },
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
}
