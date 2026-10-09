//! SoftHSM2 integration plus PIN / verify unit tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::error::{Error as CkError, RvError};
use cryptoki::mechanism::Mechanism;
use cryptoki::object::{Attribute, AttributeType, CertificateType, ObjectClass};
use cryptoki::session::UserType;
use cryptoki::types::AuthPin;
use der::asn1::{BitString, UintRef};
use der::{Decode, Encode, Sequence};
use openpdf_core::test_support::with_locale;
use openpdf_core::{
    describe_certificate, describe_unusable, format_verify_result, format_verify_results,
    pin_prompt_text, tr, CertUnusable, Document, Error, PinProvider, SecretPin, SignRequest,
    Signer, VisibleAppearance,
};
use x509_cert::name::Name;
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::{AlgorithmIdentifierOwned, SubjectPublicKeyInfoOwned};
use x509_cert::time::Validity;
use x509_cert::{Certificate, TbsCertificate, Version};

use crate::backend::{sign_pdf, KeyBackend, ListedCert};
use crate::cades;
use crate::cert::{unusable_reason, EcdsaHash, KeyKind};
use crate::pin::StaticPinProvider;
use crate::token::{Pkcs11Config, Pkcs11Signer};
use crate::{default_signer, verify_pdf, CertificateUsage};

const USER_PIN: &str = "1234";
const SO_PIN: &str = "12345678";
const KEY_ID: &[u8] = &[0x01];

struct TestEnv {
    module: PathBuf,
}

fn softhsm_module() -> Option<PathBuf> {
    [
        "/usr/lib/softhsm/libsofthsm2.so",
        "/usr/lib/x86_64-linux-gnu/softhsm/libsofthsm2.so",
        "/usr/lib/x86_64-linux-gnu/pkcs11/libsofthsm2.so",
        "/usr/local/lib/softhsm/libsofthsm2.so",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|p| p.exists())
}

fn require_softhsm() -> PathBuf {
    match softhsm_module() {
        Some(p) => p,
        None if std::env::var_os("CI").is_some() && cfg!(target_os = "linux") => {
            panic!("SoftHSM2 is required in CI (apt install softhsm2)")
        }
        None => panic!("skip-not-soft: no libsofthsm2.so"),
    }
}

fn test_env() -> &'static TestEnv {
    static ENV: OnceLock<TestEnv> = OnceLock::new();
    static LOCK: Mutex<()> = Mutex::new(());
    let module = require_softhsm();
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    ENV.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "openpdf-sign-softhsm-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(dir.join("tokens")).unwrap();
        let conf = dir.join("softhsm2.conf");
        fs::write(
            &conf,
            format!(
                "directories.tokendir = {}\nobjectstore.backend = file\nlog.level = ERROR\n",
                dir.join("tokens").display()
            ),
        )
        .unwrap();
        std::env::set_var("SOFTHSM2_CONF", &conf);
        provision_token(&module);
        TestEnv {
            module: module.clone(),
        }
    })
}

#[derive(Sequence)]
struct RsaPub<'a> {
    n: UintRef<'a>,
    e: UintRef<'a>,
}

