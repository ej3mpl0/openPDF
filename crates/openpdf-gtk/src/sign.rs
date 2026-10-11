// SPDX-License-Identifier: MPL-2.0
//! Signing UI helpers. GTK never depends on `openpdf-sign`.
//!
//! Certificates come from [`Signer::list_certificates`]. The picker only
//! forwards [`SignRequest::cert_id`]. PIN collection is [`crate::GtkPinProvider`].
//! Certificate / verify text comes from `openpdf-core`.

use openpdf_core::{
    CertificateInfo, CertificateUsage, Error, InvalidKind, Result, SignRequest, SignatureInfo,
    Signer, Verifier,
};

pub use openpdf_core::{
    certificate_common_name, describe_certificate, describe_certificate_detail,
    format_verify_result, format_verify_results, pin_prompt_text, usage_label,
};

/// Empty signer for tests and `--measure`. The application binary injects
/// `Pkcs11Signer` instead.
pub struct UnavailableSigner;

impl Signer for UnavailableSigner {
    fn list_certificates(&self) -> Result<Vec<CertificateInfo>> {
        Ok(Vec::new())
    }

    fn sign(&self, _pdf: &[u8], request: &SignRequest) -> Result<Vec<u8>> {
        if request.cert_id.trim().is_empty() {
            return Err(Error::invalid(InvalidKind::CertIdRequired));
        }
        Err(Error::Sign(
            "signing is provided by openpdf-sign through core's Signer; this UI does not link that crate or ask for a PIN".into(),
        ))
    }

    fn verify(&self, _pdf: &[u8]) -> Result<Vec<SignatureInfo>> {
        Ok(Vec::new())
    }
}

/// Empty verifier for tests and `--measure`. The application binary injects
/// an `openpdf-sign` implementation (`PdfVerifier`) instead.
pub struct NoopVerifier;

impl Verifier for NoopVerifier {
    fn verify(&self, _pdf: &[u8]) -> Result<Vec<SignatureInfo>> {
        Ok(Vec::new())
    }
}

/// Only [`CertificateUsage::Signing`] certificates can be used for PAdES.
pub fn certificate_can_sign(cert: &CertificateInfo) -> bool {
    cert.usage.contains(&CertificateUsage::Signing)
}

/// Build the request the UI is allowed to send. No PIN field exists here.
pub fn sign_request_for(cert_id: impl Into<String>) -> SignRequest {
    SignRequest::new(cert_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openpdf_core::test_support::with_locale;
    use openpdf_core::CertificateValidity;

    fn dnie_certs() -> Vec<CertificateInfo> {
        vec![
            CertificateInfo {
                id: "dnie-auth".into(),
                subject: "CN=CITIZEN (AUTHENTICATION)".into(),
                issuer: "CN=AC DNIE 001".into(),
                validity: CertificateValidity::default(),
                usage: vec![CertificateUsage::Authentication],
            },
            CertificateInfo {
                id: "dnie-sign".into(),
                subject: "CN=CITIZEN (SIGNING)".into(),
                issuer: "CN=AC DNIE 001".into(),
                validity: CertificateValidity::default(),
                usage: vec![CertificateUsage::Signing],
            },
        ]
    }

    #[test]
    fn unavailable_signer_lists_nothing_and_never_asks_pin() {
        let s = UnavailableSigner;
        assert!(s.list_certificates().unwrap().is_empty());
        let req = sign_request_for("dnie-sign");
        assert_eq!(req.cert_id, "dnie-sign");
        assert!(s.sign(b"%PDF", &req).is_err());
        assert!(s.sign(b"%PDF", &SignRequest::new("")).is_err());
    }

    #[test]
    fn only_signing_usage_is_pickable() {
        let certs = dnie_certs();
        assert!(!certificate_can_sign(&certs[0]));
        assert!(certificate_can_sign(&certs[1]));
        assert_eq!(sign_request_for(&certs[1].id).cert_id, "dnie-sign");
    }

    #[test]
    fn describe_and_verify_use_english_msgids() {
        with_locale("en", || {
            let cert = &dnie_certs()[1];
            assert_eq!(
                describe_certificate(cert),
                "CN=CITIZEN (SIGNING) — Signing — validity unknown"
            );
            let info = SignatureInfo {
                name: Some("Ana".into()),
                reason: Some("agree".into()),
                location: Some("Madrid".into()),
                valid: Some(true),
            };
            assert_eq!(
                format_verify_result(&info),
                "Signer: Ana; Reason: agree; Location: Madrid; Valid"
            );
            assert_eq!(format_verify_results(&[]), "No signatures found");
        });
    }
}
