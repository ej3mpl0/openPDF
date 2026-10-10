//! RFC 3161 timestamp client and token validation for PAdES B-T.
//!
//! The TSA URL always comes from the caller ([`openpdf_core::SignRequest::tsa_url`]
//! or [`crate::Pkcs11Config::tsa_url`]). There is no default server. Only the
//! SHA-256 hash of the CMS signature value is sent, never the document.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use cms::cert::CertificateChoices;
use cms::content_info::ContentInfo;
use cms::signed_data::{SignedData, SignerIdentifier};
use const_oid::db::rfc5912::{
    ID_SHA_256, ID_SHA_384, ID_SHA_512, RSA_ENCRYPTION, SHA_256_WITH_RSA_ENCRYPTION,
    SHA_384_WITH_RSA_ENCRYPTION, SHA_512_WITH_RSA_ENCRYPTION,
};
use const_oid::{AssociatedOid, ObjectIdentifier};
use der::asn1::{Int, OctetString, SetOfVec};
use der::{Any, Decode, Encode, Sequence};
use openpdf_core::{CancelToken, Error, Result, TimestampErrorKind};
use rsa::pkcs1v15::{Signature, VerifyingKey};
use rsa::pkcs8::DecodePublicKey;
use rsa::signature::Verifier;
use rsa::RsaPublicKey;
use sha2::{Digest, Sha256, Sha384, Sha512};
use x509_cert::ext::pkix::{
    BasicConstraints, ExtendedKeyUsage, KeyUsage, KeyUsages, SubjectKeyIdentifier,
};
use x509_cert::spki::AlgorithmIdentifierOwned;
use x509_cert::Certificate;
use x509_tsp::{MessageImprint, TimeStampReq, TimeStampResp, TspVersion, TstInfo};

/// `id-aa-signatureTimeStampToken` (RFC 3161 appendix A / CAdES).
pub(crate) const ID_AA_SIGNATURE_TIME_STAMP_TOKEN: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.14");
const ID_CT_TST_INFO: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.1.4");
const ID_SIGNED_DATA: ObjectIdentifier = const_oid::db::rfc5911::ID_SIGNED_DATA;
const ID_CONTENT_TYPE: ObjectIdentifier = const_oid::db::rfc5911::ID_CONTENT_TYPE;
const ID_MESSAGE_DIGEST: ObjectIdentifier = const_oid::db::rfc5911::ID_MESSAGE_DIGEST;
const ID_AA_SIGNING_CERTIFICATE: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.12");
const ID_AA_SIGNING_CERTIFICATE_V2: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.47");
const ID_KP_TIME_STAMPING: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.8");
const ID_SHA_1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.14.3.2.26");

/// Default network timeout for one TSA request.
pub const DEFAULT_TSA_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RESPONSE_BYTES: u64 = 256 * 1024;
const MAX_CHAIN: usize = 8;

/// A resolved TSA: URL, timeout and the roots its certificate must chain to.
pub(crate) struct Tsa {
    url: String,
    timeout: Duration,
    trust: Arc<Vec<Certificate>>,
}

impl Tsa {
    /// Validates the URL scheme. Called before any PIN prompt.
    pub(crate) fn new(
        url: Option<&str>,
        timeout: Duration,
        trust: Arc<Vec<Certificate>>,
    ) -> Result<Self> {
        let url = url.map(str::trim).filter(|u| !u.is_empty());
        let Some(url) = url else {
            return Err(Error::Timestamp(TimestampErrorKind::UrlMissing));
        };
        let lower = url.to_ascii_lowercase();
        if !(lower.starts_with("https://") || lower.starts_with("http://")) || url.len() < 9 {
            return Err(Error::Timestamp(TimestampErrorKind::BadUrl));
        }
        Ok(Self {
            url: url.to_string(),
            timeout,
            trust,
        })
    }
}

fn ts_err(kind: TimestampErrorKind) -> Error {
    Error::Timestamp(kind)
}

