// SPDX-License-Identifier: MPL-2.0
//! Signing UI helpers. GTK never depends on `openpdf-sign`.
//!
//! Certificates come from [`Signer::list_certificates`]. The picker builds a
//! [`SignRequest`] (cert, reason, location, PAdES level, cancel token). PIN
//! collection is [`crate::GtkPinProvider`]. Certificate / verify text comes
//! from `openpdf-core`.

use openpdf_core::{
    CancelToken, CertificateInfo, CertificateUsage, Error, InvalidKind, PadesLevel, Result,
    SignRequest, SignatureInfo, Signer, Verifier,
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
        request.check_cancelled()?;
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

/// Certificate picker → [`SignRequest`]. Empty reason/location stay unset.
/// `level` is never rewritten: B-T stays B-T (the signer refuses it until TSA).
pub fn build_sign_request(
    cert_id: impl Into<String>,
    reason: &str,
    location: &str,
    level: PadesLevel,
    cancel: CancelToken,
) -> SignRequest {
    let mut req = SignRequest::new(cert_id)
        .with_level(level)
        .with_cancel_token(cancel);
    let reason = reason.trim();
    if !reason.is_empty() {
        req = req.with_reason(reason);
    }
    let location = location.trim();
    if !location.is_empty() {
        req = req.with_location(location);
    }
    req
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
    fn build_sign_request_sets_reason_location_level_and_token() {
        let token = CancelToken::new();
        let req = build_sign_request(
            "dnie-sign",
            "  Approved  ",
            "Madrid",
            PadesLevel::BB,
            token.clone(),
        );
        assert_eq!(req.cert_id, "dnie-sign");
        assert_eq!(req.reason.as_deref(), Some("Approved"));
        assert_eq!(req.location.as_deref(), Some("Madrid"));
        assert_eq!(req.level, PadesLevel::BB);
        assert!(!req.cancel_token().expect("token").is_cancelled());
        token.cancel();
        assert!(req.cancel_token().expect("token").is_cancelled());
        assert!(matches!(req.check_cancelled(), Err(Error::Cancelled)));
        assert!(!matches!(req.check_cancelled(), Err(Error::PinCancelled)));
    }

    #[test]
    fn build_sign_request_never_silently_falls_back_from_bt() {
        let req = build_sign_request("c", "", "", PadesLevel::BT, CancelToken::new());
        assert_eq!(req.level, PadesLevel::BT);
        assert!(req.reason.is_none());
        assert!(req.location.is_none());
    }

    #[test]
    fn only_signing_usage_is_pickable() {
        let certs = dnie_certs();
        assert!(!certificate_can_sign(&certs[0]));
        assert!(certificate_can_sign(&certs[1]));
        assert_eq!(sign_request_for(&certs[1].id).cert_id, "dnie-sign");
    }

    #[test]
    fn cancel_during_fake_slow_tsa_is_silent_and_leaves_pdf_untouched() {
        use std::time::{Duration, Instant};

        struct SlowTsa;
        impl Signer for SlowTsa {
            fn list_certificates(&self) -> Result<Vec<CertificateInfo>> {
                Ok(Vec::new())
            }
            fn sign(&self, pdf: &[u8], request: &SignRequest) -> Result<Vec<u8>> {
                request.check_cancelled()?;
                request.check_cancelled()?;
                request
                    .cancel_token()
                    .cloned()
                    .unwrap_or_default()
                    .wait(Duration::from_secs(5))?;
                Ok(pdf.to_vec())
            }
            fn verify(&self, _pdf: &[u8]) -> Result<Vec<SignatureInfo>> {
                Ok(Vec::new())
            }
        }

        let pdf = b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n";
        let token = CancelToken::new();
        let req = build_sign_request("c", "motivo", "lugar", PadesLevel::BT, token.clone());
        let handle = std::thread::spawn({
            let token = token.clone();
            move || {
                std::thread::sleep(Duration::from_millis(80));
                token.cancel();
                Instant::now()
            }
        });
        let err = SlowTsa.sign(pdf, &req).unwrap_err();
        let cancelled_at = handle.join().unwrap();
        let noticed = Instant::now().saturating_duration_since(cancelled_at);
        assert!(
            noticed < openpdf_core::CANCEL_NOTICE,
            "fake TSA must notice cancel within 200 ms, took {noticed:?}"
        );
        assert!(matches!(err, Error::Cancelled));
        assert!(err.is_silent_cancel());
        assert!(!matches!(err, Error::PinCancelled));
    }

    #[test]
    fn describe_and_verify_use_english_msgids() {
        with_locale("en", || {
            let cert = &dnie_certs()[1];
            assert_eq!(
                describe_certificate(cert),
                "CN=CITIZEN (SIGNING) — Signing — validity unknown"
            );
            let info = SignatureInfo::new(
                Some("Ana".into()),
                Some("agree".into()),
                Some("Madrid".into()),
            )
            .with_valid(Some(true));
            assert_eq!(
                format_verify_result(&info),
                "Signer: Ana; Reason: agree; Location: Madrid; Valid"
            );
            assert_eq!(format_verify_results(&[]), "No signatures found");
        });
    }
}
