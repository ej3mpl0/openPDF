use lopdf::{Dictionary, Document as LopdfDocument, Object};

use crate::document::Document;
use crate::error::{Error, InvalidKind, Result};
use crate::i18n::{tr, tr_args};

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
/// The PIN is collected through [`crate::PinProvider`] in the UI and used only
/// inside the signer backend. It must never appear on this struct, on
/// [`Signer`], or on [`crate::Document`].
///
/// `#[non_exhaustive]`: build it with [`SignRequest::new`] and the `with_*`
/// methods so new fields do not break downstream crates.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct SignRequest {
    /// Token certificate id from [`Signer::list_certificates`].
    pub cert_id: String,
    /// Written as `/Reason` in the signature dictionary.
    pub reason: Option<String>,
    /// Written as `/Location` in the signature dictionary.
    pub location: Option<String>,
    pub contact: Option<String>,
    pub appearance: Option<VisibleAppearance>,
    /// PAdES baseline level. Backends must refuse levels they cannot produce
    /// rather than silently downgrading.
    pub level: PadesLevel,
    /// RFC 3161 timestamp server for [`PadesLevel::BT`]. Overrides the
    /// backend's configured URL. There is no built-in default.
    pub tsa_url: Option<String>,
    /// Lets the UI abort signing (e.g. while waiting for the TSA). Checked
    /// before the PIN prompt, before the TSA request and while waiting.
    pub cancel: Option<CancelToken>,
}

impl SignRequest {
    pub fn new(cert_id: impl Into<String>) -> Self {
        Self {
            cert_id: cert_id.into(),
            ..Self::default()
        }
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    pub fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    pub fn with_contact(mut self, contact: impl Into<String>) -> Self {
        self.contact = Some(contact.into());
        self
    }

    pub fn with_appearance(mut self, appearance: VisibleAppearance) -> Self {
        self.appearance = Some(appearance);
        self
    }

    pub fn with_level(mut self, level: PadesLevel) -> Self {
        self.level = level;
        self
    }

    pub fn with_cancel(mut self, token: CancelToken) -> Self {
        self.cancel = Some(token);
        self
    }

    /// [`Error::Cancelled`] if the request's token has been cancelled.
    pub fn check_cancelled(&self) -> Result<()> {
        match &self.cancel {
            Some(t) if t.is_cancelled() => Err(Error::Cancelled),
            _ => Ok(()),
        }
    }

    /// TSA URL for B-T (`http://` or `https://`).
    pub fn with_tsa_url(mut self, url: impl Into<String>) -> Self {
        self.tsa_url = Some(url.into());
        self
    }
}

/// Shared cancellation flag. Clone it, hand one copy to [`SignRequest::with_cancel`]
/// and call [`CancelToken::cancel`] from any thread.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// PAdES baseline signature level (ETSI EN 319 142-1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PadesLevel {
    /// B-B: signed attributes only, no timestamp.
    #[default]
    BB,
    /// B-T: B-B plus an RFC 3161 signature timestamp from a TSA. The TSA URL
    /// must be configured explicitly; there is no default server.
    BT,
}

impl PadesLevel {
    /// Display name, e.g. `"PAdES-B-T"`.
    pub fn label(self) -> &'static str {
        match self {
            PadesLevel::BB => "PAdES-B-B",
            PadesLevel::BT => "PAdES-B-T",
        }
    }
}

/// Information about an existing signature dictionary. Validity is left to the signer backend.
#[derive(Clone, Debug, Default)]
pub struct SignatureInfo {
    pub name: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub valid: Option<bool>,
    /// RFC 3161 signature timestamp (PAdES B-T), if the signature carries one.
    pub timestamp: Option<TimestampInfo>,
}

/// A signature timestamp found while verifying.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimestampInfo {
    /// `genTime` as RFC 3339 UTC, when the token could be parsed.
    pub time: Option<String>,
    /// Token signature, imprint, ESS binding and TSA chain all verified.
    pub valid: bool,
    /// Why it is not valid, when `valid` is false.
    pub problem: Option<crate::error::TimestampErrorKind>,
}

