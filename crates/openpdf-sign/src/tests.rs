//! SoftHSM2 integration plus PIN / verify unit tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::error::{Error as CkError, RvError};
use cryptoki::mechanism::Mechanism;
use cryptoki::object::{Attribute, AttributeType, CertificateType, KeyType, ObjectClass};
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
use sha2::Digest;
use x509_cert::name::Name;
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::{AlgorithmIdentifierOwned, SubjectPublicKeyInfoOwned};
use x509_cert::time::Validity;
use x509_cert::{Certificate, TbsCertificate, Version};

use crate::backend::{sign_pdf, KeyBackend, ListedCert};
use crate::cades;
use crate::cert::{unusable_reason, EcdsaHash, KeyKind};
use crate::pin::StaticPinProvider;
use crate::token::{Pkcs11Backend, Pkcs11Config, Pkcs11Signer};
use crate::{default_signer, verify_pdf, CertificateUsage};

const USER_PIN: &str = "1234";
const SO_PIN: &str = "12345678";
const KEY_ID: &[u8] = &[0x01];
const EC_KEY_ID: &[u8] = &[0x02];
const SECP256R1_OID_DER: &[u8] = &[0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07];

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

    provision_ec_p256(&session);
}

fn unwrap_ec_point(bytes: &[u8]) -> Vec<u8> {
    if let Ok(os) = der::asn1::OctetString::from_der(bytes) {
        let inner = os.as_bytes();
        if let Ok(os2) = der::asn1::OctetString::from_der(inner) {
            return os2.as_bytes().to_vec();
        }
        return inner.to_vec();
    }
    bytes.to_vec()
}