fn provision_token(module: &Path) {
    let pkcs11 = Pkcs11::new(module).expect("load softhsm");
    match pkcs11.initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK)) {
        Ok(()) => {}
        Err(CkError::Pkcs11(RvError::CryptokiAlreadyInitialized, _)) => {}
        Err(e) => panic!("C_Initialize: {e}"),
    }
    let slot = pkcs11
        .get_slots_with_token()
        .expect("slots")
        .into_iter()
        .next()
        .expect("softhsm slot");
    let so = AuthPin::new(SO_PIN.into());
    pkcs11
        .init_token(slot, &so, "openpdf-test")
        .expect("init_token");
    let session = pkcs11.open_rw_session(slot).expect("rw session");
    session.login(UserType::So, Some(&so)).expect("SO login");
    session
        .init_pin(&AuthPin::new(USER_PIN.into()))
        .expect("init_pin");
    session.logout().ok();
    session
        .login(UserType::User, Some(&AuthPin::new(USER_PIN.into())))
        .expect("user login");

    let pub_tmpl = [
        Attribute::Token(true),
        Attribute::Private(false),
        Attribute::Verify(true),
        Attribute::ModulusBits(2048.into()),
        Attribute::PublicExponent(vec![0x01, 0x00, 0x01]),
        Attribute::Id(KEY_ID.to_vec()),
        Attribute::Label(b"openpdf-sign".to_vec()),
    ];
    let priv_tmpl = [
        Attribute::Token(true),
        Attribute::Private(true),
        Attribute::Sensitive(true),
        Attribute::Sign(true),
        Attribute::Id(KEY_ID.to_vec()),
        Attribute::Label(b"openpdf-sign".to_vec()),
    ];
    let (public, private) = session
        .generate_key_pair(&Mechanism::RsaPkcsKeyPairGen, &pub_tmpl, &priv_tmpl)
        .expect("keypair");

    let attrs = session
        .get_attributes(
            public,
            &[AttributeType::Modulus, AttributeType::PublicExponent],
        )
        .expect("pub attrs");
    let mut modulus = None;
    let mut exponent = None;
    for a in attrs {
        match a {
            Attribute::Modulus(v) => modulus = Some(v),
            Attribute::PublicExponent(v) => exponent = Some(v),
            _ => {}
        }
    }
    let modulus = modulus.expect("modulus");
    let exponent = exponent.expect("exponent");
    let pkcs1 = RsaPub {
        n: UintRef::new(&modulus).expect("n"),
        e: UintRef::new(&exponent).expect("e"),
    };
    let pkcs1_der = pkcs1.to_der().expect("pkcs1");
    let spki = SubjectPublicKeyInfoOwned {
        algorithm: AlgorithmIdentifierOwned {
            oid: const_oid::db::rfc5912::RSA_ENCRYPTION,
            parameters: Some(der::Any::null()),
        },
        subject_public_key: BitString::from_bytes(&pkcs1_der).expect("bitstring"),
    };

    let name: Name = "CN=OpenPDF Test Signer".parse().expect("name");
    let validity = Validity::from_now(Duration::from_secs(365 * 86400)).expect("validity");
    let sig_alg = AlgorithmIdentifierOwned {
        oid: const_oid::db::rfc5912::SHA_256_WITH_RSA_ENCRYPTION,
        parameters: Some(der::Any::null()),
    };
    let tbs = TbsCertificate {
        version: Version::V3,
        serial_number: SerialNumber::from(1u32),
        signature: sig_alg.clone(),
        issuer: name.clone(),
        validity,
        subject: name,
        subject_public_key_info: spki,
        issuer_unique_id: None,
        subject_unique_id: None,
        extensions: None,
    };
    let tbs_der = tbs.to_der().expect("tbs der");
    let sig = session
        .sign(&Mechanism::Sha256RsaPkcs, private, &tbs_der)
        .expect("sign tbs");
    let cert = Certificate {
        tbs_certificate: tbs,
        signature_algorithm: sig_alg,
        signature: BitString::from_bytes(&sig).expect("sig bits"),
    };
    let cert_der = cert.to_der().expect("cert der");
    let subject_der = cert.tbs_certificate.subject.to_der().expect("subject");
    let issuer_der = cert.tbs_certificate.issuer.to_der().expect("issuer");
    let serial_der = cert.tbs_certificate.serial_number.to_der().expect("serial");
    session
        .create_object(&[
            Attribute::Class(ObjectClass::CERTIFICATE),
            Attribute::CertificateType(CertificateType::X_509),
            Attribute::Token(true),
            Attribute::Private(false),
            Attribute::Label(b"openpdf-sign".to_vec()),
            Attribute::Id(KEY_ID.to_vec()),
            Attribute::Subject(subject_der),
            Attribute::Issuer(issuer_der),
            Attribute::SerialNumber(serial_der),
            Attribute::Value(cert_der),
        ])
        .expect("store cert");
}

fn signer() -> Pkcs11Signer {
    let env = test_env();
    Pkcs11Signer::new(
        Pkcs11Config::with_module(&env.module),
        StaticPinProvider::new(USER_PIN),
    )
    .expect("Pkcs11Signer")
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .join(name)
}

