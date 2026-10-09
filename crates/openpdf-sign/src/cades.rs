//! Detached CAdES-BES (PAdES-B-B) CMS SignedData.

use cms::cert::{CertificateChoices, IssuerAndSerialNumber};
use cms::content_info::{CmsVersion, ContentInfo};
use cms::signed_data::{
    CertificateSet, EncapsulatedContentInfo, SignedData, SignerIdentifier, SignerInfo, SignerInfos,
};
use der::asn1::{OctetString, SetOfVec};
use der::{Any, Decode, Encode, Sequence};
use openpdf_core::{Error, Result};
use rsa::pkcs1v15::{Signature, VerifyingKey};
use rsa::pkcs8::DecodePublicKey;
use rsa::signature::Verifier;
use rsa::RsaPublicKey;
use sha2::{Digest, Sha256};
use x509_cert::attr::Attribute;
use x509_cert::spki::AlgorithmIdentifierOwned;
use x509_cert::Certificate;

const ID_DATA: const_oid::ObjectIdentifier = const_oid::db::rfc5911::ID_DATA;
const ID_SIGNED_DATA: const_oid::ObjectIdentifier = const_oid::db::rfc5911::ID_SIGNED_DATA;
const ID_CONTENT_TYPE: const_oid::ObjectIdentifier = const_oid::db::rfc5911::ID_CONTENT_TYPE;
const ID_MESSAGE_DIGEST: const_oid::ObjectIdentifier = const_oid::db::rfc5911::ID_MESSAGE_DIGEST;
const ID_AA_SIGNING_CERTIFICATE_V2: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.47");
const ID_RSA_ENCRYPTION: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");

/// RFC 5035 `ESSCertIDv2` with the SHA-256 default algorithm omitted.
#[derive(Clone, Debug, Eq, PartialEq, Sequence)]
struct EssCertIdV2 {
    cert_hash: OctetString,
}

/// RFC 5035 `SigningCertificateV2` (PAdES-B-B / CAdES-BES binding).
#[derive(Clone, Debug, Eq, PartialEq, Sequence)]
struct SigningCertificateV2 {
    certs: Vec<EssCertIdV2>,
}

/// Sign `message` (the PDF `/ByteRange` bytes) and return a detached CMS blob.
pub(crate) fn sign_detached(
    message: &[u8],
    cert_der: &[u8],
    sign: impl FnOnce(&[u8]) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    let cert = Certificate::from_der(cert_der).map_err(cms_err)?;
    let digest = Sha256::digest(message);
    let digest_alg = sha256_alg();
    let encap = EncapsulatedContentInfo {
        econtent_type: ID_DATA,
        econtent: None,
    };

    let signed_attrs = signed_attributes(cert_der, digest.as_slice())?;
    let tbs = signed_attrs.to_der().map_err(cms_err)?;
    let signature = sign(&tbs)?;
    let sig_octets = OctetString::new(signature).map_err(cms_err)?;

    let signer_info = SignerInfo {
        version: CmsVersion::V1,
        sid: SignerIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
            issuer: cert.tbs_certificate.issuer.clone(),
            serial_number: cert.tbs_certificate.serial_number.clone(),
        }),
        digest_alg: digest_alg.clone(),
        signed_attrs: Some(signed_attrs),
        signature_algorithm: AlgorithmIdentifierOwned {
            oid: const_oid::db::rfc5912::SHA_256_WITH_RSA_ENCRYPTION,
            parameters: Some(der::Any::null()),
        },
        signature: sig_octets,
        unsigned_attrs: None,
    };

    let mut digest_algs = SetOfVec::new();
    digest_algs.insert(digest_alg).map_err(cms_err)?;
    let mut certs = SetOfVec::new();
    certs
        .insert(CertificateChoices::Certificate(cert))
        .map_err(cms_err)?;
    let mut infos = SetOfVec::new();
    infos.insert(signer_info).map_err(cms_err)?;

    let signed_data = SignedData {
        version: CmsVersion::V1,
        digest_algorithms: digest_algs,
        encap_content_info: encap,
        certificates: Some(CertificateSet(certs)),
        crls: None,
        signer_infos: SignerInfos(infos),
    };
    let sd_der = signed_data.to_der().map_err(cms_err)?;
    let content_info = ContentInfo {
        content_type: ID_SIGNED_DATA,
        content: Any::from_der(&sd_der).map_err(cms_err)?,
    };
    content_info.to_der().map_err(cms_err)
}

