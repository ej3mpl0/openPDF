use zeroize::Zeroizing;

use crate::error::Result;

/// Supplies a token PIN when a [`crate::Signer`] must log in.
///
/// The UI implements this (a dialog). Core never stores the PIN on
/// [`crate::SignRequest`], [`crate::Document`], or any other persistent type.
/// The value lives only in [`SecretPin`] and is wiped when that wrapper is dropped.
/// Prompt text for the dialog is [`crate::pin_prompt_text`].
pub trait PinProvider: Send + Sync {
    fn request_pin(&self, token_label: &str) -> Result<SecretPin>;
}

/// UTF-8 PIN that is wiped from memory when dropped.
///
/// This is a transient holder, not a stored field. Construct it in a
/// [`PinProvider`] and hand it to the signer backend for login only.
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

impl PinProvider for std::sync::Arc<dyn PinProvider> {
    fn request_pin(&self, token_label: &str) -> Result<SecretPin> {
        (**self).request_pin(token_label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_pin_exposes_then_drops() {
        let pin = SecretPin::new("1234");
        assert_eq!(pin.as_str(), "1234");
        assert_eq!(pin.as_bytes(), b"1234");
        drop(pin);
    }
}
