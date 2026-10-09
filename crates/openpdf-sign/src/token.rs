//! PKCS#11 module loading, certificate listing (no PIN), and login-on-sign.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::error::{Error as CkError, RvError};
use cryptoki::mechanism::Mechanism;
use cryptoki::object::{Attribute, AttributeType, ObjectClass, ObjectHandle};
use cryptoki::session::{Session, UserType};
use cryptoki::slot::{Slot, TokenInfo};
use cryptoki::types::AuthPin;
use der::Decode;
use openpdf_core::{
    tr_args, CertificateInfo, Error, PinProvider, Result, SignRequest, SignatureInfo, Signer,
    TokenPinStatus,
};
use sha2::{Digest, Sha256};
use x509_cert::Certificate;

use crate::backend::{list_certificates, sign_pdf, KeyBackend, ListedCert};
use crate::cades;
use crate::cert::{self, can_use_for_pades, key_kind, EcdsaHash, KeyKind};
use crate::pades;

/// OpenSC (Spanish DNIe), p11-kit / NSS softoken (FNMT in a user NSS db), SoftHSM.
/// OpenSC and p11-kit also ship on Windows and macOS.
#[cfg(not(any(windows, target_os = "macos")))]
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

#[cfg(windows)]
const DEFAULT_MODULES: &[&str] = &[
    r"C:\Program Files\OpenSC Project\OpenSC\pkcs11\opensc-pkcs11.dll",
    r"C:\Program Files (x86)\OpenSC Project\OpenSC\pkcs11\opensc-pkcs11.dll",
    r"C:\Windows\System32\opensc-pkcs11.dll",
    r"C:\Windows\SysWOW64\opensc-pkcs11.dll",
];

#[cfg(target_os = "macos")]
const DEFAULT_MODULES: &[&str] = &[
    "/opt/homebrew/lib/opensc-pkcs11.so",
    "/opt/homebrew/lib/pkcs11/opensc-pkcs11.so",
    "/usr/local/lib/opensc-pkcs11.so",
    "/usr/local/lib/pkcs11/opensc-pkcs11.so",
    "/Library/OpenSC/lib/opensc-pkcs11.so",
    "/opt/homebrew/lib/pkcs11/p11-kit-proxy.dylib",
    "/usr/local/lib/pkcs11/p11-kit-proxy.dylib",
];

/// Where to load PKCS#11 modules from.
#[derive(Clone, Debug, Default)]
pub struct Pkcs11Config {
    /// Explicit module. When unset, [`OPENPDF_PKCS11_MODULE`] and the built-in defaults are tried.
    pub module_path: Option<PathBuf>,
}

impl Pkcs11Config {
    pub fn with_module(path: impl Into<PathBuf>) -> Self {
        Self {
            module_path: Some(path.into()),
        }
    }
}

/// PKCS#11 backend that implements [`Signer`].
///
/// PIN entry happens only through [`PinProvider`] inside [`Signer::sign`].
pub struct Pkcs11Signer {
    backend: Pkcs11Backend,
}

pub(crate) struct Pkcs11Backend {
    modules: Vec<LoadedModule>,
    pin: Arc<dyn PinProvider>,
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
        Ok(Self {
            backend: Pkcs11Backend::new(config, Arc::new(pin))?,
        })
    }
}

impl Pkcs11Backend {
    pub(crate) fn new(config: Pkcs11Config, pin: Arc<dyn PinProvider>) -> Result<Self> {
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
        Ok(Self { modules, pin })
    }

    fn pick(&self, cert_id: &str) -> Result<PickedCert> {
        let wanted = cert_id.trim();
        if wanted.is_empty() {
            return Err(Error::invalid(openpdf_core::InvalidKind::CertIdRequired));
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
                let token_info = module.ctx.get_token_info(slot).ok();
                let token_label = token_info
                    .as_ref()
                    .map(|t| trim_pkcs11_label(t.label()))
                    .unwrap_or_default();
                let pin_status = token_info.as_ref().map(token_pin_status);
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
                    if let Some(mut picked) = read_cert(
                        &session,
                        handle,
                        module_index,
                        &module.stem,
                        slot,
                        &token_label,
                        prefix_module,
                    ) {
                        picked.info.pin_status = pin_status;
                        out.push(picked);
                    }
                }
            }
        }
        Ok(out)
    }

    fn sign_raw(&self, picked: &PickedCert, data: &[u8]) -> Result<Vec<u8>> {
        let module = self
            .modules
            .get(picked.module_index)
            .ok_or_else(|| pkcs11_err("missing module"))?;
        let session = module
            .ctx
            .open_rw_session(picked.slot)
            .or_else(|_| module.ctx.open_ro_session(picked.slot))
            .map_err(pkcs11_err)?;

        login_user(
            module,
            &session,
            picked.slot,
            &picked.token_label,
            &self.pin,
        )?;
        let sig = sign_with_session(&session, picked, data);
        let _ = session.logout();
        sig
    }
}

