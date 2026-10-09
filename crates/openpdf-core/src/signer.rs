use lopdf::{Dictionary, Document as LopdfDocument, Object};

use crate::document::Document;
use crate::error::{Error, InvalidKind, Result};

/// Visible appearance for a certificate signature. Coordinates are PDF points.
#[derive(Clone, Debug, Default)]
pub struct VisibleAppearance {
    pub page: u32,
    pub rect: [f32; 4],
}

/// How a certificate on a token (DNIe, FNMT, …) may be used.
///
/// A DNIe typically exposes several certificates; only [`CertificateUsage::Signing`]
/// is appropriate for PAdES. Authentication and encryption stay listed so the UI
/// can show them as disabled instead of hiding them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CertificateUsage {
    Authentication,
    Signing,
    Encryption,
    Other(String),
}

/// Validity window as reported by the token. Dates are RFC 3339 when the backend knows them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CertificateValidity {
    pub not_before: Option<String>,
    pub not_after: Option<String>,
}

/// Descriptor for one certificate on a token. `id` is what [`SignRequest::cert_id`] must carry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertificateInfo {
    pub id: String,
    pub subject: String,
    pub issuer: String,
    pub validity: CertificateValidity,
    pub usage: Vec<CertificateUsage>,
}

/// Options passed from the UI to a [`Signer`] implementation (PKCS#11 lives in `openpdf-sign`).
///
/// The PIN is collected and used **only** inside the signer backend. It must never
/// appear on this struct, on [`Signer`], or in any other core/UI API.
#[derive(Clone, Debug, Default)]
pub struct SignRequest {
    /// Token certificate id from [`Signer::list_certificates`].
    pub cert_id: String,
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
///
/// PIN entry, PKCS#11 login, and raw key handles stay inside the implementor.
pub trait Signer: Send + Sync {
    /// Certificates available on the token. A DNIe returns several; the UI picks by `id`.
    fn list_certificates(&self) -> Result<Vec<CertificateInfo>>;

    fn sign(&self, pdf: &[u8], request: &SignRequest) -> Result<Vec<u8>>;
    fn verify(&self, pdf: &[u8]) -> Result<Vec<SignatureInfo>>;
}

impl Document {
    /// Replace this document with the bytes returned by `signer`.
    ///
    /// Already-signed documents are passed through without a full rewrite so the
    /// existing signature bytes stay intact; the backend must append incrementally.
    pub fn apply_signer(&mut self, signer: &dyn Signer, request: &SignRequest) -> Result<()> {
        if request.cert_id.trim().is_empty() {
            return Err(Error::invalid(InvalidKind::CertIdRequired));
        }
        let input = self.bytes_for_signer()?;
        let signed = signer.sign(&input, request)?;
        *self = Document::from_bytes(signed, self.path().map(|p| p.to_path_buf()))?;
        Ok(())
    }