/// Parse DER trust anchors. Empty input means "use the system store".
pub(crate) fn trust_from_der(anchors: &[Vec<u8>]) -> Result<Arc<Vec<Certificate>>> {
    if anchors.is_empty() {
        return Ok(system_trust());
    }
    let mut out = Vec::with_capacity(anchors.len());
    for der in anchors {
        out.push(Certificate::from_der(der).map_err(|e| Error::Sign(e.to_string()))?);
    }
    Ok(Arc::new(out))
}

/// Roots from the operating system store (same set used for HTTPS).
pub(crate) fn system_trust() -> Arc<Vec<Certificate>> {
    static ROOTS: OnceLock<Arc<Vec<Certificate>>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let found = rustls_native_certs::load_native_certs();
            Arc::new(
                found
                    .certs
                    .iter()
                    .filter_map(|c| Certificate::from_der(c.as_ref()).ok())
                    .collect(),
            )
        })
        .clone()
}

fn tls_roots() -> Arc<Vec<ureq::tls::Certificate<'static>>> {
    static ROOTS: OnceLock<Arc<Vec<ureq::tls::Certificate<'static>>>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let found = rustls_native_certs::load_native_certs();
            Arc::new(
                found
                    .certs
                    .iter()
                    .map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned())
                    .collect(),
            )
        })
        .clone()
}

/// Request a timestamp over SHA-256(`signature_value`), validate it fully and
/// return the `TimeStampToken` (a CMS `ContentInfo`) DER.
pub(crate) fn timestamp(
    tsa: &Tsa,
    signature_value: &[u8],
    cancel: Option<&CancelToken>,
) -> Result<Vec<u8>> {
    let digest = Sha256::digest(signature_value);
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|e| {
        ts_err(TimestampErrorKind::Rng {
            detail: e.to_string(),
        })
    })?;
    // Positive, minimally encoded INTEGER.
    nonce[0] = (nonce[0] & 0x7f) | 0x01;
    let nonce_int = Int::new(&nonce).map_err(|_| ts_err(TimestampErrorKind::Malformed))?;
    let req = TimeStampReq {
        version: TspVersion::V1,
        message_imprint: MessageImprint {
            hash_algorithm: AlgorithmIdentifierOwned {
                oid: ID_SHA_256,
                parameters: Some(Any::null()),
            },
            hashed_message: OctetString::new(digest.to_vec())
                .map_err(|_| ts_err(TimestampErrorKind::Malformed))?,
        },
        req_policy: None,
        nonce: Some(nonce_int.clone()),
        cert_req: true,
        extensions: None,
    };
    let body = req
        .to_der()
        .map_err(|_| ts_err(TimestampErrorKind::Malformed))?;
    let reply = post_cancellable(tsa, body, cancel)?;
    let token = parse_response(&reply)?;
    validate_token(&token, signature_value, Some(&nonce_int), &tsa.trust, true).map_err(ts_err)?;
    Ok(token)
}

/// How often the waiting thread looks at the cancel token.
const CANCEL_POLL: Duration = Duration::from_millis(25);

