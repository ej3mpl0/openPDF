use std::sync::{Arc, Mutex};

use openpdf_core::{Error as CoreError, PinProvider as CorePinProvider, Result as CoreResult};

use crate::error::FfiError;

/// PIN collected by Swift / C# and handed to Rust for PKCS#11 login only.
///
/// The value is wiped when dropped. Foreign code constructs this from its
/// own dialog; Rust never logs it.
#[derive(uniffi::Object)]
pub struct SecretPin {
    inner: Mutex<Option<openpdf_core::SecretPin>>,
}

#[uniffi::export]
impl SecretPin {
    #[uniffi::constructor]
    pub fn new(value: String) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Some(openpdf_core::SecretPin::new(value))),
        })
    }
}

impl SecretPin {
    pub(crate) fn take_nonempty(&self) -> Result<openpdf_core::SecretPin, FfiError> {
        let mut slot = self.inner.lock().map_err(|_| FfiError::lock_poisoned())?;
        let pin = slot.take().ok_or_else(FfiError::pin_cancelled)?;
        if pin.as_str().is_empty() {
            return Err(FfiError::pin_cancelled());
        }
        Ok(pin)
    }
}

/// Foreign PIN dialog. Signing stays in Rust; only the PIN crosses the FFI.
///
/// Return [`SecretPin`] from the platform dialog. Cancel or an empty field
/// must be [`FfiError::PinCancelled`] — never an empty PIN.
#[uniffi::export(callback_interface)]
pub trait PinProvider: Send + Sync {
    fn request_pin(&self, token_label: String) -> Result<Arc<SecretPin>, FfiError>;
}

/// Adapts the UniFFI callback to [`openpdf_core::PinProvider`].
pub(crate) struct ForeignPin {
    inner: Box<dyn PinProvider>,
}

impl ForeignPin {
    pub(crate) fn new(inner: Box<dyn PinProvider>) -> Self {
        Self { inner }
    }
}

impl CorePinProvider for ForeignPin {
    fn request_pin(&self, token_label: &str) -> CoreResult<openpdf_core::SecretPin> {
        match self.inner.request_pin(token_label.to_string()) {
            Ok(pin) => match pin.take_nonempty() {
                Ok(secret) => Ok(secret),
                Err(FfiError::PinCancelled { .. }) => Err(CoreError::PinCancelled),
                Err(e) => Err(CoreError::Sign(e.to_string())),
            },
            Err(FfiError::PinCancelled { .. }) => Err(CoreError::PinCancelled),
            Err(e) => Err(CoreError::Sign(e.to_string())),
        }
    }
}

/// Test / CI PIN from `OPENPDF_PKCS11_PIN`. Not compiled into release builds.
/// Empty or unset is cancel — never an empty [`SecretPin`].
#[cfg(test)]
pub(crate) struct EnvPinProvider;

#[cfg(test)]
impl PinProvider for EnvPinProvider {
    fn request_pin(&self, _token_label: String) -> Result<Arc<SecretPin>, FfiError> {
        match std::env::var("OPENPDF_PKCS11_PIN") {
            Ok(value) if !value.is_empty() => Ok(SecretPin::new(value)),
            _ => Err(FfiError::pin_cancelled()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_pin_cancels_when_unset_or_empty() {
        let prev = std::env::var_os("OPENPDF_PKCS11_PIN");
        std::env::remove_var("OPENPDF_PKCS11_PIN");
        assert!(matches!(
            EnvPinProvider.request_pin("tok".into()),
            Err(FfiError::PinCancelled { .. })
        ));

        std::env::set_var("OPENPDF_PKCS11_PIN", "");
        assert!(matches!(
            EnvPinProvider.request_pin("tok".into()),
            Err(FfiError::PinCancelled { .. })
        ));

        std::env::set_var("OPENPDF_PKCS11_PIN", "1234");
        let pin = EnvPinProvider.request_pin("tok".into()).unwrap();
        let core = pin.take_nonempty().unwrap();
        assert_eq!(core.as_str().len(), 4);

        match prev {
            Some(v) => std::env::set_var("OPENPDF_PKCS11_PIN", v),
            None => std::env::remove_var("OPENPDF_PKCS11_PIN"),
        }
    }

    #[test]
    fn empty_secret_pin_is_cancelled() {
        let pin = SecretPin::new(String::new());
        assert!(matches!(
            pin.take_nonempty(),
            Err(FfiError::PinCancelled { .. })
        ));
    }

    struct CancelPin;

    impl PinProvider for CancelPin {
        fn request_pin(&self, _token_label: String) -> Result<Arc<SecretPin>, FfiError> {
            Err(FfiError::pin_cancelled())
        }
    }

    struct EmptyPin;

    impl PinProvider for EmptyPin {
        fn request_pin(&self, _token_label: String) -> Result<Arc<SecretPin>, FfiError> {
            Ok(SecretPin::new(String::new()))
        }
    }

    #[test]
    fn foreign_pin_cancel_maps_to_core_pin_cancelled() {
        let cancel = ForeignPin::new(Box::new(CancelPin));
        assert!(matches!(
            cancel.request_pin("tok"),
            Err(CoreError::PinCancelled)
        ));

        let empty = ForeignPin::new(Box::new(EmptyPin));
        assert!(matches!(
            empty.request_pin("tok"),
            Err(CoreError::PinCancelled)
        ));
    }
}
