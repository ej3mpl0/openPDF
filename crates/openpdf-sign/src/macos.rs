//! macOS Keychain identities via Security.framework.
//!
//! Keychain / smart-card / token prompts are shown by the OS. This backend
//! never collects a PIN.

#![cfg(target_os = "macos")]

use std::path::PathBuf;

use openpdf_core::{
    tr_args, CertificateInfo, Error, Result, SignRequest, SignatureInfo, Signer,
};
use security_framework::item::{
    ItemClass, ItemSearchOptions, Limit, Reference, SearchResult,
};
use security_framework::key::Algorithm;
use security_framework::os::macos::keychain::SecKeychain;

use crate::backend::{list_certificates, sign_pdf, KeyBackend, ListedCert};
use crate::cert::{EcdsaHash, KeyKind};
use crate::pades;

const ID_PREFIX: &str = "keychain";

/// Default keychain search list, or `OPENPDF_TEST_KEYCHAIN` when set (CI).
pub struct KeychainSigner {
    backend: KeychainBackend,
}

impl KeychainSigner {
    pub fn new() -> Self {
        Self {
            backend: KeychainBackend::default(),
        }
    }
}

impl Default for KeychainSigner {
    fn default() -> Self {
        Self::new()
    }
}

impl Signer for KeychainSigner {
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

#[derive(Default)]
pub(crate) struct KeychainBackend;

impl KeychainBackend {
    fn search(&self) -> Result<Vec<ListedIdentity>> {
        let mut opts = ItemSearchOptions::new();
        opts.class(ItemClass::identity())
            .load_refs(true)
            .limit(Limit::All);
        let keychain;
        if let Some(path) = test_keychain_path() {
            keychain = SecKeychain::open(&path).map_err(|e| {
                Error::Sign(tr_args(
                    "Keychain error: {error}",
                    &[("error", &e.to_string())],
                ))
            })?;
            opts.keychains(&[keychain.clone()]);
        }
        let results = opts.search().map_err(|e| {
            Error::Sign(tr_args(
                "Could not search the Keychain for identities: {error}",
                &[("error", &e.to_string())],
            ))
        })?;
        let mut out = Vec::new();
        for item in results {
            let SearchResult::Ref(Reference::Identity(identity)) = item else {
                continue;
            };
            let cert = match identity.certificate() {
                Ok(c) => c,
                Err(_) => continue,
            };
            let der = cert.to_der();
            let Some(listed) = crate::cert::listed_from_der(ID_PREFIX, der) else {
                continue;
            };
            // Must have a private key (SecIdentity implies one; confirm we can copy it).
            if identity.private_key().is_err() {
                continue;
            }
            out.push(ListedIdentity { listed, identity });
        }
        Ok(out)
    }
}

struct ListedIdentity {
    listed: ListedCert,
    identity: security_framework::identity::SecIdentity,
}

impl KeyBackend for KeychainBackend {
    fn list(&self) -> Result<Vec<ListedCert>> {
        Ok(self.search()?.into_iter().map(|i| i.listed).collect())
    }

    fn sign_bytes(&self, cert_id: &str, data: &[u8]) -> Result<Vec<u8>> {
        let found = self
            .search()?
            .into_iter()
            .find(|i| i.listed.id == cert_id)
            .ok_or_else(|| {
                Error::Sign(tr_args(
                    "unknown certificate id {id}",
                    &[("id", cert_id)],
                ))
            })?;
        let key = found.identity.private_key().map_err(|e| {
            Error::Sign(tr_args(
                "Keychain error: {error}",
                &[("error", &e.to_string())],
            ))
        })?;
        let alg = match found.listed.kind {
            Some(KeyKind::Rsa) => Algorithm::RSASignatureMessagePKCS1v15SHA256,
            Some(KeyKind::Ecdsa { hash }) => match hash {
                EcdsaHash::Sha256 => Algorithm::ECDSASignatureMessageX962SHA256,
                EcdsaHash::Sha384 => Algorithm::ECDSASignatureMessageX962SHA384,
                EcdsaHash::Sha512 => Algorithm::ECDSASignatureMessageX962SHA512,
            },
            None => return Err(Error::Sign(openpdf_core::tr("unsupported public key algorithm"))),
        };
        key.create_signature(alg, data).map_err(|e| {
            Error::Sign(tr_args(
                "SecKeyCreateSignature failed: {error}",
                &[("error", &e.to_string())],
            ))
        })
    }
}

fn test_keychain_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        return std::env::var_os("OPENPDF_TEST_KEYCHAIN").map(PathBuf::from);
    }
    #[cfg(not(test))]
    None
}
