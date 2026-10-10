//! PKCS#11 module loading, certificate listing (no PIN), and login-on-sign.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use const_oid::AssociatedOid;
use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::error::{Error as CkError, RvError};
use cryptoki::mechanism::Mechanism;
use cryptoki::object::{Attribute, AttributeType, ObjectClass, ObjectHandle};
use cryptoki::session::{Session, UserType};
use cryptoki::slot::Slot;
use cryptoki::types::AuthPin;
use der::Decode;
use openpdf_core::{
    tr_args, CertificateInfo, CertificateUsage, CertificateValidity, Error, InvalidKind,
    PadesLevel, PinProvider, Result, SecretPin, SignRequest, SignatureInfo, Signer,
};
use sha2::{Digest, Sha256};
use x509_cert::ext::pkix::KeyUsage;
use x509_cert::Certificate;

use crate::cades;
use crate::pades;
use crate::tsp::{self, Tsa};

/// OpenSC (Spanish DNIe), p11-kit / NSS softoken (FNMT in a user NSS db), SoftHSM.
const DEFAULT_MODULES: &[&str] = &[
    "/usr/lib/x86_64-linux-gnu/opensc-pkcs11.so",
    "/usr/lib/x86_64-linux-gnu/pkcs11/opensc-pkcs11.so",
    "/usr/lib/opensc-pkcs11.so",
    "/usr/lib64/opensc-pkcs11.so",
    "/usr/lib/opensc/opensc-pkcs11.so",
    "/usr/lib/x86_64-linux-gnu/pkcs11/p11-kit-proxy.so",
    "/usr/lib/x86_64-linux-gnu/p11-kit-proxy.so",
    "/usr/lib/p11-kit-proxy.so",
    "/usr/lib64/p11-kit-proxy.so",
    "/usr/lib/x86_64-linux-gnu/nss/libsoftokn3.so",
    "/usr/lib/nss/libsoftokn3.so",
    "/usr/lib/x86_64-linux-gnu/pkcs11/libsoftokn3.so",
    "/usr/lib/softhsm/libsofthsm2.so",
    "/usr/lib/x86_64-linux-gnu/softhsm/libsofthsm2.so",
    "/usr/lib/x86_64-linux-gnu/pkcs11/libsofthsm2.so",
];

/// Where to load PKCS#11 modules from.
#[derive(Clone, Debug, Default)]
pub struct Pkcs11Config {
    /// Explicit module. When unset, [`OPENPDF_PKCS11_MODULE`] and the built-in defaults are tried.
    pub module_path: Option<PathBuf>,
    /// RFC 3161 TSA for PAdES B-T, used when the request has no
    /// [`SignRequest::tsa_url`]. There is deliberately no default server.
    pub tsa_url: Option<String>,
    /// Network timeout for one TSA request ([`tsp::DEFAULT_TSA_TIMEOUT`] when unset).
    pub tsa_timeout: Option<Duration>,
    /// DER roots the TSA certificate must chain to, for signing and verifying.
    /// Empty: the operating system certificate store.
    pub tsa_trust_anchors: Vec<Vec<u8>>,
}

impl Pkcs11Config {
    pub fn with_module(path: impl Into<PathBuf>) -> Self {
        Self {
            module_path: Some(path.into()),
            ..Self::default()
        }
    }

    pub fn with_tsa_url(mut self, url: impl Into<String>) -> Self {
        self.tsa_url = Some(url.into());
        self
    }

    pub fn with_tsa_timeout(mut self, timeout: Duration) -> Self {
        self.tsa_timeout = Some(timeout);
        self
    }

    /// Trust this DER root for TSA certificates (replaces the system store).
    pub fn with_tsa_trust_anchor(mut self, der: impl Into<Vec<u8>>) -> Self {
        self.tsa_trust_anchors.push(der.into());
        self
    }
}

/// PKCS#11 backend that implements [`Signer`].
///
/// PIN entry happens only through [`PinProvider`] inside [`Signer::sign`].
pub struct Pkcs11Signer {
    modules: Vec<LoadedModule>,
    pin: Box<dyn PinProvider>,
    tsa_url: Option<String>,
    tsa_timeout: Duration,
    tsa_trust: Arc<Vec<x509_cert::Certificate>>,
}

struct LoadedModule {
    stem: String,
    ctx: Pkcs11,
}

struct PickedCert {
    module_index: usize,
    slot: Slot,
    token_label: String,
    cka_id: Vec<u8>,
    der: Vec<u8>,
    info: CertificateInfo,
}