fn catch_skip<T>(f: impl FnOnce() -> T + std::panic::UnwindSafe) -> Option<T> {
    match std::panic::catch_unwind(f) {
        Ok(v) => Some(v),
        Err(p) => {
            if p.downcast_ref::<String>()
                .is_some_and(|s| s.starts_with("skip-not-soft"))
                || p.downcast_ref::<&str>()
                    .is_some_and(|s| s.starts_with("skip-not-soft"))
            {
                eprintln!("skipping SoftHSM test: libsofthsm2.so not installed");
                None
            } else {
                std::panic::resume_unwind(p);
            }
        }
    }
}

/// SoftHSM / PKCS#11 is process-global and not safe to drive from several tests at once.
fn with_softhsm<T>(f: impl FnOnce() -> T + std::panic::UnwindSafe) -> Option<T> {
    static PKCS11: Mutex<()> = Mutex::new(());
    catch_skip(|| {
        let _g = PKCS11.lock().unwrap_or_else(|e| e.into_inner());
        f()
    })
}

struct PanicPin;

impl PinProvider for PanicPin {
    fn request_pin(&self, _token_label: &str) -> openpdf_core::Result<SecretPin> {
        panic!("PIN must not be requested when listing certificates");
    }
}

#[test]
fn secret_pin_is_zeroizing() {
    let pin = SecretPin::new("1234");
    assert_eq!(pin.as_str(), "1234");
    drop(pin);
}

#[test]
fn verify_unsigned_corpus() {
    let pdf = fs::read(corpus("hello.pdf")).unwrap();
    assert!(verify_pdf(&pdf).unwrap().is_empty());
    with_locale("en", || {
        assert_eq!(format_verify_results(&[]), tr("No signatures found"));
    });
}

#[test]
fn list_certificates_does_not_ask_for_pin() {
    let Some(()) = with_softhsm(|| {
        let env = test_env();
        let signer =
            Pkcs11Signer::new(Pkcs11Config::with_module(&env.module), PanicPin).expect("signer");
        let certs = signer.list_certificates().expect("list");
        assert!(
            !certs.is_empty(),
            "SoftHSM token should expose the test certificate without a PIN"
        );
        assert!(certs.iter().any(|c| c.subject.contains("OpenPDF Test")));
        with_locale("en", || {
            let desc = describe_certificate(&certs[0]);
            assert!(desc.contains("OpenPDF Test"));
            let _ = pin_prompt_text("openpdf-test");
        });
        let _ = CertificateUsage::Signing;
    }) else {
        return;
    };
}

#[test]
fn sign_and_verify_corpus_keeps_prefix() {
    let Some(()) = with_softhsm(|| {
        let signer = signer();
        let certs = signer.list_certificates().unwrap();
        let cert = certs.iter().find(|c| can_use(c)).unwrap_or(&certs[0]);
        let original = fs::read(corpus("hello.pdf")).unwrap();
        let request = SignRequest {
            cert_id: cert.id.clone(),
            reason: Some("openpdf-sign test".into()),
            location: Some("CI".into()),
            contact: None,
            appearance: None,
        };
        let signed = signer.sign(&original, &request).expect("sign");
        assert!(
            signed.starts_with(&original),
            "incremental update must leave earlier bytes unchanged"
        );
        assert!(signed.len() > original.len());

        let infos = signer.verify(&signed).expect("verify");
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].valid, Some(true));
        assert_eq!(infos[0].reason.as_deref(), Some("openpdf-sign test"));
        assert_eq!(infos[0].location.as_deref(), Some("CI"));
        assert!(
            infos[0]
                .name
                .as_deref()
                .is_some_and(|n| n.contains("OpenPDF Test")),
            "name: {:?}",
            infos[0].name
        );
        with_locale("en", || {
            let line = format_verify_result(&infos[0]);
            assert!(line.contains(&tr("Valid")), "{line}");
        });

        let mut doc = Document::from_bytes(signed.clone(), None).unwrap();
        assert!(doc.is_signed());

        let second = signer.sign(&signed, &request).expect("second sign");
        assert!(
            second.starts_with(&signed),
            "second signature must be incremental"
        );
        let infos = verify_pdf(&second).unwrap();
        assert_eq!(infos.len(), 2);
        assert!(infos.iter().all(|i| i.valid == Some(true)));

        doc.apply_signer(&signer, &request).unwrap();
        assert!(doc.is_signed());
    }) else {
        return;
    };
}