/// Certificate / PKCS#11 signing is implemented in `openpdf-sign`.
/// GTK and the signing crate talk only through this trait — never to each other.
///
/// The application binary constructs the backend and hands it to the UI:
///
/// ```ignore
/// let pin = /* UI type that implements PinProvider */;
/// let signer: Box<dyn openpdf_core::Signer> = Box::new(
///     openpdf_sign::Pkcs11Signer::new(openpdf_sign::Pkcs11Config::default(), pin)?,
/// );
/// // pass `signer` into UI modules that depend only on openpdf-core
/// ```
///
/// PIN entry is [`crate::PinProvider`] (implemented by the UI). PKCS#11 login
/// and raw key handles stay inside the `openpdf-sign` implementor.
pub trait Signer: Send + Sync {
    /// Certificates available on the token. A DNIe returns several; the UI picks by `id`.
    fn list_certificates(&self) -> Result<Vec<CertificateInfo>>;

    fn sign(&self, pdf: &[u8], request: &SignRequest) -> Result<Vec<u8>>;
    fn verify(&self, pdf: &[u8]) -> Result<Vec<SignatureInfo>>;
}

/// User-facing PIN prompt for a token (translated).
pub fn pin_prompt_text(token_label: &str) -> String {
    tr_args("Enter PIN for token \"{token}\"", &[("token", token_label)])
}

/// Translated label for a certificate usage.
pub fn usage_label(usage: &CertificateUsage) -> String {
    match usage {
        CertificateUsage::Signing => tr("Signing"),
        CertificateUsage::Authentication => tr("Authentication"),
        CertificateUsage::Encryption => tr("Encryption"),
        CertificateUsage::Other(detail) => tr_args("Other ({detail})", &[("detail", detail)]),
    }
}