/// Run the blocking HTTP exchange on a helper thread and poll `cancel` while
/// waiting, so a cancel takes effect within ~`CANCEL_POLL` in every phase
/// (DNS, connect, TLS, waiting for headers, reading the body). On cancel the
/// helper is detached; it owns only the request bytes and ends at its own
/// timeout. Nothing touches the PDF.
fn post_cancellable(tsa: &Tsa, body: Vec<u8>, cancel: Option<&CancelToken>) -> Result<Vec<u8>> {
    let Some(cancel) = cancel else {
        return post(&tsa.url, tsa.timeout, &body);
    };
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let url = tsa.url.clone();
    let timeout = tsa.timeout;
    std::thread::Builder::new()
        .name("openpdf-tsa".into())
        .spawn(move || {
            let _ = tx.send(post(&url, timeout, &body));
        })
        .map_err(|e| {
            ts_err(TimestampErrorKind::Network {
                detail: e.to_string(),
            })
        })?;
    loop {
        match rx.recv_timeout(CANCEL_POLL) {
            Ok(r) => {
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
                return r;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if cancel.is_cancelled() {
                    return Err(Error::Cancelled);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(ts_err(TimestampErrorKind::Network {
                    detail: "timestamp request thread ended".into(),
                }))
            }
        }
    }
}

/// `user:pass@` of an `http(s)://user:pass@host/` URL, if any.
fn url_userinfo(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let at = authority.rfind('@')?;
    Some(&authority[..=at])
}

/// Error text without the URL's credentials (they would end up in the UI and logs).
pub(crate) fn redact(detail: &str, url: &str) -> String {
    match url_userinfo(url) {
        Some(ui) => detail.replace(ui, ""),
        _ => detail.to_string(),
    }
}

fn post(url: &str, timeout: Duration, body: &[u8]) -> Result<Vec<u8>> {
    let net = |e: ureq::Error| match e {
        ureq::Error::BodyExceedsLimit(_) => ts_err(TimestampErrorKind::ResponseTooLarge),
        e => ts_err(TimestampErrorKind::Network {
            detail: redact(&e.to_string(), url),
        }),
    };
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::Specific(tls_roots()))
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .tls_config(tls)
        .build()
        .into();
    let mut resp = agent
        .post(url)
        .header("Content-Type", "application/timestamp-query")
        .header("Accept", "application/timestamp-reply")
        .send(body)
        .map_err(net)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ts_err(TimestampErrorKind::Http {
            status: status.as_u16(),
        }));
    }
    resp.body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_vec()
        .map_err(net)
}

fn parse_response(reply: &[u8]) -> Result<Vec<u8>> {
    let resp = TimeStampResp::from_der(reply).map_err(|_| ts_err(TimestampErrorKind::Malformed))?;
    let status = resp.status.status as u8;
    // granted (0) or grantedWithMods (1); anything else is a refusal.
    if status > 1 {
        return Err(ts_err(TimestampErrorKind::Rejected { status }));
    }
    let token = resp
        .time_stamp_token
        .ok_or_else(|| ts_err(TimestampErrorKind::Malformed))?;
    token
        .to_der()
        .map_err(|_| ts_err(TimestampErrorKind::Malformed))
}

/// `genTime` of a token in Unix seconds, without validating it.
pub(crate) fn token_gen_time(token_der: &[u8]) -> Option<u64> {
    let (_, tst, _) = decode_token(token_der).ok()?;
    Some(tst.gen_time.to_unix_duration().as_secs())
}

fn decode_token(
    token_der: &[u8],
) -> std::result::Result<(SignedData, TstInfo, Vec<u8>), TimestampErrorKind> {
    let malformed = |_| TimestampErrorKind::Malformed;
    let ci = ContentInfo::from_der(token_der).map_err(malformed)?;
    if ci.content_type != ID_SIGNED_DATA {
        return Err(TimestampErrorKind::Malformed);
    }
    let sd = ci.content.decode_as::<SignedData>().map_err(malformed)?;
    if sd.encap_content_info.econtent_type != ID_CT_TST_INFO {
        return Err(TimestampErrorKind::Malformed);
    }
    let econtent = sd
        .encap_content_info
        .econtent
        .as_ref()
        .ok_or(TimestampErrorKind::Malformed)?;
    let octets = econtent.decode_as::<OctetString>().map_err(malformed)?;
    let tst_der = octets.as_bytes().to_vec();
    let tst = TstInfo::from_der(&tst_der).map_err(malformed)?;
    Ok((sd, tst, tst_der))
}