impl Pkcs11Signer {
    pub fn new(config: Pkcs11Config, pin: impl PinProvider + 'static) -> Result<Self> {
        let tsa_trust = tsp::trust_from_der(&config.tsa_trust_anchors)?;
        let paths = resolve_module_paths(&config);
        let mut modules = Vec::new();
        let mut last_err = None;
        for path in paths {
            if !path.exists() {
                continue;
            }
            match load_module(&path) {
                Ok(m) => modules.push(m),
                Err(e) => last_err = Some(e),
            }
        }
        if modules.is_empty() {
            return Err(last_err.unwrap_or_else(|| {
                Error::Sign(
                    "no PKCS#11 module found (set OPENPDF_PKCS11_MODULE or install OpenSC / p11-kit)"
                        .into(),
                )
            }));
        }
        Ok(Self {
            modules,
            pin: Box::new(pin),
            tsa_url: config.tsa_url,
            tsa_timeout: config.tsa_timeout.unwrap_or(tsp::DEFAULT_TSA_TIMEOUT),
            tsa_trust,
        })
    }

    fn pick(&self, cert_id: &str) -> Result<PickedCert> {
        let wanted = cert_id.trim();
        if wanted.is_empty() {
            return Err(Error::invalid(InvalidKind::CertIdRequired));
        }
        for info in self.enumerate_certs()? {
            if info.info.id == wanted {
                return Ok(info);
            }
        }
        Err(Error::Sign(tr_args(
            "unknown certificate id {id}",
            &[("id", wanted)],
        )))
    }

    fn enumerate_certs(&self) -> Result<Vec<PickedCert>> {
        let prefix_module = self.modules.len() > 1;
        let mut out = Vec::new();
        for (module_index, module) in self.modules.iter().enumerate() {
            let slots = match module.ctx.get_slots_with_token() {
                Ok(s) => s,
                Err(_) => continue,
            };
            for slot in slots {
                let token_label = module
                    .ctx
                    .get_token_info(slot)
                    .ok()
                    .map(|t| trim_pkcs11_label(t.label()))
                    .unwrap_or_default();
                let session = match module.ctx.open_ro_session(slot) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let handles =
                    match session.find_objects(&[Attribute::Class(ObjectClass::CERTIFICATE)]) {
                        Ok(h) => h,
                        Err(_) => continue,
                    };
                for handle in handles {
                    if let Some(picked) = read_cert(
                        &session,
                        handle,
                        module_index,
                        &module.stem,
                        slot,
                        &token_label,
                        prefix_module,
                    ) {
                        out.push(picked);
                    }
                }
            }
        }
        Ok(out)
    }

    fn sign_cms(
        &self,
        message: &[u8],
        picked: &PickedCert,
        request: &SignRequest,
    ) -> Result<Vec<u8>> {
        let module = self
            .modules
            .get(picked.module_index)
            .ok_or_else(|| pkcs11_err("missing module"))?;
        let session = module
            .ctx
            .open_rw_session(picked.slot)
            .or_else(|_| module.ctx.open_ro_session(picked.slot))
            .map_err(pkcs11_err)?;

        // Last check before the PIN dialog.
        request.check_cancelled()?;
        let pin: SecretPin = self.pin.request_pin(&picked.token_label)?;
        {
            let auth = AuthPin::new(pin.as_str().into());
            session
                .login(UserType::User, Some(&auth))
                .map_err(pkcs11_err)?;
        }
        drop(pin);

        let key = find_private_key(&session, &picked.cka_id)?;
        let cms = cades::sign_detached(message, &picked.der, |data| {
            pkcs11_rsa_sha256(&session, key, data)
        });
        let _ = session.logout();
        cms
    }
}

impl Signer for Pkcs11Signer {
    fn list_certificates(&self) -> Result<Vec<CertificateInfo>> {
        Ok(self
            .enumerate_certs()?
            .into_iter()
            .map(|c| c.info)
            .collect())
    }

    fn sign(&self, pdf: &[u8], request: &SignRequest) -> Result<Vec<u8>> {
        if request.cert_id.trim().is_empty() {
            return Err(Error::invalid(InvalidKind::CertIdRequired));
        }
        // Refuse unsupported levels / a missing TSA before any token login or PIN prompt.
        let tsa = self.resolve_tsa(request)?;
        pades::ensure_supported_level(request, tsa.as_ref())?;
        request.check_cancelled()?;
        let picked = self.pick(&request.cert_id)?;
        if !can_sign(&picked.info.usage) {
            return Err(Error::Sign("certificate is not a signing cert".into()));
        }
        let cert = Certificate::from_der(&picked.der).map_err(|e| Error::Sign(e.to_string()))?;
        let name = common_name_or_subject(&cert);
        pades::sign_incremental(pdf, request, &name, tsa.as_ref(), |message| {
            self.sign_cms(message, &picked, request)
        })
    }