/// One-line certificate description for a picker (translated).
pub fn describe_certificate(info: &CertificateInfo) -> String {
    let usages = if info.usage.is_empty() {
        tr("unspecified usage")
    } else {
        info.usage
            .iter()
            .map(usage_label)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let validity = match (&info.validity.not_before, &info.validity.not_after) {
        (Some(a), Some(b)) => tr_args(
            "valid {not_before} to {not_after}",
            &[("not_before", a), ("not_after", b)],
        ),
        _ => tr("validity unknown"),
    };
    tr_args(
        "{subject} — {usage} — {validity}",
        &[
            ("subject", &info.subject),
            ("usage", &usages),
            ("validity", &validity),
        ],
    )
}

/// User-facing verify line for one signature dictionary (translated).
pub fn format_verify_result(info: &SignatureInfo) -> String {
    let mut parts = Vec::new();
    if let Some(name) = &info.name {
        parts.push(tr_args("Signer: {name}", &[("name", name)]));
    }
    if let Some(reason) = &info.reason {
        parts.push(tr_args("Reason: {reason}", &[("reason", reason)]));
    }
    if let Some(location) = &info.location {
        parts.push(tr_args("Location: {location}", &[("location", location)]));
    }
    parts.push(match info.valid {
        Some(true) => tr("Valid"),
        Some(false) => tr("Invalid"),
        None => tr("Validity unknown"),
    });
    if let Some(ts) = &info.timestamp {
        let time = ts.time.clone().unwrap_or_else(|| tr("unknown time"));
        parts.push(if ts.valid {
            tr_args("Timestamp: {time} (valid)", &[("time", &time)])
        } else {
            tr_args("Timestamp: {time} (invalid)", &[("time", &time)])
        });
    }
    if parts.len() == 1 {
        parts.insert(0, tr("Signature"));
    }
    parts.join("; ")
}

/// Format every signature, or a “none found” line.
pub fn format_verify_results(infos: &[SignatureInfo]) -> String {
    if infos.is_empty() {
        return tr("No signatures found");
    }
    infos
        .iter()
        .map(format_verify_result)
        .collect::<Vec<_>>()
        .join("\n")
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
    dict.set("SubFilter", Object::Name(b"adbe.pkcs7.detached".to_vec()));
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
    use crate::i18n::Catalog;
    use crate::test_support::{corpus, with_locale};

    fn es_catalog() -> Catalog {
        Catalog::parse(include_str!("../../../po/es.po"))
    }

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
        SignRequest::new("dnie-sign").with_reason("test")
    }

    #[test]
    fn sign_request_defaults_to_pades_bb() {
        let r = SignRequest::new("c").with_reason("r").with_location("l");
        assert_eq!(r.level, PadesLevel::BB);
        assert_eq!(r.reason.as_deref(), Some("r"));
        assert_eq!(r.location.as_deref(), Some("l"));
        assert_eq!(
            SignRequest::new("c").with_level(PadesLevel::BT).level,
            PadesLevel::BT
        );
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

    #[test]
    fn gtk_manifest_does_not_depend_on_sign() {
        let manifest =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../openpdf-gtk/Cargo.toml");
        let src = std::fs::read_to_string(manifest).unwrap();
        assert!(
            !src.contains("openpdf-sign"),
            "openpdf-gtk must depend only on openpdf-core"
        );
    }

    #[test]
    fn pin_prompt_and_usage_follow_catalog() {
        with_locale("en", || {
            assert_eq!(
                pin_prompt_text("openpdf-test"),
                "Enter PIN for token \"openpdf-test\""
            );
            assert_eq!(usage_label(&CertificateUsage::Signing), "Signing");
        });
        with_locale("es_ES.UTF-8", || {
            let cat = es_catalog();
            assert_eq!(
                pin_prompt_text("DNIe"),
                cat.translate("Enter PIN for token \"{token}\"")
                    .replace("{token}", "DNIe")
            );
            assert_eq!(
                usage_label(&CertificateUsage::Signing),
                cat.translate("Signing")
            );
        });
    }

    #[test]
    fn describe_certificate_follows_catalog() {
        let info = CertificateInfo {
            id: "1".into(),
            subject: "CN=Test".into(),
            issuer: "CN=CA".into(),
            validity: CertificateValidity {
                not_before: Some("2020-01-01T00:00:00Z".into()),
                not_after: Some("2030-01-01T00:00:00Z".into()),
            },
            usage: vec![CertificateUsage::Signing],
        };
        with_locale("en", || {
            assert_eq!(
                describe_certificate(&info),
                "CN=Test — Signing — valid 2020-01-01T00:00:00Z to 2030-01-01T00:00:00Z"
            );
        });
        with_locale("es", || {
            let cat = es_catalog();
            let usage = cat.translate("Signing");
            let validity = cat
                .translate("valid {not_before} to {not_after}")
                .replace("{not_before}", "2020-01-01T00:00:00Z")
                .replace("{not_after}", "2030-01-01T00:00:00Z");
            let expected = cat
                .translate("{subject} — {usage} — {validity}")
                .replace("{subject}", "CN=Test")
                .replace("{usage}", usage)
                .replace("{validity}", &validity);
            assert_eq!(describe_certificate(&info), expected);
            assert_ne!(
                expected,
                "CN=Test — Signing — valid 2020-01-01T00:00:00Z to 2030-01-01T00:00:00Z"
            );
        });
    }

    #[test]
    fn verify_result_follows_catalog() {
        let info = SignatureInfo {
            name: Some("Ana".into()),
            reason: Some("agree".into()),
            location: Some("Madrid".into()),
            valid: Some(true),
            timestamp: None,
        };
        with_locale("en", || {
            assert_eq!(
                format_verify_result(&info),
                "Signer: Ana; Reason: agree; Location: Madrid; Valid"
            );
            assert_eq!(format_verify_results(&[]), "No signatures found");
        });
        with_locale("es", || {
            let cat = es_catalog();
            let expected = [
                cat.translate("Signer: {name}").replace("{name}", "Ana"),
                cat.translate("Reason: {reason}")
                    .replace("{reason}", "agree"),
                cat.translate("Location: {location}")
                    .replace("{location}", "Madrid"),
                cat.translate("Valid").to_string(),
            ]
            .join("; ");
            assert_eq!(format_verify_result(&info), expected);
            assert_eq!(
                format_verify_results(&[]),
                cat.translate("No signatures found")
            );
        });
    }
}
