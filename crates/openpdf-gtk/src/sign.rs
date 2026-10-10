// SPDX-License-Identifier: MPL-2.0
//! Signing UI helpers. GTK never depends on `openpdf-sign`.
//!
//! Certificates come from [`Signer::list_certificates`]. The picker only
//! forwards [`SignRequest::cert_id`]. PIN collection is [`crate::GtkPinProvider`].
//! Certificate / verify text comes from `openpdf-core`.

use openpdf_core::{
    CertificateInfo, Error, InvalidKind, Result, SignRequest, SignatureInfo, Signer, Verifier,
};

pub use openpdf_core::{
    describe_certificate, describe_pin_status, describe_unusable, format_verify_result,
    format_verify_results, pin_prompt_text, usage_label, CertUnusable, TokenPinStatus,
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

/// Pickable when core did not mark the cert unusable and the token PIN is not locked.
pub fn certificate_can_sign(cert: &CertificateInfo) -> bool {
    cert.unusable.is_none() && !cert.pin_status.is_some_and(|s| s.locked)
}

/// Core's translated reason. The `_` arm is required because
/// [`CertUnusable`] is `#[non_exhaustive]`.
pub fn unusable_reason(reason: &CertUnusable) -> String {
    match reason {
        CertUnusable::AuthenticationOnly
        | CertUnusable::Expired { .. }
        | CertUnusable::NotYetValid { .. }
        | CertUnusable::UnsupportedKey => describe_unusable(reason),
        _ => describe_unusable(reason),
    }
}

/// Build the request the UI is allowed to send. No PIN field exists here.
pub fn sign_request_for(cert_id: impl Into<String>) -> SignRequest {
    SignRequest {
        cert_id: cert_id.into(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openpdf_core::test_support::with_locale;
    use openpdf_core::{CertificateUsage, CertificateValidity};

    fn dnie_certs() -> Vec<CertificateInfo> {
        vec![
            CertificateInfo {
                id: "dnie-auth".into(),
                subject: "CN=CITIZEN (AUTHENTICATION)".into(),
                issuer: "CN=AC DNIE 001".into(),
                validity: CertificateValidity::default(),
                usage: vec![CertificateUsage::Authentication],
                unusable: Some(CertUnusable::AuthenticationOnly),
                pin_status: None,
            },
            CertificateInfo {
                id: "dnie-sign".into(),
                subject: "CN=CITIZEN (SIGNING)".into(),
                issuer: "CN=AC DNIE 001".into(),
                validity: CertificateValidity::default(),
                usage: vec![CertificateUsage::Signing],
                unusable: None,
                pin_status: None,
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
        assert!(s.sign(b"%PDF", &SignRequest::default()).is_err());
    }

    #[test]
    fn unusable_certs_are_not_pickable() {
        let certs = dnie_certs();
        assert!(!certificate_can_sign(&certs[0]));
        assert!(certificate_can_sign(&certs[1]));
        assert_eq!(sign_request_for(&certs[1].id).cert_id, "dnie-sign");
        with_locale("en", || {
            let reason = certs[0].unusable.as_ref().unwrap();
            assert_eq!(unusable_reason(reason), describe_unusable(reason));
            let expired = CertUnusable::Expired {
                not_after: "2020-01-01T00:00:00Z".into(),
            };
            assert_eq!(unusable_reason(&expired), describe_unusable(&expired));
        });
    }

    #[test]
    fn locked_token_is_not_pickable() {
        let mut cert = dnie_certs().remove(1);
        cert.pin_status = Some(TokenPinStatus::new(false, false, true, false));
        assert!(!certificate_can_sign(&cert));
        with_locale("en", || {
            assert_eq!(
                describe_pin_status(cert.pin_status.as_ref().unwrap()).as_deref(),
                Some("Token PIN is locked")
            );
            let last = TokenPinStatus::new(false, true, false, false);
            assert_eq!(
                describe_pin_status(&last).as_deref(),
                Some("Last PIN attempt")
            );
        });
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