    fn verify(&self, pdf: &[u8]) -> Result<Vec<SignatureInfo>> {
        pades::verify_signatures(pdf, &self.tsa_trust)
    }
}

impl Pkcs11Signer {
    /// B-T only: request URL first, then config. No URL is a typed error.
    fn resolve_tsa(&self, request: &SignRequest) -> Result<Option<Tsa>> {
        if request.level != PadesLevel::BT {
            return Ok(None);
        }
        let url = request.tsa_url.as_deref().or(self.tsa_url.as_deref());
        Tsa::new(url, self.tsa_timeout, self.tsa_trust.clone()).map(Some)
    }
}

/// Verify PAdES signatures without opening a PKCS#11 module.
///
/// Signature timestamps are checked against the operating system store.
pub fn verify_pdf(pdf: &[u8]) -> Result<Vec<SignatureInfo>> {
    pades::verify_signatures(pdf, &tsp::system_trust())
}

/// Like [`verify_pdf`], but TSA certificates must chain to `tsa_trust_anchors` (DER).
pub fn verify_pdf_with_tsa_trust(
    pdf: &[u8],
    tsa_trust_anchors: &[Vec<u8>],
) -> Result<Vec<SignatureInfo>> {
    let trust = tsp::trust_from_der(tsa_trust_anchors)?;
    pades::verify_signatures(pdf, &trust)
}

fn resolve_module_paths(config: &Pkcs11Config) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(p) = &config.module_path {
        paths.push(p.clone());
        return paths;
    }
    if let Ok(env) = std::env::var("OPENPDF_PKCS11_MODULE") {
        let p = PathBuf::from(env);
        if !p.as_os_str().is_empty() {
            paths.push(p);
        }
    }
    for d in DEFAULT_MODULES {
        let p = PathBuf::from(d);
        if !paths.contains(&p) {
            paths.push(p);
        }
    }
    paths
}

fn load_module(path: &Path) -> Result<LoadedModule> {
    let ctx = Pkcs11::new(path).map_err(|e| {
        Error::Sign(tr_args(
            "Could not load PKCS#11 module {path}: {error}",
            &[
                ("path", &path.display().to_string()),
                ("error", &e.to_string()),
            ],
        ))
    })?;
    match ctx.initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK)) {
        Ok(()) => {}
        Err(CkError::Pkcs11(RvError::CryptokiAlreadyInitialized, _)) => {}
        Err(e) => {
            return Err(Error::Sign(tr_args(
                "Could not load PKCS#11 module {path}: {error}",
                &[
                    ("path", &path.display().to_string()),
                    ("error", &e.to_string()),
                ],
            )))
        }
    }
    let stem = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("pkcs11")
        .to_string();
    Ok(LoadedModule { stem, ctx })
}

fn read_cert(
    session: &Session,
    handle: ObjectHandle,
    module_index: usize,
    stem: &str,
    slot: Slot,
    token_label: &str,
    prefix_module: bool,
) -> Option<PickedCert> {
    let der = attr_bytes(session, handle, AttributeType::Value)?;
    if der.is_empty() {
        return None;
    }
    let cka_id = attr_bytes(session, handle, AttributeType::Id).unwrap_or_default();
    let cert = Certificate::from_der(&der).ok()?;
    let info = cert_info(&cert, &der, &cka_id, stem, slot, prefix_module);
    Some(PickedCert {
        module_index,
        slot,
        token_label: token_label.to_string(),
        cka_id,
        der,
        info,
    })
}

fn cert_info(
    cert: &Certificate,
    der: &[u8],
    cka_id: &[u8],
    stem: &str,
    slot: Slot,
    prefix_module: bool,
) -> CertificateInfo {
    let key = if cka_id.is_empty() {
        let hash = Sha256::digest(der);
        format!("sha256:{}", pades::hex_encode_lower(&hash[..8]))
    } else {
        pades::hex_encode_lower(cka_id)
    };
    let id = if prefix_module {
        format!("{stem}:{}:{key}", slot_id(slot))
    } else {
        format!("{}:{key}", slot_id(slot))
    };
    CertificateInfo {
        id,
        subject: cert.tbs_certificate.subject.to_string(),
        issuer: cert.tbs_certificate.issuer.to_string(),
        validity: CertificateValidity {
            not_before: time_rfc3339(&cert.tbs_certificate.validity.not_before),
            not_after: time_rfc3339(&cert.tbs_certificate.validity.not_after),
        },
        usage: key_usage(cert),
    }
}

