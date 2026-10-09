//! Windows certificate store via CNG / NCrypt.
//!
//! PIN / smart-card prompts are shown by Windows. This backend never collects one.

#![cfg(windows)]

use std::ffi::c_void;
use std::ptr::null_mut;
use std::slice;

use openpdf_core::{tr, tr_args, CertificateInfo, Error, Result, SignRequest, SignatureInfo, Signer};
use sha2::{Digest, Sha256, Sha384, Sha512};
use windows::core::{w, BOOL};
use windows::Win32::Security::Cryptography::{
    CertCloseStore, CertEnumCertificatesInStore, CertGetCertificateContextProperty, CertOpenStore,
    CryptAcquireCertificatePrivateKey, NCryptFreeObject, NCryptSignHash, BCRYPT_PKCS1_PADDING_INFO,
    CERT_CONTEXT, CERT_KEY_PROV_INFO_PROP_ID, CERT_KEY_SPEC, CERT_NCRYPT_KEY_SPEC,
    CERT_OPEN_STORE_FLAGS, CERT_QUERY_ENCODING_TYPE, CERT_STORE_OPEN_EXISTING_FLAG,
    CERT_STORE_PROV_SYSTEM_W, CERT_STORE_READONLY_FLAG, CERT_SYSTEM_STORE_CURRENT_USER,
    CRYPT_ACQUIRE_ONLY_NCRYPT_KEY_FLAG, CRYPT_ACQUIRE_PREFER_NCRYPT_KEY_FLAG, HCERTSTORE,
    HCRYPTPROV_OR_NCRYPT_KEY_HANDLE, NCRYPT_FLAGS, NCRYPT_KEY_HANDLE, NCRYPT_PAD_PKCS1_FLAG,
};

use crate::backend::{list_certificates, sign_pdf, KeyBackend, ListedCert};
use crate::cert::{EcdsaHash, KeyKind};
use crate::pades;

const ID_PREFIX: &str = "cng";

/// CurrentUser\\MY store. Windows owns any PIN dialog.
pub struct WindowsStoreSigner {
    backend: WindowsStoreBackend,
}

impl WindowsStoreSigner {
    pub fn new() -> Result<Self> {
        let backend = WindowsStoreBackend;
        let _ = backend.open_store()?;
        Ok(Self { backend })
    }
}

impl Default for WindowsStoreSigner {
    fn default() -> Self {
        Self {
            backend: WindowsStoreBackend,
        }
    }
}

impl Signer for WindowsStoreSigner {
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

pub(crate) struct WindowsStoreBackend;

impl WindowsStoreBackend {
    fn open_store(&self) -> Result<StoreHandle> {
        let flags = CERT_OPEN_STORE_FLAGS(CERT_SYSTEM_STORE_CURRENT_USER)
            | CERT_STORE_OPEN_EXISTING_FLAG
            | CERT_STORE_READONLY_FLAG;
        let store = unsafe {
            CertOpenStore(
                CERT_STORE_PROV_SYSTEM_W,
                CERT_QUERY_ENCODING_TYPE(0),
                None,
                flags,
                Some(w!("MY").as_ptr() as *const c_void),
            )
        }
        .map_err(|e| {
            Error::Sign(tr_args(
                "Windows certificate store error: {error}",
                &[("error", &e.to_string())],
            ))
        })?;
        if store.is_invalid() {
            return Err(Error::Sign(tr(
                "Could not open the Windows certificate store",
            )));
        }
        Ok(StoreHandle(store))
    }
}

struct StoreHandle(HCERTSTORE);

impl Drop for StoreHandle {
    fn drop(&mut self) {
        let _ = unsafe { CertCloseStore(Some(self.0), 0) };
    }
}

impl KeyBackend for WindowsStoreBackend {
    fn list(&self) -> Result<Vec<ListedCert>> {
        let store = self.open_store()?;
        let mut out = Vec::new();
        let mut prev: *const CERT_CONTEXT = null_mut();
        loop {
            let ctx = unsafe { CertEnumCertificatesInStore(store.0, Some(prev)) };
            if ctx.is_null() {
                break;
            }
            prev = ctx;
            if !has_private_key(ctx) {
                continue;
            }
            if let Some(listed) = context_to_listed(ctx) {
                out.push(listed);
            }
        }
        Ok(out)
    }

