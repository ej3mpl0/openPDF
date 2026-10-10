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
use der::{Encode, Sequence};
use openpdf_core::test_support::with_locale;
use openpdf_core::{
    describe_certificate, format_verify_result, format_verify_results, pin_prompt_text, tr,
    Document, Error, PadesLevel, PinProvider, SecretPin, SignRequest, Signer, VisibleAppearance,
};
use x509_cert::name::Name;
use x509_cert::serial_number::SerialNumber;
use x509_cert::spki::{AlgorithmIdentifierOwned, SubjectPublicKeyInfoOwned};
use x509_cert::time::Validity;
use x509_cert::{Certificate, TbsCertificate, Version};

use crate::pin::StaticPinProvider;
use crate::token::{Pkcs11Config, Pkcs11Signer};
use crate::{verify_pdf, CertificateUsage};

pub(crate) const USER_PIN: &str = "1234";
const SO_PIN: &str = "12345678";
const KEY_ID: &[u8] = &[0x01];

pub(crate) struct TestEnv {
    pub(crate) module: PathBuf,
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
        None if std::env::var_os("CI").is_some() => {
            panic!("SoftHSM2 is required in CI (apt install softhsm2)")
        }
        None => panic!("skip-not-soft: no libsofthsm2.so"),
    }
}

pub(crate) fn test_env() -> &'static TestEnv {
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

pub(crate) fn corpus(name: &str) -> PathBuf {
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
pub(crate) fn with_softhsm<T>(f: impl FnOnce() -> T + std::panic::UnwindSafe) -> Option<T> {
    static PKCS11: Mutex<()> = Mutex::new(());
    catch_skip(|| {
        let _g = PKCS11.lock().unwrap_or_else(|e| e.into_inner());
        f()
    })
}

pub(crate) struct PanicPin;

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
        let request = SignRequest::new(cert.id.clone())
            .with_reason("openpdf-sign test")
            .with_location("CI");
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
        let request = SignRequest::new(cert.id.clone()).with_appearance(VisibleAppearance {
            page: 0,
            rect: [72.0, 72.0, 240.0, 120.0],
        });
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
            .sign(&pdf, &SignRequest::new("no-such-cert"))
            .unwrap_err();
        assert!(matches!(err, Error::Sign(_)));
    }) else {
        return;
    };
}

fn can_use(c: &openpdf_core::CertificateInfo) -> bool {
    c.usage.is_empty() || c.usage.contains(&CertificateUsage::Signing)
}

#[test]
fn reason_and_location_written_and_read_back() {
    let Some(()) = with_softhsm(|| {
        let signer = signer();
        let certs = signer.list_certificates().unwrap();
        let cert = certs.iter().find(|c| can_use(c)).unwrap_or(&certs[0]);
        let original = fs::read(corpus("hello.pdf")).unwrap();
        let request = SignRequest::new(cert.id.clone())
            .with_reason("Approved by QA")
            .with_location("Madrid");
        let signed = signer.sign(&original, &request).expect("sign");
        let tail = &signed[original.len()..];
        let has = |needle: &[u8]| tail.windows(needle.len()).any(|w| w == needle);
        assert!(has(b"/Reason (Approved by QA)"));
        assert!(has(b"/Location (Madrid)"));
        let infos = verify_pdf(&signed).unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].valid, Some(true));
        assert_eq!(infos[0].reason.as_deref(), Some("Approved by QA"));
        assert_eq!(infos[0].location.as_deref(), Some("Madrid"));
    }) else {
        return;
    };
}

#[test]
fn pades_bt_without_tsa_url_is_a_typed_error_not_a_downgrade() {
    let pdf = fs::read(corpus("hello.pdf")).unwrap();
    let request = SignRequest::new("any").with_level(PadesLevel::BT);
    let mut called = false;
    let err = crate::pades::sign_incremental(&pdf, &request, "x", None, |_| {
        called = true;
        Ok(Vec::new())
    })
    .unwrap_err();
    assert!(matches!(
        err,
        Error::Timestamp(openpdf_core::TimestampErrorKind::UrlMissing)
    ));
    assert!(!called, "no CMS may be produced without a TSA");
    with_locale("en", || {
        assert_eq!(
            err.to_string(),
            "timestamp: PAdES-B-T needs a timestamp server (TSA) URL; none is configured"
        );
    });
    with_locale("es", || {
        assert_ne!(
            err.to_string(),
            "timestamp: PAdES-B-T needs a timestamp server (TSA) URL; none is configured"
        );
    });

    // Through the PKCS#11 signer: refused before the PIN prompt.
    let Some(()) = with_softhsm(|| {
        let env = test_env();
        let signer =
            Pkcs11Signer::new(Pkcs11Config::with_module(&env.module), PanicPin).expect("signer");
        let cert = &signer.list_certificates().unwrap()[0];
        let request = SignRequest::new(cert.id.clone()).with_level(PadesLevel::BT);
        assert!(matches!(
            signer.sign(&pdf, &request),
            Err(Error::Timestamp(
                openpdf_core::TimestampErrorKind::UrlMissing
            ))
        ));
        let request = request.with_tsa_url("ftp://tsa.invalid/");
        assert!(matches!(
            signer.sign(&pdf, &request),
            Err(Error::Timestamp(openpdf_core::TimestampErrorKind::BadUrl))
        ));
    }) else {
        return;
    };
}