fn key_usage(cert: &Certificate) -> Vec<CertificateUsage> {
    let Some(exts) = &cert.tbs_certificate.extensions else {
        return Vec::new();
    };
    for ext in exts.iter() {
        if ext.extn_id != KeyUsage::OID {
            continue;
        }
        let Ok(ku) = KeyUsage::from_der(ext.extn_value.as_bytes()) else {
            continue;
        };
        let mut usage = Vec::new();
        if ku.non_repudiation() {
            usage.push(CertificateUsage::Signing);
        }
        if ku.digital_signature() && !ku.non_repudiation() {
            usage.push(CertificateUsage::Authentication);
        } else if ku.digital_signature()
            && ku.non_repudiation()
            && !usage.contains(&CertificateUsage::Signing)
        {
            usage.push(CertificateUsage::Signing);
        }
        if ku.key_encipherment() || ku.data_encipherment() {
            usage.push(CertificateUsage::Encryption);
        }
        if usage.is_empty() {
            usage.push(CertificateUsage::Other("keyUsage".into()));
        }
        return usage;
    }
    Vec::new()
}

fn can_sign(usage: &[CertificateUsage]) -> bool {
    usage.is_empty() || usage.contains(&CertificateUsage::Signing)
}

fn time_rfc3339(t: &x509_cert::time::Time) -> Option<String> {
    let secs = t.to_unix_duration().as_secs();
    let (y, m, d, h, min, s) = pades::civil_utc(secs);
    Some(format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}Z"))
}

fn common_name_or_subject(cert: &Certificate) -> String {
    let subject = &cert.tbs_certificate.subject;
    for rdn in subject.0.iter() {
        for ava in rdn.0.iter() {
            if ava.oid == const_oid::db::rfc4519::COMMON_NAME {
                if let Ok(s) = ava.value.decode_as::<der::asn1::Utf8StringRef<'_>>() {
                    return s.to_string();
                }
                if let Ok(s) = ava.value.decode_as::<der::asn1::PrintableStringRef<'_>>() {
                    return s.to_string();
                }
                if let Ok(s) = ava.value.decode_as::<der::asn1::Ia5StringRef<'_>>() {
                    return s.to_string();
                }
            }
        }
    }
    subject.to_string()
}

fn attr_bytes(session: &Session, handle: ObjectHandle, ty: AttributeType) -> Option<Vec<u8>> {
    let attrs = session.get_attributes(handle, &[ty]).ok()?;
    for a in attrs {
        match a {
            Attribute::Value(v) | Attribute::Id(v) | Attribute::Label(v) => return Some(v),
            _ => {}
        }
    }
    None
}

fn find_private_key(session: &Session, cka_id: &[u8]) -> Result<ObjectHandle> {
    let mut template = vec![Attribute::Class(ObjectClass::PRIVATE_KEY)];
    if !cka_id.is_empty() {
        template.push(Attribute::Id(cka_id.to_vec()));
    }
    let found = session.find_objects(&template).map_err(pkcs11_err)?;
    found
        .into_iter()
        .next()
        .ok_or_else(|| Error::Sign("no private key for certificate".into()))
}

fn pkcs11_rsa_sha256(session: &Session, key: ObjectHandle, data: &[u8]) -> Result<Vec<u8>> {
    match session.sign(&Mechanism::Sha256RsaPkcs, key, data) {
        Ok(sig) => Ok(sig),
        Err(_) => {
            let mut info = Vec::with_capacity(19 + 32);
            info.extend_from_slice(&[
                0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
                0x01, 0x05, 0x00, 0x04, 0x20,
            ]);
            info.extend_from_slice(Sha256::digest(data).as_slice());
            session
                .sign(&Mechanism::RsaPkcs, key, &info)
                .map_err(pkcs11_err)
        }
    }
}

fn pkcs11_err(e: impl ToString) -> Error {
    Error::Sign(tr_args(
        "PKCS#11 error: {error}",
        &[("error", &e.to_string())],
    ))
}

fn slot_id(slot: Slot) -> u64 {
    slot.id()
}

fn trim_pkcs11_label(label: &str) -> String {
    label.trim().trim_end_matches('\0').trim().to_string()
}