fn signed_attributes(cert_der: &[u8], digest: &[u8]) -> Result<x509_cert::attr::Attributes> {
    let content_type = attribute(ID_CONTENT_TYPE, Any::from(&ID_DATA))?;
    let md = OctetString::new(digest.to_vec()).map_err(cms_err)?;
    let message_digest = attribute(
        ID_MESSAGE_DIGEST,
        Any::from_der(&md.to_der().map_err(cms_err)?).map_err(cms_err)?,
    )?;
    let scv2 = signing_cert_v2_attribute(cert_der)?;
    SetOfVec::try_from(vec![content_type, message_digest, scv2]).map_err(cms_err)
}

fn attribute(oid: const_oid::ObjectIdentifier, value: Any) -> Result<Attribute> {
    Ok(Attribute {
        oid,
        values: SetOfVec::try_from(vec![value]).map_err(cms_err)?,
    })
}

fn sha256_alg() -> AlgorithmIdentifierOwned {
    AlgorithmIdentifierOwned {
        oid: const_oid::db::rfc5912::ID_SHA_256,
        parameters: None,
    }
}

fn signing_cert_v2_attribute(cert_der: &[u8]) -> Result<Attribute> {
    let hash = Sha256::digest(cert_der);
    let body = SigningCertificateV2 {
        certs: vec![EssCertIdV2 {
            cert_hash: OctetString::new(hash.to_vec()).map_err(cms_err)?,
        }],
    };
    let der = body.to_der().map_err(cms_err)?;
    attribute(
        ID_AA_SIGNING_CERTIFICATE_V2,
        Any::from_der(&der).map_err(cms_err)?,
    )
}

fn cms_err(e: impl ToString) -> Error {
    Error::Sign(e.to_string())
}

/// Result of checking a detached CMS blob against the `/ByteRange` bytes.
#[derive(Clone, Debug)]
pub(crate) struct CmsCheck {
    pub valid: bool,
    pub signer: Option<String>,
}

pub(crate) fn verify_detached(cms_der: &[u8], message: &[u8]) -> CmsCheck {
    match verify_inner(cms_der, message) {
        Some(c) => c,
        None => CmsCheck {
            valid: false,
            signer: None,
        },
    }
}

fn verify_inner(cms_der: &[u8], message: &[u8]) -> Option<CmsCheck> {
    let content_info = ContentInfo::from_der(cms_der).ok()?;
    let signed_data = content_info.content.decode_as::<SignedData>().ok()?;
    let signer_info = signed_data.signer_infos.0.as_slice().first()?;
    let cert = embedded_leaf(&signed_data)?;
    let signer = Some(cert.tbs_certificate.subject.to_string());

    let expected = Sha256::digest(message);
    let signed_attrs = signer_info.signed_attrs.as_ref()?;
    let claimed = signed_attrs
        .iter()
        .find(|attr| attr.oid == ID_MESSAGE_DIGEST)
        .and_then(|attr| attr.values.as_slice().first())
        .and_then(|value| value.decode_as::<OctetString>().ok())?;
    if claimed.as_bytes() != expected.as_slice() {
        return Some(CmsCheck {
            valid: false,
            signer,
        });
    }

    let tbs = SetOfVec::from_iter(signed_attrs.iter().cloned())
        .ok()?
        .to_der()
        .ok()?;
    let spki_der = cert.tbs_certificate.subject_public_key_info.to_der().ok()?;
    let public_key = RsaPublicKey::from_public_key_der(&spki_der).ok()?;
    let signature = Signature::try_from(signer_info.signature.as_bytes()).ok()?;
    let sig_oid = signer_info.signature_algorithm.oid;
    let rsa_ok = sig_oid == const_oid::db::rfc5912::SHA_256_WITH_RSA_ENCRYPTION
        || sig_oid == ID_RSA_ENCRYPTION;
    if !rsa_ok {
        return Some(CmsCheck {
            valid: false,
            signer,
        });
    }
    let valid = VerifyingKey::<Sha256>::new(public_key)
        .verify(&tbs, &signature)
        .is_ok();
    Some(CmsCheck { valid, signer })
}

fn embedded_leaf(signed_data: &SignedData) -> Option<&Certificate> {
    let certs = signed_data.certificates.as_ref()?;
    certs.0.iter().find_map(|choice| match choice {
        CertificateChoices::Certificate(c) => Some(c),
        _ => None,
    })
}