#[test]
fn sign_visible_appearance() {
    let Some(()) = with_softhsm(|| {
        let signer = signer();
        let cert = &signer.list_certificates().unwrap()[0];
        let original = fs::read(corpus("two_page.pdf")).unwrap();
        let request = SignRequest {
            cert_id: cert.id.clone(),
            reason: None,
            location: None,
            contact: None,
            appearance: Some(VisibleAppearance {
                page: 0,
                rect: [72.0, 72.0, 240.0, 120.0],
            }),
        };
        let signed = signer.sign(&original, &request).expect("sign visible");
        assert!(signed.starts_with(&original));
        let infos = signer.verify(&signed).unwrap();
        assert_eq!(infos[0].valid, Some(true));
    }) else {
        return;
    };
}

#[test]
fn unknown_cert_id_fails() {
    let Some(()) = with_softhsm(|| {
        let signer = signer();
        let pdf = fs::read(corpus("hello.pdf")).unwrap();
        let err = signer
            .sign(
                &pdf,
                &SignRequest {
                    cert_id: "no-such-cert".into(),
                    ..SignRequest::default()
                },
            )
            .unwrap_err();
        assert!(matches!(err, Error::Sign(_)));
    }) else {
        return;
    };
}

fn can_use(c: &openpdf_core::CertificateInfo) -> bool {
    c.unusable.is_none() && (c.usage.is_empty() || c.usage.contains(&CertificateUsage::Signing))
}

/// Downstream crates must keep a wildcard: [`CertUnusable`] is `#[non_exhaustive]`.
fn classify_unusable(reason: &CertUnusable) -> &'static str {
    match reason {
        CertUnusable::AuthenticationOnly => "auth",
        CertUnusable::Expired { .. } => "expired",
        CertUnusable::NotYetValid { .. } => "not-yet-valid",
        CertUnusable::UnsupportedKey => "key",
        _ => "other",
    }
}

struct MockRsaBackend {
    id: String,
    der: Vec<u8>,
    key: rsa::RsaPrivateKey,
}

impl KeyBackend for MockRsaBackend {
    fn list(&self) -> openpdf_core::Result<Vec<ListedCert>> {
        let cert = x509_cert::Certificate::from_der(&self.der).expect("mock cert");
        Ok(vec![ListedCert {
            id: self.id.clone(),
            der: self.der.clone(),
            info: crate::cert::info_from_cert(&cert, &self.der, self.id.clone()),
            kind: Some(KeyKind::Rsa),
        }])
    }

    fn sign_bytes(&self, cert_id: &str, data: &[u8]) -> openpdf_core::Result<Vec<u8>> {
        assert_eq!(cert_id, self.id);
        use rsa::signature::SignatureEncoding;
        let signing_key = rsa::pkcs1v15::SigningKey::<sha2::Sha256>::new(self.key.clone());
        Ok(rsa::signature::Signer::sign(&signing_key, data).to_vec())
    }
}

fn mock_rsa_backend() -> MockRsaBackend {
    mock_rsa_backend_with_validity(
        x509_cert::time::Validity::from_now(Duration::from_secs(365 * 86400)).expect("validity"),
    )
}