    /// Bytes handed to a [`Signer`]: rewrite only when the document is still unsigned.
    pub(crate) fn bytes_for_signer(&self) -> Result<Vec<u8>> {
        if self.is_signed() {
            if self.is_dirty() {
                self.to_incremental_bytes()
            } else {
                Ok(self.original_bytes().to_vec())
            }
        } else if self.is_dirty() {
            self.to_bytes()
        } else {
            Ok(self.original_bytes().to_vec())
        }
    }
}

pub(crate) fn contains_signature(doc: &LopdfDocument) -> bool {
    doc.objects
        .values()
        .any(|object| object.as_dict().is_ok_and(is_signature_dict))
}

pub(crate) fn strip_signatures(doc: &mut LopdfDocument) {
    let kill: Vec<_> = doc
        .objects
        .iter()
        .filter(|(_, object)| object.as_dict().is_ok_and(is_signature_dict))
        .map(|(id, _)| *id)
        .collect();
    for id in kill {
        doc.objects.remove(&id);
    }
    if let Ok(catalog) = doc.catalog_mut() {
        catalog.remove(b"Perms");
    }
}

fn is_signature_dict(dict: &Dictionary) -> bool {
    match dict.get(b"Type") {
        Ok(Object::Name(name)) if name.as_slice() == b"Sig" => return true,
        _ => {}
    }
    match dict.get(b"FT") {
        Ok(Object::Name(name)) if name.as_slice() == b"Sig" => return true,
        _ => {}
    }
    false
}

/// Test helper: append a `/Type /Sig` dictionary without touching the original bytes prefix.
#[cfg(test)]
pub(crate) fn append_stub_signature(pdf: &[u8]) -> Result<Vec<u8>> {
    use lopdf::IncrementalDocument;

    let prev = LopdfDocument::load_mem(pdf)?;
    let mut inc = IncrementalDocument::create_from(pdf.to_vec(), prev);
    let mut dict = Dictionary::new();
    dict.set("Type", Object::Name(b"Sig".to_vec()));
    dict.set("Filter", Object::Name(b"Adobe.PPKLite".to_vec()));
    dict.set(
        "SubFilter",
        Object::Name(b"adbe.pkcs7.detached".to_vec()),
    );
    dict.set(
        "ByteRange",
        Object::Array(vec![0.into(), 0.into(), 0.into(), 0.into()]),
    );
    dict.set("Contents", Object::string_literal(""));
    inc.new_document.add_object(Object::Dictionary(dict));
    let mut out = Vec::new();
    inc.save_to(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::test_support::corpus;

    fn dnie_certs() -> Vec<CertificateInfo> {
        vec![
            CertificateInfo {
                id: "dnie-auth".into(),
                subject: "CN=CITIZEN (AUTHENTICATION)".into(),
                issuer: "CN=AC DNIE 001".into(),
                validity: CertificateValidity {
                    not_before: Some("2024-01-01T00:00:00Z".into()),
                    not_after: Some("2028-01-01T00:00:00Z".into()),
                },
                usage: vec![CertificateUsage::Authentication],
            },
            CertificateInfo {
                id: "dnie-sign".into(),
                subject: "CN=CITIZEN (SIGNATURE)".into(),
                issuer: "CN=AC DNIE 001".into(),
                validity: CertificateValidity {
                    not_before: Some("2024-01-01T00:00:00Z".into()),
                    not_after: Some("2028-01-01T00:00:00Z".into()),
                },
                usage: vec![CertificateUsage::Signing],
            },
        ]
    }

    struct TokenSigner {
        certs: Vec<CertificateInfo>,
    }

    impl Signer for TokenSigner {
        fn list_certificates(&self) -> Result<Vec<CertificateInfo>> {
            Ok(self.certs.clone())
        }

        fn sign(&self, pdf: &[u8], request: &SignRequest) -> Result<Vec<u8>> {
            let cert = self
                .certs
                .iter()
                .find(|c| c.id == request.cert_id)
                .ok_or_else(|| Error::Sign("unknown certificate".into()))?;
            if !cert.usage.contains(&CertificateUsage::Signing) {
                return Err(Error::Sign("certificate is not a signing cert".into()));
            }
            append_stub_signature(pdf)
        }

        fn verify(&self, pdf: &[u8]) -> Result<Vec<SignatureInfo>> {
            let doc = LopdfDocument::load_mem(pdf)?;
            if contains_signature(&doc) {
                Ok(vec![SignatureInfo {
                    name: Some("stub".into()),
                    ..SignatureInfo::default()
                }])
            } else {
                Ok(Vec::new())
            }
        }
    }

    fn signing_request() -> SignRequest {
        SignRequest {
            cert_id: "dnie-sign".into(),
            reason: Some("test".into()),
            location: None,
            contact: None,
            appearance: None,
        }
    }

    #[test]
    fn list_certificates_exposes_dnie_slots_without_pin() {
        let signer = TokenSigner {
            certs: dnie_certs(),
        };
        let certs = signer.list_certificates().unwrap();
        assert_eq!(certs.len(), 2);
        assert_eq!(certs[0].id, "dnie-auth");
        assert_eq!(certs[1].usage, vec![CertificateUsage::Signing]);
        // PIN is not a field on SignRequest or CertificateInfo — constructing
        // these values is the API contract the UI is allowed to use.
        let req = signing_request();
        assert_eq!(req.cert_id, "dnie-sign");
        assert!(req.reason.is_some());
    }

    #[test]
    fn apply_signer_requires_cert_id() {
        let mut doc = Document::open(corpus("hello.pdf")).unwrap();
        let signer = TokenSigner {
            certs: dnie_certs(),
        };
        let err = doc
            .apply_signer(&signer, &SignRequest::default())
            .unwrap_err();
        assert!(matches!(err, Error::Invalid(_)));
    }

    #[test]
    fn apply_signer_locks_destructive_edits_and_full_rewrite() {
        let mut doc = Document::open(corpus("two_page.pdf")).unwrap();
        assert!(!doc.is_signed());
        assert!(!doc.is_read_only());
        let signer = TokenSigner {
            certs: dnie_certs(),
        };
        doc.apply_signer(&signer, &signing_request()).unwrap();
        assert!(doc.is_signed());
        assert!(doc.is_read_only());
        assert!(matches!(
            doc.rotate_page(0, 90),
            Err(Error::Signed {
                op: crate::error::SignedOp::Rotate
            })
        ));
        assert!(matches!(
            doc.delete_page(0),
            Err(Error::Signed {
                op: crate::error::SignedOp::Delete
            })
        ));
        assert!(matches!(
            doc.move_page(0, 2),
            Err(Error::Signed {
                op: crate::error::SignedOp::Move
            })
        ));
        assert!(matches!(
            doc.to_bytes(),
            Err(Error::Signed {
                op: crate::error::SignedOp::Rewrite
            })
        ));
        assert!(matches!(
            doc.save(std::env::temp_dir().join("openpdf-signed-rewrite.pdf")),
            Err(Error::Signed {
                op: crate::error::SignedOp::Rewrite
            })
        ));
        let incremental = doc.to_incremental_bytes().unwrap();
        assert!(incremental.starts_with(doc.original_bytes()));
        assert!(contains_signature(
            &LopdfDocument::load_mem(&incremental).unwrap()
        ));
    }

    #[test]
    fn second_signature_is_incremental() {
        let mut doc = Document::open(corpus("hello.pdf")).unwrap();
        let signer = TokenSigner {
            certs: dnie_certs(),
        };
        doc.apply_signer(&signer, &signing_request()).unwrap();
        let after_first = doc.original_bytes().to_vec();
        doc.apply_signer(&signer, &signing_request()).unwrap();
        assert!(doc.original_bytes().starts_with(&after_first));
        assert!(doc.is_signed());
    }
}
