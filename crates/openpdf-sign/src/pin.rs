use openpdf_core::Result;
use zeroize::Zeroizing;

/// Supplies a token PIN when [`crate::Pkcs11Signer`] must log in.
///
/// The PIN is collected here — never on [`openpdf_core::SignRequest`] or any
/// other core/UI type — and wrapped in [`SecretPin`] so it is zeroized after use.
/// Prompt text for the user is [`crate::pin_prompt_text`].
pub trait PinProvider: Send + Sync {
    fn request_pin(&self, token_label: &str) -> Result<SecretPin>;
}

/// UTF-8 PIN that is wiped from memory when dropped.
pub struct SecretPin(Zeroizing<String>);

impl SecretPin {
    pub fn new(pin: impl Into<String>) -> Self {
        Self(Zeroizing::new(pin.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

/// Test / CLI helper that always returns the same PIN.
pub struct StaticPinProvider {
    pin: Zeroizing<String>,
}

impl StaticPinProvider {
    pub fn new(pin: impl Into<String>) -> Self {
        Self {
            pin: Zeroizing::new(pin.into()),
        }
    }
}

impl PinProvider for StaticPinProvider {
    fn request_pin(&self, _token_label: &str) -> Result<SecretPin> {
        Ok(SecretPin::new(self.pin.as_str()))
    }
}

impl PinProvider for std::sync::Arc<dyn PinProvider> {
    fn request_pin(&self, token_label: &str) -> Result<SecretPin> {
        (**self).request_pin(token_label)
    }
}