fn unix_time(secs: u64) -> x509_cert::time::Time {
    x509_cert::time::Time::try_from(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
        .expect("time")
}

fn mock_rsa_backend_with_validity(validity: x509_cert::time::Validity) -> MockRsaBackend {
    use der::asn1::BitString;
    use der::Encode;
    use rsa::pkcs8::EncodePublicKey;
    use rsa::signature::{SignatureEncoding, Signer};
    use rsa::{RsaPrivateKey, RsaPublicKey};
    use x509_cert::name::Name;
    use x509_cert::serial_number::SerialNumber;
    use x509_cert::spki::{AlgorithmIdentifierOwned, SubjectPublicKeyInfoOwned};
    use x509_cert::{Certificate, TbsCertificate, Version};

    let mut rng = rand::rngs::OsRng;
    let key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa key");
    let public = RsaPublicKey::from(&key);
    let spki_der = public.to_public_key_der().expect("spki");
    let spki = SubjectPublicKeyInfoOwned::from_der(spki_der.as_bytes()).expect("spki parse");
    let name: Name = "CN=OpenPDF Mock Signer".parse().expect("name");
    let sig_alg = AlgorithmIdentifierOwned {
        oid: const_oid::db::rfc5912::SHA_256_WITH_RSA_ENCRYPTION,
        parameters: Some(der::Any::null()),
    };
    let tbs = TbsCertificate {
        version: Version::V3,
        serial_number: SerialNumber::from(1u32),
        signature: sig_alg.clone(),
        issuer: name.clone(),
        validity,
        subject: name,
        subject_public_key_info: spki,
        issuer_unique_id: None,
        subject_unique_id: None,
        extensions: None,
    };
    let tbs_der = tbs.to_der().expect("tbs");
    let signing_key = rsa::pkcs1v15::SigningKey::<sha2::Sha256>::new(key.clone());
    let sig = signing_key.sign(&tbs_der);
    let cert = Certificate {
        tbs_certificate: tbs,
        signature_algorithm: sig_alg,
        signature: BitString::from_bytes(&sig.to_bytes()).expect("sig bits"),
    };
    MockRsaBackend {
        id: "mock".into(),
        der: cert.to_der().expect("cert der"),
        key,
    }
}

#[test]
fn mock_backend_shared_cms_roundtrip() {
    let backend = mock_rsa_backend();
    let listed = backend.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind, Some(KeyKind::Rsa));
    assert_eq!(listed[0].info.unusable, None);
    let message = b"openpdf-sign mock CMS message";
    let cms = cades::sign_detached(message, &listed[0].der, |tbs| {
        backend.sign_bytes(&listed[0].id, tbs)
    })
    .expect("cms");
    let check = cades::verify_detached(&cms, message);
    assert!(check.valid, "mock CMS must verify");
    assert!(
        check
            .signer
            .as_deref()
            .is_some_and(|s| s.contains("OpenPDF Mock")),
        "signer: {:?}",
        check.signer
    );
    let bad = cades::verify_detached(&cms, b"tampered");
    assert!(!bad.valid);
}

#[test]
fn mock_backend_pades_incremental_and_verify() {
    let backend = mock_rsa_backend();
    let original = fs::read(corpus("hello.pdf")).unwrap();
    let request = SignRequest {
        cert_id: "mock".into(),
        reason: Some("mock cms".into()),
        location: Some("unit".into()),
        contact: None,
        appearance: None,
    };
    let signed = sign_pdf(&backend, &original, &request).expect("sign");
    assert!(
        signed.starts_with(&original),
        "incremental update must leave earlier bytes unchanged"
    );
    let infos = verify_pdf(&signed).unwrap();
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].valid, Some(true));
    assert_eq!(infos[0].reason.as_deref(), Some("mock cms"));
}

#[test]
fn cert_unusable_match_needs_wildcard() {
    assert_eq!(
        classify_unusable(&CertUnusable::AuthenticationOnly),
        "auth"
    );
    assert_eq!(
        classify_unusable(&CertUnusable::Expired {
            not_after: "2020-01-01T00:00:00Z".into()
        }),
        "expired"
    );
    assert_eq!(
        classify_unusable(&CertUnusable::NotYetValid {
            not_before: "2030-01-01T00:00:00Z".into()
        }),
        "not-yet-valid"
    );
    assert_eq!(classify_unusable(&CertUnusable::UnsupportedKey), "key");
}

#[test]
fn sign_rejects_expired_certificate() {
    let backend = mock_rsa_backend_with_validity(x509_cert::time::Validity {
        not_before: unix_time(1_577_836_800),
        not_after: unix_time(1_609_459_200),
    });
    let listed = backend.list().unwrap();
    let reason = listed[0]
        .info
        .unusable
        .clone()
        .expect("expired cert must be marked unusable");
    assert!(matches!(reason, CertUnusable::Expired { .. }));
    assert_eq!(classify_unusable(&reason), "expired");
    with_locale("en", || {
        let original = fs::read(corpus("hello.pdf")).unwrap();
        let err = sign_pdf(
            &backend,
            &original,
            &SignRequest {
                cert_id: "mock".into(),
                ..SignRequest::default()
            },
        )
        .unwrap_err();
        match err {
            Error::Sign(msg) => assert_eq!(msg, describe_unusable(&reason)),
            other => panic!("expected Sign, got {other:?}"),
        }
    });
}