    fn sign_bytes(&self, cert_id: &str, data: &[u8]) -> Result<Vec<u8>> {
        let store = self.open_store()?;
        let mut prev: *const CERT_CONTEXT = null_mut();
        loop {
            let ctx = unsafe { CertEnumCertificatesInStore(store.0, Some(prev)) };
            if ctx.is_null() {
                break;
            }
            prev = ctx;
            let Some(listed) = context_to_listed(ctx) else {
                continue;
            };
            if listed.id != cert_id {
                continue;
            }
            return ncrypt_sign(ctx, data, listed.kind);
        }
        Err(Error::Sign(tr_args(
            "unknown certificate id {id}",
            &[("id", cert_id)],
        )))
    }
}

fn has_private_key(ctx: *const CERT_CONTEXT) -> bool {
    let mut cb = 0u32;
    unsafe {
        CertGetCertificateContextProperty(ctx, CERT_KEY_PROV_INFO_PROP_ID, None, &mut cb).is_ok()
            && cb > 0
    }
}

fn context_to_listed(ctx: *const CERT_CONTEXT) -> Option<ListedCert> {
    let der = unsafe {
        let ctx = ctx.as_ref()?;
        if ctx.pbCertEncoded.is_null() || ctx.cbCertEncoded == 0 {
            return None;
        }
        slice::from_raw_parts(ctx.pbCertEncoded, ctx.cbCertEncoded as usize).to_vec()
    };
    crate::cert::listed_from_der(ID_PREFIX, der, true)
}

fn ncrypt_sign(ctx: *const CERT_CONTEXT, data: &[u8], kind: KeyKind) -> Result<Vec<u8>> {
    let mut handle = HCRYPTPROV_OR_NCRYPT_KEY_HANDLE::default();
    let mut key_spec = CERT_KEY_SPEC::default();
    let mut caller_free = BOOL::default();
    // Do not pass CRYPT_ACQUIRE_SILENT_FLAG: Windows must be able to show a PIN.
    let flags = CRYPT_ACQUIRE_ONLY_NCRYPT_KEY_FLAG | CRYPT_ACQUIRE_PREFER_NCRYPT_KEY_FLAG;
    unsafe {
        CryptAcquireCertificatePrivateKey(
            ctx,
            flags,
            None,
            &mut handle,
            Some(&mut key_spec),
            Some(&mut caller_free),
        )
    }
    .map_err(|e| {
        Error::Sign(tr_args(
            "Windows CNG error: {error}",
            &[("error", &e.to_string())],
        ))
    })?;

    if key_spec != CERT_NCRYPT_KEY_SPEC {
        if caller_free.as_bool() {
            let _ = unsafe { NCryptFreeObject(NCRYPT_KEY_HANDLE(handle.0).into()) };
        }
        return Err(Error::Sign(tr("certificate has no usable private key")));
    }

    let key = NCRYPT_KEY_HANDLE(handle.0);
    let result = sign_with_ncrypt(key, data, kind);
    if caller_free.as_bool() {
        let _ = unsafe { NCryptFreeObject(key.into()) };
    }
    result
}

fn sign_with_ncrypt(key: NCRYPT_KEY_HANDLE, data: &[u8], kind: KeyKind) -> Result<Vec<u8>> {
    match kind {
        KeyKind::Rsa => {
            let hash = Sha256::digest(data);
            let mut padding = BCRYPT_PKCS1_PADDING_INFO {
                pszAlgId: w!("SHA256"),
            };
            ncrypt_sign_hash(
                key,
                Some((&mut padding as *mut BCRYPT_PKCS1_PADDING_INFO).cast()),
                hash.as_slice(),
                NCRYPT_PAD_PKCS1_FLAG,
            )
        }
        KeyKind::Ecdsa { hash } => {
            let digest: Vec<u8> = match hash {
                EcdsaHash::Sha256 => Sha256::digest(data).to_vec(),
                EcdsaHash::Sha384 => Sha384::digest(data).to_vec(),
                EcdsaHash::Sha512 => Sha512::digest(data).to_vec(),
            };
            let raw = ncrypt_sign_hash(key, None, &digest, NCRYPT_FLAGS(0))?;
            if raw.len() < 2 || !raw.len().is_multiple_of(2) {
                return Err(Error::Sign(tr("NCryptSignHash failed")));
            }
            let mid = raw.len() / 2;
            crate::cades::ecdsa_integers_to_der(&raw[..mid], &raw[mid..])
        }
    }
}

fn ncrypt_sign_hash(
    key: NCRYPT_KEY_HANDLE,
    padding: Option<*const c_void>,
    hash: &[u8],
    flags: NCRYPT_FLAGS,
) -> Result<Vec<u8>> {
    let mut needed = 0u32;
    unsafe { NCryptSignHash(key, padding, hash, None, &mut needed, flags) }
        .map_err(|_| Error::Sign(tr("NCryptSignHash failed")))?;
    let mut buf = vec![0u8; needed as usize];
    let mut written = 0u32;
    unsafe { NCryptSignHash(key, padding, hash, Some(&mut buf), &mut written, flags) }
        .map_err(|_| Error::Sign(tr("NCryptSignHash failed")))?;
    buf.truncate(written as usize);
    Ok(buf)
}
