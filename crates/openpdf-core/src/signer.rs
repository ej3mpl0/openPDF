use crate::document::Document;
use crate::error::Result;

/// Visible appearance for a certificate signature. Coordinates are PDF points.
#[derive(Clone, Debug, Default)]
pub struct VisibleAppearance {
    pub page: u32,
    pub rect: [f32; 4],
}

/// Options passed from the UI to a [`Signer`] implementation (PKCS#11 lives in `openpdf-sign`).
#[derive(Clone, Debug, Default)]
pub struct SignRequest {
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact: Option<String>,
    pub appearance: Option<VisibleAppearance>,
}

/// Information about an existing signature dictionary. Validity is left to the signer backend.
#[derive(Clone, Debug, Default)]
pub struct SignatureInfo {
    pub name: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub valid: Option<bool>,
}

/// Certificate / PKCS#11 signing is implemented in `openpdf-sign`.
/// GTK and the signing crate talk only through this trait — never to each other.
pub trait Signer: Send + Sync {
    fn sign(&self, pdf: &[u8], request: &SignRequest) -> Result<Vec<u8>>;
    fn verify(&self, pdf: &[u8]) -> Result<Vec<SignatureInfo>>;
}

impl Document {
    /// Replace this document with the bytes returned by `signer`.
    pub fn apply_signer(&mut self, signer: &dyn Signer, request: &SignRequest) -> Result<()> {
        let signed = signer.sign(&self.to_bytes()?, request)?;
        *self = Document::from_bytes(signed, self.path().map(|p| p.to_path_buf()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::corpus;

    struct IdentitySigner;

    impl Signer for IdentitySigner {
        fn sign(&self, pdf: &[u8], _: &SignRequest) -> Result<Vec<u8>> {
            Ok(pdf.to_vec())
        }

        fn verify(&self, _: &[u8]) -> Result<Vec<SignatureInfo>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn apply_identity_signer_roundtrip() {
        let mut doc = Document::open(corpus("hello.pdf")).unwrap();
        let before = doc.page_count();
        doc.apply_signer(&IdentitySigner, &SignRequest::default())
            .unwrap();
        assert_eq!(doc.page_count(), before);
    }
}
