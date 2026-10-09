//! Internal key backend: enumerate certificates and sign opaque bytes.

use openpdf_core::{
    tr, tr_args, CertificateInfo, Error, InvalidKind, PinProvider, Result, SignRequest,
    SignatureInfo, Signer,
};
use der::Decode;
use x509_cert::Certificate;

use crate::cades;
use crate::cert::{can_use_for_pades, common_name_or_subject, KeyKind};
use crate::pades;
use crate::token::{Pkcs11Backend, Pkcs11Config};

/// One certificate a [`KeyBackend`] can sign with.
#[derive(Clone, Debug)]
pub(crate) struct ListedCert {
    pub id: String,
    pub der: Vec<u8>,
    pub info: CertificateInfo,
    #[allow(dead_code)] // read by Windows CNG and macOS Keychain signers
    pub kind: KeyKind,
}

/// Platform or PKCS#11 source. Backends do not build CMS or touch `/ByteRange`.
pub(crate) trait KeyBackend: Send + Sync {
    fn list(&self) -> Result<Vec<ListedCert>>;

    /// Raw signature over `data` (the DER of CMS signed attributes).
    fn sign_bytes(&self, cert_id: &str, data: &[u8]) -> Result<Vec<u8>>;
}

pub(crate) fn list_certificates(backend: &dyn KeyBackend) -> Result<Vec<CertificateInfo>> {
    Ok(backend.list()?.into_iter().map(|c| c.info).collect())
}

pub(crate) fn sign_pdf(
    backend: &dyn KeyBackend,
    pdf: &[u8],
    request: &SignRequest,
) -> Result<Vec<u8>> {
    if request.cert_id.trim().is_empty() {
        return Err(Error::invalid(InvalidKind::CertIdRequired));
    }
    let listed = backend.list()?;
    let picked = listed
        .iter()
        .find(|c| c.info.id == request.cert_id.trim())
        .ok_or_else(|| {
            Error::Sign(tr_args(
                "unknown certificate id {id}",
                &[("id", request.cert_id.trim())],
            ))
        })?;
    if !can_use_for_pades(&picked.info.usage) {
        return Err(Error::Sign(tr("certificate is not a signing cert")));
    }
    let cert = Certificate::from_der(&picked.der).map_err(|e| Error::Sign(e.to_string()))?;
    let name = common_name_or_subject(&cert);
    let der = picked.der.clone();
    let id = picked.info.id.clone();
    pades::sign_incremental(pdf, request, &name, |message| {
        cades::sign_detached(message, &der, |tbs| backend.sign_bytes(&id, tbs))
    })
}

/// Native OS store (when compiled in) plus PKCS#11 on the same listing.
pub struct CompositeSigner {
    backends: Vec<Box<dyn KeyBackend>>,
}

impl CompositeSigner {
    pub fn new(pin: impl PinProvider + 'static) -> Result<Self> {
        let pin: std::sync::Arc<dyn PinProvider> = std::sync::Arc::new(pin);
        let mut backends: Vec<Box<dyn KeyBackend>> = Vec::new();

        #[cfg(windows)]
        {
            backends.push(Box::new(crate::windows::WindowsStoreBackend));
        }
        #[cfg(target_os = "macos")]
        {
            backends.push(Box::new(crate::macos::KeychainBackend::default()));
        }

        match Pkcs11Backend::new(Pkcs11Config::default(), pin) {
            Ok(pkcs11) => {
                if backends.is_empty() {
                    backends.push(Box::new(pkcs11));
                } else {
                    backends.push(Box::new(PrefixedBackend {
                        prefix: "pkcs11:".into(),
                        inner: pkcs11,
                    }));
                }
            }
            Err(e) => {
                if backends.is_empty() {
                    return Err(e);
                }
            }
        }

        Ok(Self { backends })
    }
}

impl Signer for CompositeSigner {
    fn list_certificates(&self) -> Result<Vec<CertificateInfo>> {
        let mut out = Vec::new();
        let mut last_err = None;
        for backend in &self.backends {
            match backend.list() {
                Ok(certs) => out.extend(certs.into_iter().map(|c| c.info)),
                Err(e) => last_err = Some(e),
            }
        }
        if out.is_empty() {
            if let Some(e) = last_err {
                return Err(e);
            }
        }
        Ok(out)
    }

    fn sign(&self, pdf: &[u8], request: &SignRequest) -> Result<Vec<u8>> {
        if request.cert_id.trim().is_empty() {
            return Err(Error::invalid(InvalidKind::CertIdRequired));
        }
        let wanted = request.cert_id.trim();
        let mut last_err = None;
        for backend in &self.backends {
            match backend.list() {
                Ok(certs) => {
                    if certs.iter().any(|c| c.info.id == wanted) {
                        return sign_pdf(backend.as_ref(), pdf, request);
                    }
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| {
            Error::Sign(tr_args(
                "unknown certificate id {id}",
                &[("id", wanted)],
            ))
        }))
    }

    fn verify(&self, pdf: &[u8]) -> Result<Vec<SignatureInfo>> {
        pades::verify_signatures(pdf)
    }
}

struct PrefixedBackend<B> {
    prefix: String,
    inner: B,
}

impl<B: KeyBackend> KeyBackend for PrefixedBackend<B> {
    fn list(&self) -> Result<Vec<ListedCert>> {
        Ok(self
            .inner
            .list()?
            .into_iter()
            .map(|mut c| {
                c.id = format!("{}{}", self.prefix, c.id);
                c.info.id = c.id.clone();
                c
            })
            .collect())
    }

    fn sign_bytes(&self, cert_id: &str, data: &[u8]) -> Result<Vec<u8>> {
        let id = cert_id
            .strip_prefix(&self.prefix)
            .unwrap_or(cert_id);
        self.inner.sign_bytes(id, data)
    }
}

/// Native store + PKCS#11 combined listing for the current OS.
pub fn default_signer(pin: impl PinProvider + 'static) -> Result<Box<dyn Signer>> {
    Ok(Box::new(CompositeSigner::new(pin)?))
}