/// Full RFC 3161 token check against our signature value.
///
/// `strict_request` (signing time): the imprint must use SHA-256, which is what
/// we asked for. At verify time any supported hash is accepted.
/// Returns `genTime` in Unix seconds.
pub(crate) fn validate_token(
    token_der: &[u8],
    signature_value: &[u8],
    nonce: Option<&Int>,
    trust: &[Certificate],
    strict_request: bool,
) -> std::result::Result<u64, TimestampErrorKind> {
    let (sd, tst, tst_der) = decode_token(token_der)?;

    // messageImprint: our hash, with our algorithm.
    let imprint_alg = tst.message_imprint.hash_algorithm.oid;
    if strict_request && imprint_alg != ID_SHA_256 {
        return Err(TimestampErrorKind::ImprintMismatch);
    }
    let expected =
        hash_with(imprint_alg, signature_value).ok_or(TimestampErrorKind::ImprintMismatch)?;
    if tst.message_imprint.hashed_message.as_bytes() != expected.as_slice() {
        return Err(TimestampErrorKind::ImprintMismatch);
    }

    if let Some(n) = nonce {
        match &tst.nonce {
            Some(got) if got.as_bytes() == n.as_bytes() => {}
            _ => return Err(TimestampErrorKind::NonceMismatch),
        }
    }

    let gen_time = tst.gen_time.to_unix_duration().as_secs();

    // Token CMS signature.
    let si = sd
        .signer_infos
        .0
        .as_slice()
        .first()
        .ok_or(TimestampErrorKind::Malformed)?;
    let pool: Vec<Certificate> = sd
        .certificates
        .as_ref()
        .map(|set| {
            set.0
                .iter()
                .filter_map(|c| match c {
                    CertificateChoices::Certificate(c) => Some(c.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let tsa_cert = pool
        .iter()
        .find(|c| matches_sid(c, &si.sid))
        .ok_or(TimestampErrorKind::UntrustedCert)?;

    let attrs = si
        .signed_attrs
        .as_ref()
        .ok_or(TimestampErrorKind::BadSignature)?;
    let attr_value = |oid: ObjectIdentifier| {
        attrs
            .iter()
            .find(|a| a.oid == oid)
            .and_then(|a| a.values.as_slice().first())
    };
    let ct = attr_value(ID_CONTENT_TYPE)
        .and_then(|v| v.decode_as::<ObjectIdentifier>().ok())
        .ok_or(TimestampErrorKind::BadSignature)?;
    if ct != ID_CT_TST_INFO {
        return Err(TimestampErrorKind::BadSignature);
    }
    let md = attr_value(ID_MESSAGE_DIGEST)
        .and_then(|v| v.decode_as::<OctetString>().ok())
        .ok_or(TimestampErrorKind::BadSignature)?;
    let content_hash =
        hash_with(si.digest_alg.oid, &tst_der).ok_or(TimestampErrorKind::BadSignature)?;
    if md.as_bytes() != content_hash.as_slice() {
        return Err(TimestampErrorKind::BadSignature);
    }
    let tbs = SetOfVec::from_iter(attrs.iter().cloned())
        .and_then(|s| s.to_der())
        .map_err(|_| TimestampErrorKind::BadSignature)?;
    if !rsa_verify(
        tsa_cert,
        si.signature_algorithm.oid,
        Some(si.digest_alg.oid),
        &tbs,
        si.signature.as_bytes(),
    ) {
        return Err(TimestampErrorKind::BadSignature);
    }

    // ESS signing-certificate(-v2) must name this TSA certificate.
    let tsa_der = tsa_cert
        .to_der()
        .map_err(|_| TimestampErrorKind::Malformed)?;
    let ess_ok = if let Some(v) = attr_value(ID_AA_SIGNING_CERTIFICATE_V2) {
        let scv2 = v
            .decode_as::<SigningCertificateV2>()
            .map_err(|_| TimestampErrorKind::SigningCertMismatch)?;
        scv2.certs.first().is_some_and(|id| {
            let alg = id.hash_algorithm.as_ref().map_or(ID_SHA_256, |a| a.oid);
            hash_with(alg, &tsa_der).is_some_and(|h| h == id.cert_hash.as_bytes())
        })
    } else if let Some(v) = attr_value(ID_AA_SIGNING_CERTIFICATE) {
        let scv1 = v
            .decode_as::<SigningCertificate>()
            .map_err(|_| TimestampErrorKind::SigningCertMismatch)?;
        scv1.certs.first().is_some_and(|id| {
            hash_with(ID_SHA_1, &tsa_der).is_some_and(|h| h == id.cert_hash.as_bytes())
        })
    } else {
        false
    };
    if !ess_ok {
        return Err(TimestampErrorKind::SigningCertMismatch);
    }

    // TSA certificate: timeStamping purpose and a chain to a trusted root.
    if !has_time_stamping_eku(tsa_cert) || !chain_trusted(tsa_cert, &pool, trust, gen_time) {
        return Err(TimestampErrorKind::UntrustedCert);
    }
    Ok(gen_time)
}

/// RFC 5035 `ESSCertIDv2`.
#[derive(Clone, Debug, Eq, PartialEq, Sequence)]
struct EssCertIdV2 {
    #[asn1(optional = "true")]
    hash_algorithm: Option<AlgorithmIdentifierOwned>,
    cert_hash: OctetString,
    #[asn1(optional = "true")]
    issuer_serial: Option<Any>,
}

#[derive(Clone, Debug, Eq, PartialEq, Sequence)]
struct SigningCertificateV2 {
    certs: Vec<EssCertIdV2>,
    #[asn1(optional = "true")]
    policies: Option<Any>,
}

/// RFC 2634 `ESSCertID` (SHA-1).
#[derive(Clone, Debug, Eq, PartialEq, Sequence)]
struct EssCertId {
    cert_hash: OctetString,
    #[asn1(optional = "true")]
    issuer_serial: Option<Any>,
}

#[derive(Clone, Debug, Eq, PartialEq, Sequence)]
struct SigningCertificate {
    certs: Vec<EssCertId>,
    #[asn1(optional = "true")]
    policies: Option<Any>,
}

fn hash_with(alg: ObjectIdentifier, data: &[u8]) -> Option<Vec<u8>> {
    Some(match alg {
        a if a == ID_SHA_256 => Sha256::digest(data).to_vec(),
        a if a == ID_SHA_384 => Sha384::digest(data).to_vec(),
        a if a == ID_SHA_512 => Sha512::digest(data).to_vec(),
        a if a == ID_SHA_1 => sha1::Sha1::digest(data).to_vec(),
        _ => return None,
    })
}

fn matches_sid(cert: &Certificate, sid: &SignerIdentifier) -> bool {
    match sid {
        SignerIdentifier::IssuerAndSerialNumber(ias) => {
            cert.tbs_certificate.issuer == ias.issuer
                && cert.tbs_certificate.serial_number == ias.serial_number
        }
        SignerIdentifier::SubjectKeyIdentifier(ski) => {
            extension::<SubjectKeyIdentifier>(cert).is_some_and(|own| own.0 == ski.0)
        }
    }
}

fn extension<T: AssociatedOid + for<'a> Decode<'a>>(cert: &Certificate) -> Option<T> {
    cert.tbs_certificate
        .extensions
        .as_ref()?
        .iter()
        .find(|e| e.extn_id == T::OID)
        .and_then(|e| T::from_der(e.extn_value.as_bytes()).ok())
}

/// RFC 3161 §2.3: the TSA certificate MUST have exactly one extended key
/// usage, timeStamping, and the extension MUST be critical.
fn has_time_stamping_eku(cert: &Certificate) -> bool {
    let Some(ext) = cert
        .tbs_certificate
        .extensions
        .as_ref()
        .and_then(|exts| exts.iter().find(|e| e.extn_id == ExtendedKeyUsage::OID))
    else {
        return false;
    };
    if !ext.critical {
        return false;
    }
    ExtendedKeyUsage::from_der(ext.extn_value.as_bytes())
        .is_ok_and(|eku| eku.0.as_slice() == [ID_KP_TIME_STAMPING])
}

/// A CA in the chain may sign certificates: keyUsage present with keyCertSign
/// (RFC 5280 §4.2.1.3), and `below` intermediate CAs under it fit its
/// pathLenConstraint (§4.2.1.9).
fn ca_may_issue(cert: &Certificate, below: usize) -> bool {
    let Some(bc) = extension::<BasicConstraints>(cert) else {
        return false;
    };
    if !bc.ca {
        return false;
    }
    if bc
        .path_len_constraint
        .is_some_and(|max| below > max as usize)
    {
        return false;
    }
    extension::<KeyUsage>(cert).is_some_and(|ku| ku.0.contains(KeyUsages::KeyCertSign))
}

fn valid_at(cert: &Certificate, at: u64) -> bool {
    let v = &cert.tbs_certificate.validity;
    v.not_before.to_unix_duration().as_secs() <= at
        && at <= v.not_after.to_unix_duration().as_secs()
}

fn signed_by(child: &Certificate, issuer: &Certificate) -> bool {
    if child.tbs_certificate.issuer != issuer.tbs_certificate.subject {
        return false;
    }
    let Ok(tbs) = child.tbs_certificate.to_der() else {
        return false;
    };
    let Some(sig) = child.signature.as_bytes() else {
        return false;
    };
    rsa_verify(issuer, child.signature_algorithm.oid, None, &tbs, sig)
}

/// Walk issuer links through the token's certificates up to a trusted root.
fn chain_trusted(leaf: &Certificate, pool: &[Certificate], trust: &[Certificate], at: u64) -> bool {
    let mut current = leaf.clone();
    for depth in 0..MAX_CHAIN {
        if !valid_at(&current, at) {
            return false;
        }
        // `current` at depth d has d-1 intermediate CAs below it.
        if depth > 0 && !ca_may_issue(&current, depth - 1) {
            return false;
        }
        if trust.iter().any(|t| t == &current) {
            return true;
        }
        if trust
            .iter()
            .any(|t| valid_at(t, at) && ca_may_issue(t, depth) && signed_by(&current, t))
        {
            return true;
        }
        let next = pool
            .iter()
            .find(|c| *c != &current && signed_by(&current, c))
            .cloned();
        match next {
            Some(n) => current = n,
            None => return false,
        }
    }
    false
}

/// RSA PKCS#1 v1.5 with SHA-256/384/512. Other key types are rejected.
fn rsa_verify(
    signer: &Certificate,
    sig_alg: ObjectIdentifier,
    digest_hint: Option<ObjectIdentifier>,
    msg: &[u8],
    sig: &[u8],
) -> bool {
    let Ok(spki) = signer.tbs_certificate.subject_public_key_info.to_der() else {
        return false;
    };
    let Ok(key) = RsaPublicKey::from_public_key_der(&spki) else {
        return false;
    };
    let Ok(sig) = Signature::try_from(sig) else {
        return false;
    };
    let digest = match sig_alg {
        a if a == SHA_256_WITH_RSA_ENCRYPTION => ID_SHA_256,
        a if a == SHA_384_WITH_RSA_ENCRYPTION => ID_SHA_384,
        a if a == SHA_512_WITH_RSA_ENCRYPTION => ID_SHA_512,
        a if a == RSA_ENCRYPTION => match digest_hint {
            Some(d) => d,
            None => return false,
        },
        _ => return false,
    };
    if let Some(hint) = digest_hint {
        if hint != digest {
            return false;
        }
    }
    match digest {
        d if d == ID_SHA_256 => VerifyingKey::<Sha256>::new(key).verify(msg, &sig).is_ok(),
        d if d == ID_SHA_384 => VerifyingKey::<Sha384>::new(key).verify(msg, &sig).is_ok(),
        d if d == ID_SHA_512 => VerifyingKey::<Sha512>::new(key).verify(msg, &sig).is_ok(),
        _ => false,
    }
}