impl KeyBackend for Pkcs11Backend {
    fn list(&self) -> Result<Vec<ListedCert>> {
        Ok(self
            .enumerate_certs()?
            .into_iter()
            .filter_map(|p| {
                let cert = Certificate::from_der(&p.der).ok()?;
                Some(ListedCert {
                    id: p.info.id.clone(),
                    der: p.der,
                    info: p.info,
                    kind: key_kind(&cert),
                })
            })
            .collect())
    }

    fn sign_bytes(&self, cert_id: &str, data: &[u8]) -> Result<Vec<u8>> {
        if cert_id.trim().is_empty() {
            return Err(Error::invalid(openpdf_core::InvalidKind::CertIdRequired));
        }
        let picked = self.pick(cert_id)?;
        if let Some(reason) = &picked.info.unusable {
            return Err(Error::Sign(openpdf_core::describe_unusable(reason)));
        }
        if !can_use_for_pades(&picked.info.usage) {
            return Err(Error::Sign(openpdf_core::tr(
                "certificate is not a signing cert",
            )));
        }
        self.sign_raw(&picked, data)
    }
}

impl Signer for Pkcs11Signer {
    fn list_certificates(&self) -> Result<Vec<CertificateInfo>> {
        list_certificates(&self.backend)
    }

    fn sign(&self, pdf: &[u8], request: &SignRequest) -> Result<Vec<u8>> {
        sign_pdf(&self.backend, pdf, request)
    }

    fn verify(&self, pdf: &[u8]) -> Result<Vec<SignatureInfo>> {
        pades::verify_signatures(pdf)
    }
}

/// Verify PAdES signatures without opening a PKCS#11 module.
pub fn verify_pdf(pdf: &[u8]) -> Result<Vec<SignatureInfo>> {
    pades::verify_signatures(pdf)
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
    cert::info_from_cert(cert, der, id)
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

fn pkcs11_ecdsa(
    session: &Session,
    key: ObjectHandle,
    data: &[u8],
    hash: EcdsaHash,
) -> Result<Vec<u8>> {
    // CKM_ECDSA: we pre-hash so SignerInfo (ecdsa-with-SHA*) matches the token.
    let digest: Vec<u8> = match hash {
        EcdsaHash::Sha256 => Sha256::digest(data).to_vec(),
        EcdsaHash::Sha384 => sha2::Sha384::digest(data).to_vec(),
        EcdsaHash::Sha512 => sha2::Sha512::digest(data).to_vec(),
    };
    let raw = session
        .sign(&Mechanism::Ecdsa, key, &digest)
        .map_err(pkcs11_err)?;
    cades::ecdsa_p1363_to_der(&raw, hash.field_len())
}

fn pkcs11_rsa_sha256(session: &Session, key: ObjectHandle, data: &[u8]) -> Result<Vec<u8>> {
    // CKM_RSA_PKCS over DigestInfo(SHA-256) so SignerInfo stays sha256WithRSAEncryption.
    let mut info = Vec::with_capacity(19 + 32);
    info.extend_from_slice(&[
        0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01,
        0x05, 0x00, 0x04, 0x20,
    ]);
    info.extend_from_slice(Sha256::digest(data).as_slice());
    match session.sign(&Mechanism::RsaPkcs, key, &info) {
        Ok(sig) => Ok(sig),
        Err(_) => session
            .sign(&Mechanism::Sha256RsaPkcs, key, data)
            .map_err(pkcs11_err),
    }
}

fn pkcs11_err(e: impl ToString) -> Error {
    Error::Sign(tr_args(
        "PKCS#11 error: {error}",
        &[("error", &e.to_string())],
    ))
}

fn login_user(
    module: &LoadedModule,
    session: &Session,
    slot: Slot,
    token_label: &str,
    pin: &Arc<dyn PinProvider>,
) -> Result<()> {
    let info = module.ctx.get_token_info(slot).map_err(pkcs11_err)?;
    if info.user_pin_locked() {
        return Err(Error::PinLocked);
    }
    // Pinpad / protected path: C_Login with a NULL PIN. Never prompt, never send "".
    if info.protected_authentication_path() {
        return ck_login(session, None);
    }
    let secret = pin.request_pin(token_label)?;
    if secret.as_str().trim().is_empty() {
        return Err(Error::PinRequired);
    }
    let auth = AuthPin::new(secret.as_str().into());
    let result = ck_login(session, Some(&auth));
    drop(secret);
    result
}

/// `CKR_USER_ALREADY_LOGGED_IN`: this session is already authenticated. Treat as
/// success so a second `C_Login` (consecutive signatures) does not fail.
fn ck_login(session: &Session, pin: Option<&AuthPin>) -> Result<()> {
    login_from_ck(session.login(UserType::User, pin))
}

fn login_from_ck(result: cryptoki::error::Result<()>) -> Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(CkError::Pkcs11(RvError::UserAlreadyLoggedIn, _)) => Ok(()),
        Err(e) => Err(pkcs11_err(e)),
    }
}