#[test]
fn sign_rejects_not_yet_valid_certificate() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let backend = mock_rsa_backend_with_validity(x509_cert::time::Validity {
        not_before: unix_time(now + 365 * 86400),
        not_after: unix_time(now + 2 * 365 * 86400),
    });
    let listed = backend.list().unwrap();
    let reason = listed[0]
        .info
        .unusable
        .clone()
        .expect("future cert must be marked unusable");
    assert!(matches!(reason, CertUnusable::NotYetValid { .. }));
    with_locale("en", || {
        let original = fs::read(corpus("hello.pdf")).unwrap();
        let err = sign_pdf(
            &backend,
            &original,
            &SignRequest {
                cert_id: "mock".into(),
                ..SignRequest::default()
            },
        )
        .unwrap_err();
        match err {
            Error::Sign(msg) => assert_eq!(msg, describe_unusable(&reason)),
            other => panic!("expected Sign, got {other:?}"),
        }
    });
}

#[test]
fn unusable_reason_none_for_fresh_rsa() {
    let backend = mock_rsa_backend();
    let cert = x509_cert::Certificate::from_der(&backend.der).unwrap();
    assert_eq!(unusable_reason(&cert), None);
}

struct MockEcdsaBackend {
    id: String,
    der: Vec<u8>,
    key: p256::ecdsa::SigningKey,
}

impl KeyBackend for MockEcdsaBackend {
    fn list(&self) -> openpdf_core::Result<Vec<ListedCert>> {
        let cert = x509_cert::Certificate::from_der(&self.der).expect("mock cert");
        Ok(vec![ListedCert {
            id: self.id.clone(),
            der: self.der.clone(),
            info: crate::cert::info_from_cert(&cert, &self.der, self.id.clone()),
            kind: Some(KeyKind::Ecdsa {
                hash: EcdsaHash::Sha256,
            }),
        }])
    }

    fn sign_bytes(&self, cert_id: &str, data: &[u8]) -> openpdf_core::Result<Vec<u8>> {
        assert_eq!(cert_id, self.id);
        use p256::ecdsa::signature::Signer;
        let sig: p256::ecdsa::Signature = self.key.sign(data);
        Ok(sig.to_der().as_bytes().to_vec())
    }
}

fn mock_ecdsa_p256_backend() -> MockEcdsaBackend {
    use der::asn1::BitString;
    use der::Encode;
    use p256::ecdsa::signature::Signer;
    use x509_cert::name::Name;
    use x509_cert::serial_number::SerialNumber;
    use x509_cert::spki::{AlgorithmIdentifierOwned, SubjectPublicKeyInfoOwned};
    use x509_cert::time::Validity;
    use x509_cert::{Certificate, TbsCertificate, Version};

    let key = p256::ecdsa::SigningKey::from_slice(&[0x42u8; 32]).expect("p256 key");
    let point = key.verifying_key().to_encoded_point(false);
    let spki = SubjectPublicKeyInfoOwned {
        algorithm: AlgorithmIdentifierOwned {
            oid: const_oid::ObjectIdentifier::new_unwrap("1.2.840.10045.2.1"),
            parameters: Some(der::Any::from(&const_oid::ObjectIdentifier::new_unwrap(
                "1.2.840.10045.3.1.7",
            ))),
        },
        subject_public_key: BitString::from_bytes(point.as_bytes()).expect("sec1"),
    };
    let name: Name = "CN=OpenPDF ECDSA Mock".parse().expect("name");
    let validity = Validity::from_now(Duration::from_secs(365 * 86400)).expect("validity");
    let sig_alg = AlgorithmIdentifierOwned {
        oid: const_oid::ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2"),
        parameters: None,
    };
    let tbs = TbsCertificate {
        version: Version::V3,
        serial_number: SerialNumber::from(1u32),
        signature: sig_alg.clone(),
        issuer: name.clone(),
        validity,
        subject: name,
        subject_public_key_info: spki,
        issuer_unique_id: None,
        subject_unique_id: None,
        extensions: None,
    };
    let tbs_der = tbs.to_der().expect("tbs");
    let sig: p256::ecdsa::Signature = key.sign(&tbs_der);
    let cert = Certificate {
        tbs_certificate: tbs,
        signature_algorithm: sig_alg,
        signature: BitString::from_bytes(sig.to_der().as_bytes()).expect("sig bits"),
    };
    MockEcdsaBackend {
        id: "mock-ecdsa".into(),
        der: cert.to_der().expect("cert der"),
        key,
    }
}