fn provision_ec_p256(session: &cryptoki::session::Session) {
    let pub_tmpl = [
        Attribute::Token(true),
        Attribute::Private(false),
        Attribute::Verify(true),
        Attribute::KeyType(KeyType::EC),
        Attribute::EcParams(SECP256R1_OID_DER.to_vec()),
        Attribute::Id(EC_KEY_ID.to_vec()),
        Attribute::Label(b"openpdf-ec".to_vec()),
    ];
    let priv_tmpl = [
        Attribute::Token(true),
        Attribute::Private(true),
        Attribute::Sensitive(true),
        Attribute::Sign(true),
        Attribute::KeyType(KeyType::EC),
        Attribute::Id(EC_KEY_ID.to_vec()),
        Attribute::Label(b"openpdf-ec".to_vec()),
    ];
    let (public, private) = session
        .generate_key_pair(&Mechanism::EccKeyPairGen, &pub_tmpl, &priv_tmpl)
        .expect("ec keypair");

    let attrs = session
        .get_attributes(public, &[AttributeType::EcPoint])
        .expect("ec point");
    let mut point = None;
    for a in attrs {
        if let Attribute::EcPoint(v) = a {
            point = Some(v);
        }
    }
    let sec1 = unwrap_ec_point(&point.expect("CKA_EC_POINT"));
    let spki = SubjectPublicKeyInfoOwned {
        algorithm: AlgorithmIdentifierOwned {
            oid: const_oid::ObjectIdentifier::new_unwrap("1.2.840.10045.2.1"),
            parameters: Some(der::Any::from(&const_oid::ObjectIdentifier::new_unwrap(
                "1.2.840.10045.3.1.7",
            ))),
        },
        subject_public_key: BitString::from_bytes(&sec1).expect("sec1"),
    };
    let name: Name = "CN=OpenPDF EC Test".parse().expect("name");
    let validity = Validity::from_now(Duration::from_secs(365 * 86400)).expect("validity");
    let sig_alg = AlgorithmIdentifierOwned {
        oid: const_oid::ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2"),
        parameters: None,
    };
    let tbs = TbsCertificate {
        version: Version::V3,
        serial_number: SerialNumber::from(2u32),
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
    let digest = sha2::Sha256::digest(&tbs_der);
    let raw = session
        .sign(&Mechanism::Ecdsa, private, digest.as_slice())
        .expect("sign ec tbs");
    let sig_der = cades::ecdsa_p1363_to_der(&raw, 32).expect("p1363");
    let cert = Certificate {
        tbs_certificate: tbs,
        signature_algorithm: sig_alg,
        signature: BitString::from_bytes(&sig_der).expect("sig bits"),
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
            Attribute::Label(b"openpdf-ec".to_vec()),
            Attribute::Id(EC_KEY_ID.to_vec()),
            Attribute::Subject(subject_der),
            Attribute::Issuer(issuer_der),
            Attribute::SerialNumber(serial_der),
            Attribute::Value(cert_der),
        ])
        .expect("store ec cert");
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

struct EmptyPin;

impl PinProvider for EmptyPin {
    fn request_pin(&self, _token_label: &str) -> openpdf_core::Result<SecretPin> {
        Ok(SecretPin::new(""))
    }
}

struct CancelPin;

impl PinProvider for CancelPin {
    fn request_pin(&self, _token_label: &str) -> openpdf_core::Result<SecretPin> {
        Err(Error::PinCancelled)
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
        assert!(certs
            .iter()
            .any(|c| c.subject.contains("OpenPDF Test Signer")));
        assert!(certs.iter().any(|c| c.subject.contains("OpenPDF EC Test")));
        let pin = certs
            .iter()
            .find(|c| c.subject.contains("OpenPDF Test Signer"))
            .and_then(|c| c.pin_status);
        let pin = pin.expect("PKCS#11 listing fills token PIN flags");
        assert!(!pin.locked);
        assert!(!pin.protected_authentication_path);
        with_locale("en", || {
            let rsa = certs
                .iter()
                .find(|c| c.subject.contains("OpenPDF Test Signer"))
                .unwrap();
            let desc = describe_certificate(rsa);
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
        let cert = certs
            .iter()
            .find(|c| can_use(c) && c.subject.contains("OpenPDF Test Signer"))
            .expect("RSA SoftHSM test cert");
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
        let certs = signer.list_certificates().unwrap();
        let cert = certs
            .iter()
            .find(|c| can_use(c))
            .expect("usable SoftHSM cert");
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
fn empty_pin_never_decrements_softhsm_retry() {
    let Some(()) = with_softhsm(|| {
        let env = test_env();
        let empty = Pkcs11Signer::new(Pkcs11Config::with_module(&env.module), EmptyPin)
            .expect("empty-pin signer");
        let certs = empty.list_certificates().unwrap();
        let cert = certs
            .iter()
            .find(|c| can_use(c) && c.subject.contains("OpenPDF Test Signer"))
            .expect("RSA SoftHSM test cert");
        let before = cert.pin_status;
        let cert_id = cert.id.clone();
        let pdf = fs::read(corpus("hello.pdf")).unwrap();
        let err = empty
            .sign(
                &pdf,
                &SignRequest {
                    cert_id: cert_id.clone(),
                    ..SignRequest::default()
                },
            )
            .unwrap_err();
        assert!(
            matches!(err, Error::PinRequired),
            "empty PIN must not call C_Login: {err:?}"
        );
        let after = empty
            .list_certificates()
            .unwrap()
            .into_iter()
            .find(|c| c.id == cert_id)
            .and_then(|c| c.pin_status);
        assert_eq!(
            before, after,
            "empty PIN must not change CKF_USER_PIN_* flags"
        );
        let ok = signer();
        let signed = ok
            .sign(
                &pdf,
                &SignRequest {
                    cert_id,
                    reason: Some("empty-pin guard".into()),
                    location: None,
                    contact: None,
                    appearance: None,
                },
            )
            .expect("correct PIN still works after empty PIN was refused");
        assert!(signed.starts_with(&pdf));
    }) else {
        return;
    };
}

#[test]
fn cancelled_pin_never_touches_token() {
    let Some(()) = with_softhsm(|| {
        let env = test_env();
        let cancel = Pkcs11Signer::new(Pkcs11Config::with_module(&env.module), CancelPin)
            .expect("cancel signer");
        let certs = cancel.list_certificates().unwrap();
        let cert = certs
            .iter()
            .find(|c| can_use(c) && c.subject.contains("OpenPDF Test Signer"))
            .expect("RSA SoftHSM test cert");
        let before = cert.pin_status;
        let cert_id = cert.id.clone();
        let pdf = fs::read(corpus("hello.pdf")).unwrap();
        let err = cancel
            .sign(
                &pdf,
                &SignRequest {
                    cert_id: cert_id.clone(),
                    ..SignRequest::default()
                },
            )
            .unwrap_err();
        assert!(matches!(err, Error::PinCancelled), "{err:?}");
        let after = cancel
            .list_certificates()
            .unwrap()
            .into_iter()
            .find(|c| c.id == cert_id)
            .and_then(|c| c.pin_status);
        assert_eq!(before, after);
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

/// Two C_Sign calls on one session. The second C_Login is
/// `CKR_USER_ALREADY_LOGGED_IN` and must be treated as success.
#[test]
fn two_signatures_one_session_already_logged_in() {
    let Some(()) = with_softhsm(|| {
        let env = test_env();
        let pins = Arc::new(AtomicUsize::new(0));
        let pin = CountPin {
            count: pins.clone(),
            pin: USER_PIN,
        };
        let backend = Pkcs11Backend::new(Pkcs11Config::with_module(&env.module), Arc::new(pin))
            .expect("backend");
        let listed = backend.list().expect("list");
        let cert = listed
            .iter()
            .find(|c| can_use(&c.info) && c.info.subject.contains("OpenPDF Test Signer"))
            .expect("RSA SoftHSM test cert");
        let der = cert.der.clone();
        let name = cert.info.subject.clone();
        let request = SignRequest {
            cert_id: cert.id.clone(),
            reason: Some("already-logged-in".into()),
            location: None,
            contact: None,
            appearance: None,
        };
        let live = backend.open_live_session(&cert.id).expect("one session");
        let pdf = fs::read(corpus("hello.pdf")).unwrap();
        let first = crate::pades::sign_incremental(&pdf, &request, &name, |message| {
            cades::sign_detached(message, &der, |tbs| live.sign_bytes(tbs))
        })
        .expect("first sign on live session");
        assert!(first.starts_with(&pdf));
        let first_infos = verify_pdf(&first).expect("verify first");
        assert_eq!(first_infos.len(), 1, "{first_infos:?}");
        assert_eq!(first_infos[0].valid, Some(true), "{first_infos:?}");
        let second = crate::pades::sign_incremental(&first, &request, &name, |message| {
            cades::sign_detached(message, &der, |tbs| live.sign_bytes(tbs))
        })
        .expect("second sign must accept CKR_USER_ALREADY_LOGGED_IN");
        assert!(second.starts_with(&first));
        let infos = verify_pdf(&second).expect("verify");
        assert_eq!(infos.len(), 2, "{infos:?}");
        assert!(infos.iter().all(|i| i.valid == Some(true)), "{infos:?}");
        assert_eq!(
            pins.load(Ordering::SeqCst),
            2,
            "login_user runs twice; the second C_Login is already-logged-in"
        );
    }) else {
        return;
    };
}

struct CountPin {
    count: Arc<AtomicUsize>,
    pin: &'static str,
}

impl PinProvider for CountPin {
    fn request_pin(&self, _token_label: &str) -> openpdf_core::Result<SecretPin> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(SecretPin::new(self.pin))
    }
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
    mock_rsa_backend_with(
        x509_cert::time::Validity::from_now(Duration::from_secs(365 * 86400)).expect("validity"),
        None,
    )
}

fn mock_rsa_backend_with_validity(validity: x509_cert::time::Validity) -> MockRsaBackend {
    mock_rsa_backend_with(validity, None)
}

fn key_usage_ext(digital_signature: bool, non_repudiation: bool) -> x509_cert::ext::Extension {
    use const_oid::AssociatedOid;
    use der::Encode;
    use x509_cert::ext::pkix::{KeyUsage, KeyUsages};
    let flags = match (digital_signature, non_repudiation) {
        (true, true) => KeyUsages::DigitalSignature | KeyUsages::NonRepudiation,
        (true, false) => KeyUsages::DigitalSignature.into(),
        (false, true) => KeyUsages::NonRepudiation.into(),
        (false, false) => KeyUsages::DigitalSignature.into(),
    };
    let ku = KeyUsage(flags);
    x509_cert::ext::Extension {
        extn_id: KeyUsage::OID,
        critical: true,
        extn_value: der::asn1::OctetString::new(ku.to_der().expect("ku der")).expect("octet"),
    }
}

fn unix_time(secs: u64) -> x509_cert::time::Time {
    x509_cert::time::Time::try_from(std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
        .expect("time")
}

fn mock_rsa_backend_with(
    validity: x509_cert::time::Validity,
    extensions: Option<Vec<x509_cert::ext::Extension>>,
) -> MockRsaBackend {
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
        extensions,
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
    assert_eq!(classify_unusable(&CertUnusable::AuthenticationOnly), "auth");
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
    kind: KeyKind,
    sign: Box<dyn Fn(&[u8]) -> Vec<u8> + Send + Sync>,
}

impl KeyBackend for MockEcdsaBackend {
    fn list(&self) -> openpdf_core::Result<Vec<ListedCert>> {
        let cert = x509_cert::Certificate::from_der(&self.der).expect("mock cert");
        Ok(vec![ListedCert {
            id: self.id.clone(),
            der: self.der.clone(),
            info: crate::cert::info_from_cert(&cert, &self.der, self.id.clone()),
            kind: Some(self.kind),
        }])
    }

    fn sign_bytes(&self, cert_id: &str, data: &[u8]) -> openpdf_core::Result<Vec<u8>> {
        assert_eq!(cert_id, self.id);
        Ok((self.sign)(data))
    }
}

fn mock_ec_cert(
    cn: &str,
    curve_oid: &str,
    sig_oid: &str,
    sec1: &[u8],
    sign_tbs: impl FnOnce(&[u8]) -> Vec<u8>,
) -> Vec<u8> {
    use der::asn1::BitString;
    use der::Encode;
    use x509_cert::name::Name;
    use x509_cert::serial_number::SerialNumber;
    use x509_cert::spki::{AlgorithmIdentifierOwned, SubjectPublicKeyInfoOwned};
    use x509_cert::time::Validity;
    use x509_cert::{Certificate, TbsCertificate, Version};

    let spki = SubjectPublicKeyInfoOwned {
        algorithm: AlgorithmIdentifierOwned {
            oid: const_oid::ObjectIdentifier::new_unwrap("1.2.840.10045.2.1"),
            parameters: Some(der::Any::from(&const_oid::ObjectIdentifier::new_unwrap(
                curve_oid,
            ))),
        },
        subject_public_key: BitString::from_bytes(sec1).expect("sec1"),
    };
    let name: Name = cn.parse().expect("name");
    let validity = Validity::from_now(Duration::from_secs(365 * 86400)).expect("validity");
    let sig_alg = AlgorithmIdentifierOwned {
        oid: const_oid::ObjectIdentifier::new_unwrap(sig_oid),
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
    let sig = sign_tbs(&tbs_der);
    let cert = Certificate {
        tbs_certificate: tbs,
        signature_algorithm: sig_alg,
        signature: BitString::from_bytes(&sig).expect("sig bits"),
    };
    cert.to_der().expect("cert der")
}

fn mock_ecdsa_p256_backend() -> MockEcdsaBackend {
    use p256::ecdsa::signature::Signer;
    let key = p256::ecdsa::SigningKey::from_slice(&[0x42u8; 32]).expect("p256 key");
    let point = key.verifying_key().to_encoded_point(false);
    let key_tbs = key.clone();
    let der = mock_ec_cert(
        "CN=OpenPDF ECDSA P-256",
        "1.2.840.10045.3.1.7",
        "1.2.840.10045.4.3.2",
        point.as_bytes(),
        move |tbs| {
            let sig: p256::ecdsa::Signature = key_tbs.sign(tbs);
            sig.to_der().as_bytes().to_vec()
        },
    );
    MockEcdsaBackend {
        id: "mock-ecdsa-p256".into(),
        der,
        kind: KeyKind::Ecdsa {
            hash: EcdsaHash::Sha256,
        },
        sign: Box::new(move |data| {
            let sig: p256::ecdsa::Signature = key.sign(data);
            sig.to_der().as_bytes().to_vec()
        }),
    }
}

fn mock_ecdsa_p384_backend() -> MockEcdsaBackend {
    use p384::ecdsa::signature::Signer;
    let key = p384::ecdsa::SigningKey::from_slice(&[0x42u8; 48]).expect("p384 key");
    let point = key.verifying_key().to_encoded_point(false);
    let key_tbs = key.clone();
    let der = mock_ec_cert(
        "CN=OpenPDF ECDSA P-384",
        "1.3.132.0.34",
        "1.2.840.10045.4.3.3",
        point.as_bytes(),
        move |tbs| {
            let sig: p384::ecdsa::Signature = key_tbs.sign(tbs);
            sig.to_der().as_bytes().to_vec()
        },
    );
    MockEcdsaBackend {
        id: "mock-ecdsa-p384".into(),
        der,
        kind: KeyKind::Ecdsa {
            hash: EcdsaHash::Sha384,
        },
        sign: Box::new(move |data| {
            let sig: p384::ecdsa::Signature = key.sign(data);
            sig.to_der().as_bytes().to_vec()
        }),
    }
}

fn mock_ecdsa_p521_backend() -> MockEcdsaBackend {
    use p521::ecdsa::signature::Signer;
    let mut p521_scalar = [0u8; 66];
    p521_scalar[65] = 0x42;
    let key = p521::ecdsa::SigningKey::from_slice(&p521_scalar).expect("p521 key");
    let point = p521::ecdsa::VerifyingKey::from(&key).to_encoded_point(false);
    let key_tbs = key.clone();
    let der = mock_ec_cert(
        "CN=OpenPDF ECDSA P-521",
        "1.3.132.0.35",
        "1.2.840.10045.4.3.4",
        point.as_bytes(),
        move |tbs| {
            let sig: p521::ecdsa::Signature = key_tbs.sign(tbs);
            sig.to_der().as_bytes().to_vec()
        },
    );
    MockEcdsaBackend {
        id: "mock-ecdsa-p521".into(),
        der,
        kind: KeyKind::Ecdsa {
            hash: EcdsaHash::Sha512,
        },
        sign: Box::new(move |data| {
            let sig: p521::ecdsa::Signature = key.sign(data);
            sig.to_der().as_bytes().to_vec()
        }),
    }
}

fn assert_ecdsa_cms_roundtrip(backend: &MockEcdsaBackend, hash: EcdsaHash, cn_part: &str) {
    let listed = backend.list().unwrap();
    assert_eq!(listed[0].kind, Some(KeyKind::Ecdsa { hash }));
    assert_eq!(listed[0].info.unusable, None);
    let message = b"openpdf-sign ECDSA CMS roundtrip";
    let cms = cades::sign_detached(message, &listed[0].der, |tbs| {
        backend.sign_bytes(&listed[0].id, tbs)
    })
    .expect("cms");
    let check = cades::verify_detached(&cms, message);
    assert!(check.valid, "ECDSA {cn_part} CMS must verify");
    assert!(
        check.signer.as_deref().is_some_and(|s| s.contains(cn_part)),
        "signer: {:?}",
        check.signer
    );
    assert!(!cades::verify_detached(&cms, b"tampered").valid);
}

#[test]
fn mock_ecdsa_p256_cms_roundtrip() {
    assert_ecdsa_cms_roundtrip(
        &mock_ecdsa_p256_backend(),
        EcdsaHash::Sha256,
        "OpenPDF ECDSA P-256",
    );
}

#[test]
fn mock_ecdsa_p384_cms_roundtrip() {
    assert_ecdsa_cms_roundtrip(
        &mock_ecdsa_p384_backend(),
        EcdsaHash::Sha384,
        "OpenPDF ECDSA P-384",
    );
}

#[test]
fn mock_ecdsa_p521_cms_roundtrip() {
    assert_ecdsa_cms_roundtrip(
        &mock_ecdsa_p521_backend(),
        EcdsaHash::Sha512,
        "OpenPDF ECDSA P-521",
    );
}

#[test]
fn ecdsa_p1363_to_der_all_field_sizes() {
    use p256::ecdsa::signature::Signer;
    let p256 = p256::ecdsa::SigningKey::from_slice(&[0x42u8; 32]).unwrap();
    let msg = b"p1363 field sizes";
    let sig: p256::ecdsa::Signature = p256.sign(msg);
    let raw = sig.to_bytes();
    assert_eq!(raw.len(), 64);
    let der = cades::ecdsa_p1363_to_der(&raw, 32).expect("p256 p1363");
    assert_eq!(
        p256::ecdsa::Signature::from_der(&der).expect("parse p256"),
        sig
    );

    let p384 = p384::ecdsa::SigningKey::from_slice(&[0x42u8; 48]).unwrap();
    let sig: p384::ecdsa::Signature = p384.sign(msg);
    let raw = sig.to_bytes();
    assert_eq!(raw.len(), 96);
    let der = cades::ecdsa_p1363_to_der(&raw, 48).expect("p384 p1363");
    assert_eq!(
        p384::ecdsa::Signature::from_der(&der).expect("parse p384"),
        sig
    );

    let mut p521_scalar = [0u8; 66];
    p521_scalar[65] = 0x42;
    let p521 = p521::ecdsa::SigningKey::from_slice(&p521_scalar).unwrap();
    let sig: p521::ecdsa::Signature = p521.sign(msg);
    let raw = sig.to_bytes();
    assert_eq!(raw.len(), 132);
    let der = cades::ecdsa_p1363_to_der(&raw, 66).expect("p521 p1363");
    assert_eq!(
        p521::ecdsa::Signature::from_der(&der).expect("parse p521"),
        sig
    );

    assert!(cades::ecdsa_p1363_to_der(&[0u8; 10], 32).is_err());
}

#[test]
fn unknown_curve_is_hard_error() {
    let point = p256::ecdsa::SigningKey::from_slice(&[0x42u8; 32])
        .unwrap()
        .verifying_key()
        .to_encoded_point(false);
    // secp256k1 — not a PAdES curve we support.
    let der = mock_ec_cert(
        "CN=OpenPDF Unknown Curve",
        "1.3.132.0.10",
        "1.2.840.10045.4.3.2",
        point.as_bytes(),
        |_| vec![0u8; 64],
    );
    let cert = x509_cert::Certificate::from_der(&der).unwrap();
    assert_eq!(crate::cert::key_kind(&cert), None);
    assert_eq!(unusable_reason(&cert), Some(CertUnusable::UnsupportedKey));
    let err = cades::sign_detached(b"msg", &der, |_| Ok(vec![0u8; 64])).unwrap_err();
    match err {
        Error::Sign(msg) => assert!(
            msg.contains("unsupported") || msg.contains("Unsupported"),
            "{msg}"
        ),
        other => panic!("expected Sign, got {other:?}"),
    }
}

#[test]
fn sign_rejects_authentication_only_certificate() {
    let backend = mock_rsa_backend_with(
        x509_cert::time::Validity::from_now(Duration::from_secs(365 * 86400)).expect("validity"),
        Some(vec![key_usage_ext(true, false)]),
    );
    let listed = backend.list().unwrap();
    assert_eq!(listed[0].info.usage, vec![CertificateUsage::Authentication]);
    assert_eq!(
        listed[0].info.unusable,
        Some(CertUnusable::AuthenticationOnly)
    );
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
            Error::Sign(msg) => {
                assert_eq!(msg, describe_unusable(&CertUnusable::AuthenticationOnly))
            }
            other => panic!("expected Sign, got {other:?}"),
        }
    });
}

#[test]
fn softhsm_ecdsa_p256_sign_and_verify() {
    let Some(()) = with_softhsm(|| {
        let signer = signer();
        let certs = signer.list_certificates().unwrap();
        let cert = certs
            .iter()
            .find(|c| c.subject.contains("OpenPDF EC Test") && c.unusable.is_none())
            .expect("SoftHSM must expose the EC test certificate");
        let original = fs::read(corpus("hello.pdf")).unwrap();
        let request = SignRequest {
            cert_id: cert.id.clone(),
            reason: Some("softhsm ecdsa".into()),
            location: Some("CI".into()),
            contact: None,
            appearance: None,
        };
        let signed = signer.sign(&original, &request).expect("sign ec");
        assert!(signed.starts_with(&original));
        let infos = signer.verify(&signed).expect("verify ec");
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].valid, Some(true));
        assert_eq!(infos[0].reason.as_deref(), Some("softhsm ecdsa"));
    }) else {
        return;
    };
}

#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn native_test_cert<'a>(
    certs: &'a [openpdf_core::CertificateInfo],
) -> Option<&'a openpdf_core::CertificateInfo> {
    certs
        .iter()
        .find(|c| c.unusable.is_none() && c.subject.contains("OpenPDF Test Signer"))
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
