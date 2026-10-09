//! Shared X.509 helpers used by every key backend.

use const_oid::AssociatedOid;
use der::Decode;
use openpdf_core::{CertificateInfo, CertificateUsage, CertificateValidity};
use sha2::{Digest, Sha256};
use x509_cert::ext::pkix::KeyUsage;
use x509_cert::Certificate;

use crate::pades;

const ID_RSA_ENCRYPTION: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
const ID_EC_PUBLIC_KEY: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
const SECP_256_R1: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");
const SECP_384_R1: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("1.3.132.0.34");
const SECP_521_R1: const_oid::ObjectIdentifier =
    const_oid::ObjectIdentifier::new_unwrap("1.3.132.0.35");

/// Public-key algorithm we can put in a PAdES CMS blob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyKind {
    Rsa,
    Ecdsa { hash: EcdsaHash },
}

/// Hash used to sign CMS signed attributes with an ECDSA key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EcdsaHash {
    Sha256,
    Sha384,
    Sha512,
}

pub(crate) fn key_kind(cert: &Certificate) -> Option<KeyKind> {
    let oid = cert.tbs_certificate.subject_public_key_info.algorithm.oid;
    if oid == ID_RSA_ENCRYPTION
        || oid == const_oid::db::rfc5912::SHA_256_WITH_RSA_ENCRYPTION
    {
        return Some(KeyKind::Rsa);
    }
    if oid != ID_EC_PUBLIC_KEY {
        return None;
    }
    let curve = cert
        .tbs_certificate
        .subject_public_key_info
        .algorithm
        .parameters
        .as_ref()
        .and_then(|p| p.decode_as::<const_oid::ObjectIdentifier>().ok());
    let hash = match curve {
        Some(c) if c == SECP_384_R1 => EcdsaHash::Sha384,
        Some(c) if c == SECP_521_R1 => EcdsaHash::Sha512,
        Some(c) if c == SECP_256_R1 => EcdsaHash::Sha256,
        _ => EcdsaHash::Sha256,
    };
    Some(KeyKind::Ecdsa { hash })
}

pub(crate) fn key_usage(cert: &Certificate) -> Vec<CertificateUsage> {
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
        } else if ku.digital_signature() && ku.non_repudiation() {
            if !usage.contains(&CertificateUsage::Signing) {
                usage.push(CertificateUsage::Signing);
            }
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

/// PKCS#11 / DNIe: empty usage or non-repudiation (`Signing`).
pub(crate) fn can_sign_pkcs11(usage: &[CertificateUsage]) -> bool {
    usage.is_empty() || usage.contains(&CertificateUsage::Signing)
}

/// Native stores: `digitalSignature` maps to Signing and/or Authentication.
pub(crate) fn can_sign_native(usage: &[CertificateUsage]) -> bool {
    usage.is_empty()
        || usage.contains(&CertificateUsage::Signing)
        || usage.contains(&CertificateUsage::Authentication)
}

/// Shared PAdES gate: digitalSignature (auth or sign) or unspecified.
pub(crate) fn can_use_for_pades(usage: &[CertificateUsage]) -> bool {
    can_sign_native(usage)
}

pub(crate) fn time_rfc3339(t: &x509_cert::time::Time) -> Option<String> {
    let secs = t.to_unix_duration().as_secs();
    let (y, m, d, h, min, s) = pades::civil_utc(secs);
    Some(format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}Z"))
}

pub(crate) fn common_name_or_subject(cert: &Certificate) -> String {
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

#[allow(dead_code)] // used by Windows / macOS native backends
pub(crate) fn cert_sha256_id(prefix: &str, der: &[u8]) -> String {
    let hash = Sha256::digest(der);
    format!("{prefix}:{}", pades::hex_encode_lower(&hash[..16]))
}

pub(crate) fn info_from_cert(cert: &Certificate, _der: &[u8], id: String) -> CertificateInfo {
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

#[allow(dead_code)] // used by Windows / macOS native backends
pub(crate) fn listed_from_der(
    prefix: &str,
    der: Vec<u8>,
    require_digital_signature: bool,
) -> Option<crate::backend::ListedCert> {
    let cert = Certificate::from_der(&der).ok()?;
    let kind = key_kind(&cert)?;
    let usage = key_usage(&cert);
    if require_digital_signature && !can_sign_native(&usage) {
        return None;
    }
    let id = cert_sha256_id(prefix, &der);
    let info = info_from_cert(&cert, &der, id.clone());
    Some(crate::backend::ListedCert { id, der, info, kind })
}