#[test]
fn mock_ecdsa_p256_cms_roundtrip() {
    let backend = mock_ecdsa_p256_backend();
    let listed = backend.list().unwrap();
    assert_eq!(
        listed[0].kind,
        Some(KeyKind::Ecdsa {
            hash: EcdsaHash::Sha256
        })
    );
    assert_eq!(listed[0].info.unusable, None);
    let message = b"openpdf-sign mock ECDSA CMS message";
    let cms = cades::sign_detached(message, &listed[0].der, |tbs| {
        backend.sign_bytes(&listed[0].id, tbs)
    })
    .expect("cms");
    let check = cades::verify_detached(&cms, message);
    assert!(check.valid, "mock ECDSA CMS must verify");
    assert!(
        check
            .signer
            .as_deref()
            .is_some_and(|s| s.contains("OpenPDF ECDSA")),
        "signer: {:?}",
        check.signer
    );
}

#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn native_test_cert<'a>(
    certs: &'a [openpdf_core::CertificateInfo],
) -> Option<&'a openpdf_core::CertificateInfo> {
    certs.iter().find(|c| {
        c.unusable.is_none() && c.subject.contains("OpenPDF Test Signer")
    })
}

#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn native_sign_and_verify(signer: &dyn Signer) {
    let certs = signer.list_certificates().expect("list native certs");
    let Some(cert) = native_test_cert(&certs) else {
        if std::env::var_os("CI").is_some() {
            panic!("CI must provision CN=OpenPDF Test Signer in the native store");
        }
        eprintln!("skipping native store sign test: no OpenPDF Test Signer certificate");
        return;
    };
    let original = fs::read(corpus("hello.pdf")).unwrap();
    let request = SignRequest {
        cert_id: cert.id.clone(),
        reason: Some("native store test".into()),
        location: Some("CI".into()),
        contact: None,
        appearance: None,
    };
    let signed = signer.sign(&original, &request).expect("native sign");
    assert!(signed.starts_with(&original));
    let infos = signer.verify(&signed).expect("native verify");
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].valid, Some(true));
    assert_eq!(infos[0].reason.as_deref(), Some("native store test"));
}

#[cfg(windows)]
#[test]
fn windows_cng_sign_and_verify() {
    let signer = crate::WindowsStoreSigner::new().expect("Windows MY store");
    native_sign_and_verify(&signer);
    let combined = default_signer(StaticPinProvider::new("unused")).expect("default_signer");
    assert!(
        combined
            .list_certificates()
            .unwrap()
            .iter()
            .any(|c| c.id.starts_with("cng:") && c.subject.contains("OpenPDF Test Signer"))
            || std::env::var_os("CI").is_none()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_keychain_sign_and_verify() {
    let signer = crate::KeychainSigner::new();
    native_sign_and_verify(&signer);
    let combined = default_signer(StaticPinProvider::new("unused")).expect("default_signer");
    let certs = combined.list_certificates().unwrap();
    if native_test_cert(&certs).is_none() && std::env::var_os("CI").is_some() {
        panic!("default_signer did not list the Keychain test identity");
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
#[test]
fn default_signer_is_pkcs11_on_linux() {
    let Some(()) = with_softhsm(|| {
        let env = test_env();
        let prev = std::env::var_os("OPENPDF_PKCS11_MODULE");
        std::env::set_var("OPENPDF_PKCS11_MODULE", &env.module);
        let signer = default_signer(StaticPinProvider::new(USER_PIN)).expect("default_signer");
        let certs = signer.list_certificates().expect("list");
        match prev {
            Some(v) => std::env::set_var("OPENPDF_PKCS11_MODULE", v),
            None => std::env::remove_var("OPENPDF_PKCS11_MODULE"),
        }
        assert!(certs.iter().any(|c| c.subject.contains("OpenPDF Test")));
    }) else {
        return;
    };
}
