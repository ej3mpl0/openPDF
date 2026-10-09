use openpdf_core::{PinProvider, Result, SecretPin};
use zeroize::Zeroizing;

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