fn sign_with_session(session: &Session, picked: &PickedCert, data: &[u8]) -> Result<Vec<u8>> {
    let key = find_private_key(session, &picked.cka_id)?;
    let cert = Certificate::from_der(&picked.der).map_err(|e| Error::Sign(e.to_string()))?;
    let kind = key_kind(&cert)
        .ok_or_else(|| Error::Sign(openpdf_core::tr("unsupported public key algorithm")))?;
    match kind {
        KeyKind::Rsa => pkcs11_rsa_sha256(session, key, data),
        KeyKind::Ecdsa { hash } => pkcs11_ecdsa(session, key, data, hash),
    }
}

fn token_pin_status(info: &TokenInfo) -> TokenPinStatus {
    TokenPinStatus::new(
        info.user_pin_count_low(),
        info.user_pin_final_try(),
        info.user_pin_locked(),
        info.protected_authentication_path(),
    )
}

fn slot_id(slot: Slot) -> u64 {
    slot.id()
}

fn trim_pkcs11_label(label: &str) -> String {
    label.trim().trim_end_matches('\0').trim().to_string()
}

/// One PKCS#11 session kept open so tests can sign twice without `C_Logout`.
#[cfg(test)]
pub(crate) struct LiveSignSession<'a> {
    backend: &'a Pkcs11Backend,
    session: Session,
    picked: PickedCert,
}

#[cfg(test)]
impl Pkcs11Backend {
    pub(crate) fn open_live_session(&self, cert_id: &str) -> Result<LiveSignSession<'_>> {
        let picked = self.pick(cert_id)?;
        if let Some(reason) = &picked.info.unusable {
            return Err(Error::Sign(openpdf_core::describe_unusable(reason)));
        }
        if !can_use_for_pades(&picked.info.usage) {
            return Err(Error::Sign(openpdf_core::tr(
                "certificate is not a signing cert",
            )));
        }
        let module = self
            .modules
            .get(picked.module_index)
            .ok_or_else(|| pkcs11_err("missing module"))?;
        let session = module
            .ctx
            .open_rw_session(picked.slot)
            .or_else(|_| module.ctx.open_ro_session(picked.slot))
            .map_err(pkcs11_err)?;
        Ok(LiveSignSession {
            backend: self,
            session,
            picked,
        })
    }
}

#[cfg(test)]
impl LiveSignSession<'_> {
    /// Login (already-logged-in is success) then `C_Sign`. Leaves the session open.
    pub(crate) fn sign_bytes(&self, data: &[u8]) -> Result<Vec<u8>> {
        let module = self
            .backend
            .modules
            .get(self.picked.module_index)
            .ok_or_else(|| pkcs11_err("missing module"))?;
        login_user(
            module,
            &self.session,
            self.picked.slot,
            &self.picked.token_label,
            &self.backend.pin,
        )?;
        sign_with_session(&self.session, &self.picked, data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cryptoki::context::Function;

    #[test]
    fn already_logged_in_is_success() {
        assert!(login_from_ck(Ok(())).is_ok());
        assert!(login_from_ck(Err(CkError::Pkcs11(
            RvError::UserAlreadyLoggedIn,
            Function::Login,
        )))
        .is_ok());
        assert!(login_from_ck(Err(
            CkError::Pkcs11(RvError::PinIncorrect, Function::Login,)
        ))
        .is_err());
    }
}
