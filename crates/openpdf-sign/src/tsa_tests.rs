//! PAdES B-T against a local RFC 3161 TSA (no external network).
//!
//! The TSA is a tiny HTTP/1.1 server on 127.0.0.1 that signs `TSTInfo` with the
//! test-only keys in `tests/fixtures/tsa`. Each test starts its own server.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cmpv2::status::{PkiStatus, PkiStatusInfo};
use cms::cert::{CertificateChoices, IssuerAndSerialNumber};
use cms::content_info::{CmsVersion, ContentInfo};
use cms::signed_data::{
    CertificateSet, EncapsulatedContentInfo, SignedData, SignerIdentifier, SignerInfo, SignerInfos,
};
use const_oid::ObjectIdentifier;
use der::asn1::{GeneralizedTime, Int, OctetString, SetOfVec};
use der::{Any, Decode, Encode, Sequence};
use openpdf_core::test_support::with_locale;
use openpdf_core::{
    format_verify_result, CancelToken, Error, PadesLevel, SignRequest, Signer, TimestampErrorKind,
};
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::signature::{SignatureEncoding, Signer as _};
use rsa::RsaPrivateKey;
use sha2::{Digest, Sha256};
use x509_cert::attr::Attribute;
use x509_cert::spki::AlgorithmIdentifierOwned;
use x509_cert::Certificate;
use x509_tsp::{TimeStampReq, TimeStampResp, TspVersion, TstInfo};

use crate::tests::{corpus, test_env, with_softhsm, PanicPin, USER_PIN};
use crate::token::{verify_pdf_with_tsa_trust, Pkcs11Config, Pkcs11Signer};
use crate::tsp::Tsa;
use crate::{cades, pades, StaticPinProvider};

const ROOT_CERT: &[u8] = include_bytes!("../tests/fixtures/tsa/root.cert.der");
const ROOT_KEY: &[u8] = include_bytes!("../tests/fixtures/tsa/root.key.der");
const TSA_CERT: &[u8] = include_bytes!("../tests/fixtures/tsa/tsa.cert.der");
const TSA_KEY: &[u8] = include_bytes!("../tests/fixtures/tsa/tsa.key.der");
const EVIL_TSA_CERT: &[u8] = include_bytes!("../tests/fixtures/tsa/eviltsa.cert.der");
const EVIL_TSA_KEY: &[u8] = include_bytes!("../tests/fixtures/tsa/eviltsa.key.der");

const ID_CT_TST_INFO: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.1.4");
const ID_AA_SIGNING_CERTIFICATE_V2: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.47");
const TEST_POLICY: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.4.1.99999.1.1");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Good,
    WrongNonce,
    WrongImprint,
    Untrusted,
    Tampered,
    Reject,
    /// Valid reply, but only after 5 s.
    Slow,
    EkuNonCritical,
    EkuMulti,
    EkuMissing,
    /// genTime after the TSA (and root) certificate's notAfter.
    ExpiredAtGenTime,
    /// ESS signing-certificate-v2 names another certificate.
    EssMismatch,
    /// 300 KiB body, over the client's 256 KiB limit.
    Huge,
    /// 302 to the given port on 127.0.0.1.
    Redirect(u16),
    /// Intermediate CA without keyCertSign.
    NoKeyCertSign,
    /// root -> inter (pathLen 0) -> inter2 -> TSA.
    PathLenExceeded,
    /// root -> inter (pathLen 0) -> TSA: allowed.
    PathLenOk,
    /// Good TSA, plus bulky extra certificates: token overflows /Contents.
    LongChain,
}

macro_rules! fx {
    ($n:literal) => {
        include_bytes!(concat!("../tests/fixtures/tsa/", $n, ".cert.der")).as_slice()
    };
}

/// (leaf certificate, extra certificates in the token) for a mode.
fn mode_certs(mode: Mode) -> (&'static [u8], Vec<&'static [u8]>) {
    match mode {
        Mode::Untrusted => (EVIL_TSA_CERT, vec![]),
        Mode::EkuNonCritical => (fx!("tsa-eku-noncrit"), vec![]),
        Mode::EkuMulti => (fx!("tsa-eku-multi"), vec![]),
        Mode::EkuMissing => (fx!("tsa-eku-none"), vec![]),
        Mode::NoKeyCertSign => (fx!("tsa-under-nokcs"), vec![fx!("inter-nokcs")]),
        Mode::PathLenExceeded => (
            fx!("tsa-under-inter2"),
            vec![fx!("inter2"), fx!("inter-pl0")],
        ),
        Mode::PathLenOk => (fx!("tsa-under-pl0"), vec![fx!("inter-pl0")]),
        Mode::LongChain => (
            TSA_CERT,
            vec![
                fx!("filler-01"),
                fx!("filler-02"),
                fx!("filler-03"),
                fx!("filler-04"),
                fx!("filler-05"),
                fx!("filler-06"),
                fx!("filler-07"),
                fx!("filler-08"),
                fx!("filler-09"),
                fx!("filler-10"),
                fx!("filler-11"),
                fx!("filler-12"),
            ],
        ),
        _ => (TSA_CERT, vec![]),
    }
}

/// Start a local TSA; returns its `http://127.0.0.1:<port>/` URL.
fn spawn_tsa(mode: Mode) -> String {
    spawn_tsa_notify(mode, None)
}

/// Like [`spawn_tsa`]; sends on `arrived` as soon as a request body is read.
fn spawn_tsa_notify(mode: Mode, arrived: Option<std::sync::mpsc::Sender<()>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}/tsa", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = serve(stream, mode, arrived.clone());
        }
    });
    url
}

fn serve(
    stream: TcpStream,
    mode: Mode,
    arrived: Option<std::sync::mpsc::Sender<()>>,
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut len = 0usize;
    let mut first = String::new();
    reader.read_line(&mut first)?;
    assert!(first.starts_with("POST "), "TSA expects POST: {first}");
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
        if let Some(v) = lower.strip_prefix("content-type:") {
            assert_eq!(v.trim(), "application/timestamp-query");
        }
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    let req = TimeStampReq::from_der(&body).expect("TimeStampReq");
    assert!(req.cert_req, "client must ask for the TSA certificate");
    assert!(req.nonce.is_some(), "client must send a nonce");
    if let Some(tx) = arrived {
        let _ = tx.send(());
    }
    if mode == Mode::Slow {
        std::thread::sleep(Duration::from_secs(5));
    }
    let mut out = stream;
    if let Mode::Redirect(port) = mode {
        return write!(
            out,
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/tsa\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
    }
    if mode == Mode::Huge {
        let body = vec![0x30u8; 300 * 1024];
        write!(
            out,
            "HTTP/1.1 200 OK\r\nContent-Type: application/timestamp-reply\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )?;
        let _ = out.write_all(&body);
        return out.flush();
    }
    let reply = respond(mode, &req);
    write!(
        out,
        "HTTP/1.1 200 OK\r\nContent-Type: application/timestamp-reply\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reply.len()
    )?;
    out.write_all(&reply)?;
    out.flush()
}

#[derive(Sequence)]
struct EssCertIdV2 {
    cert_hash: OctetString,
}

#[derive(Sequence)]
struct SigningCertificateV2 {
    certs: Vec<EssCertIdV2>,
}

fn attr(oid: ObjectIdentifier, value: Any) -> Attribute {
    Attribute {
        oid,
        values: SetOfVec::try_from(vec![value]).unwrap(),
    }
}

fn any_of(v: &impl Encode) -> Any {
    Any::from_der(&v.to_der().unwrap()).unwrap()
}

fn respond(mode: Mode, req: &TimeStampReq) -> Vec<u8> {
    let status = PkiStatusInfo {
        status: if mode == Mode::Reject {
            PkiStatus::Rejection
        } else {
            PkiStatus::Accepted
        },
        status_string: None,
        fail_info: None,
    };
    if mode == Mode::Reject {
        return TimeStampResp {
            status,
            time_stamp_token: None,
        }
        .to_der()
        .unwrap();
    }
    let (cert_der, extra) = mode_certs(mode);
    let key_der = if mode == Mode::Untrusted {
        EVIL_TSA_KEY
    } else {
        TSA_KEY
    };
    let cert = Certificate::from_der(cert_der).unwrap();

    let mut imprint = req.message_imprint.clone();
    if mode == Mode::WrongImprint {
        let mut h = imprint.hashed_message.as_bytes().to_vec();
        h[0] ^= 0xff;
        imprint.hashed_message = OctetString::new(h).unwrap();
    }
    let nonce = if mode == Mode::WrongNonce {
        Some(Int::new(&[0x01, 0x02, 0x03]).unwrap())
    } else {
        req.nonce.clone()
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let mut tst = TstInfo {
        version: TspVersion::V1,
        policy: TEST_POLICY,
        message_imprint: imprint,
        serial_number: Int::new(&[0x2a]).unwrap(),
        gen_time: GeneralizedTime::from_unix_duration(Duration::from_secs(
            if mode == Mode::ExpiredAtGenTime {
                2_429_913_600 // 2047-01-01, after every fixture's notAfter
            } else {
                now.as_secs()
            },
        ))
        .unwrap(),
        accuracy: None,
        ordering: false,
        nonce,
        tsa: None,
        extensions: None,
    };
    let tst_der = tst.to_der().unwrap();

    let scv2 = SigningCertificateV2 {
        certs: vec![EssCertIdV2 {
            cert_hash: OctetString::new(
                Sha256::digest(if mode == Mode::EssMismatch {
                    EVIL_TSA_CERT
                } else {
                    cert_der
                })
                .to_vec(),
            )
            .unwrap(),
        }],
    };
    let md = OctetString::new(Sha256::digest(&tst_der).to_vec()).unwrap();
    let signed_attrs: SetOfVec<Attribute> = SetOfVec::try_from(vec![
        attr(
            const_oid::db::rfc5911::ID_CONTENT_TYPE,
            Any::from(&ID_CT_TST_INFO),
        ),
        attr(const_oid::db::rfc5911::ID_MESSAGE_DIGEST, any_of(&md)),
        attr(ID_AA_SIGNING_CERTIFICATE_V2, any_of(&scv2)),
    ])
    .unwrap();
    let key = RsaPrivateKey::from_pkcs8_der(key_der).unwrap();
    let sig = SigningKey::<Sha256>::new(key)
        .sign(&signed_attrs.to_der().unwrap())
        .to_vec();

    // Tampered: the TSA signed one TSTInfo, the token carries another.
    let econtent_der = if mode == Mode::Tampered {
        tst.serial_number = Int::new(&[0x2b]).unwrap();
        tst.to_der().unwrap()
    } else {
        tst_der
    };

    let sha256 = AlgorithmIdentifierOwned {
        oid: const_oid::db::rfc5912::ID_SHA_256,
        parameters: None,
    };
    let si = SignerInfo {
        version: CmsVersion::V1,
        sid: SignerIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
            issuer: cert.tbs_certificate.issuer.clone(),
            serial_number: cert.tbs_certificate.serial_number.clone(),
        }),
        digest_alg: sha256.clone(),
        signed_attrs: Some(signed_attrs),
        signature_algorithm: AlgorithmIdentifierOwned {
            oid: const_oid::db::rfc5912::SHA_256_WITH_RSA_ENCRYPTION,
            parameters: Some(Any::null()),
        },
        signature: OctetString::new(sig).unwrap(),
        unsigned_attrs: None,
    };
    let sd = SignedData {
        version: CmsVersion::V3,
        digest_algorithms: SetOfVec::try_from(vec![sha256]).unwrap(),
        encap_content_info: EncapsulatedContentInfo {
            econtent_type: ID_CT_TST_INFO,
            econtent: Some(any_of(&OctetString::new(econtent_der).unwrap())),
        },
        certificates: Some(CertificateSet(
            SetOfVec::try_from(
                std::iter::once(cert)
                    .chain(extra.iter().map(|d| Certificate::from_der(d).unwrap()))
                    .map(CertificateChoices::Certificate)
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        )),
        crls: None,
        signer_infos: SignerInfos(SetOfVec::try_from(vec![si]).unwrap()),
    };
    let token = ContentInfo {
        content_type: const_oid::db::rfc5911::ID_SIGNED_DATA,
        content: any_of(&sd),
    };
    TimeStampResp {
        status,
        time_stamp_token: Some(token),
    }
    .to_der()
    .unwrap()
}

fn trusted() -> Arc<Vec<Certificate>> {
    Arc::new(vec![Certificate::from_der(ROOT_CERT).unwrap()])
}

/// Software signer (test root key) so these tests do not need SoftHSM.
fn soft_cms(message: &[u8]) -> openpdf_core::Result<Vec<u8>> {
    cades::sign_detached(message, ROOT_CERT, |tbs| {
        let key = RsaPrivateKey::from_pkcs8_der(ROOT_KEY).unwrap();
        Ok(SigningKey::<Sha256>::new(key).sign(tbs).to_vec())
    })
}

fn sign_bt(url: &str) -> openpdf_core::Result<Vec<u8>> {
    let pdf = std::fs::read(corpus("hello.pdf")).unwrap();
    let request = SignRequest::new("soft").with_level(PadesLevel::BT);
    let tsa = Tsa::new(Some(url), Duration::from_secs(10), trusted())?;
    pades::sign_incremental(&pdf, &request, "Test", Some(&tsa), soft_cms)
}

fn ts_kind(r: openpdf_core::Result<Vec<u8>>) -> TimestampErrorKind {
    match r {
        Err(Error::Timestamp(kind)) => kind,
        Err(e) => panic!("expected a timestamp error, got {e:?}"),
        Ok(_) => panic!("B-T must fail, not produce a signature"),
    }
}

#[test]
fn bt_good_signature_verifies_with_timestamp() {
    let url = spawn_tsa(Mode::Good);
    let pdf = std::fs::read(corpus("hello.pdf")).unwrap();
    let signed = sign_bt(&url).expect("B-T sign");
    assert!(signed.starts_with(&pdf));

    let infos = pades::verify_signatures(&signed, &trusted()).unwrap();
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].valid, Some(true));
    let ts = infos[0].timestamp.as_ref().expect("timestamp reported");
    assert!(ts.valid, "{ts:?}");
    assert_eq!(ts.problem, None);
    let time = ts.time.as_deref().expect("genTime");
    let year: i32 = time[..4].parse().unwrap();
    assert!(year >= 2026 && time.ends_with('Z'), "{time}");
    with_locale("en", || {
        let line = format_verify_result(&infos[0]);
        assert!(
            line.contains(&format!("Timestamp: {time} (valid)")),
            "{line}"
        );
    });

    // Same PDF, TSA root not trusted: signature still valid, timestamp not.
    let infos = pades::verify_signatures(&signed, &[]).unwrap();
    assert_eq!(infos[0].valid, Some(true));
    let ts = infos[0].timestamp.as_ref().unwrap();
    assert!(!ts.valid);
    assert_eq!(ts.problem, Some(TimestampErrorKind::UntrustedCert));
    assert!(ts.time.is_some());
    with_locale("en", || {
        let line = format_verify_result(&infos[0]);
        assert!(line.contains("(invalid)"), "{line}");
    });

    // B-B signatures report no timestamp.
    let bb =
        pades::sign_incremental(&pdf, &SignRequest::new("soft"), "Test", None, soft_cms).unwrap();
    let infos = pades::verify_signatures(&bb, &trusted()).unwrap();
    assert_eq!(infos[0].valid, Some(true), "{:?}", infos[0]);
    assert!(infos[0].timestamp.is_none());
}

#[test]
fn bt_wrong_nonce_is_refused() {
    let url = spawn_tsa(Mode::WrongNonce);
    assert_eq!(ts_kind(sign_bt(&url)), TimestampErrorKind::NonceMismatch);
}

#[test]
fn bt_wrong_imprint_is_refused() {
    let url = spawn_tsa(Mode::WrongImprint);
    assert_eq!(ts_kind(sign_bt(&url)), TimestampErrorKind::ImprintMismatch);
}

#[test]
fn bt_untrusted_tsa_cert_is_refused() {
    let url = spawn_tsa(Mode::Untrusted);
    assert_eq!(ts_kind(sign_bt(&url)), TimestampErrorKind::UntrustedCert);
}

#[test]
fn bt_tampered_token_is_refused() {
    let url = spawn_tsa(Mode::Tampered);
    assert_eq!(ts_kind(sign_bt(&url)), TimestampErrorKind::BadSignature);
}

#[test]
fn bt_rejected_request_is_refused() {
    let url = spawn_tsa(Mode::Reject);
    assert_eq!(
        ts_kind(sign_bt(&url)),
        TimestampErrorKind::Rejected { status: 2 }
    );
}

#[test]
fn bt_unreachable_tsa_is_a_network_error() {
    // Keep the port bound (a freed port can be reused by a parallel test's
    // TSA) and drop every connection without replying.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in l.incoming().flatten() {
            drop(stream);
        }
    });
    let kind = ts_kind(sign_bt(&format!("http://127.0.0.1:{port}/")));
    assert!(
        matches!(kind, TimestampErrorKind::Network { .. }),
        "{kind:?}"
    );
}

#[test]
fn bt_bad_url_is_refused_before_signing() {
    for url in ["", "   ", "ftp://tsa.example/", "tsa.example"] {
        let mut called = false;
        let pdf = std::fs::read(corpus("hello.pdf")).unwrap();
        let r = Tsa::new(Some(url), Duration::from_secs(1), trusted()).and_then(|tsa| {
            pades::sign_incremental(
                &pdf,
                &SignRequest::new("x").with_level(PadesLevel::BT),
                "x",
                Some(&tsa),
                |_| {
                    called = true;
                    Ok(Vec::new())
                },
            )
        });
        let kind = ts_kind(r);
        assert!(
            matches!(
                kind,
                TimestampErrorKind::UrlMissing | TimestampErrorKind::BadUrl
            ),
            "{url}: {kind:?}"
        );
        assert!(!called);
    }
}

#[test]
fn timestamp_errors_are_translated() {
    let kinds = [
        TimestampErrorKind::UrlMissing,
        TimestampErrorKind::BadUrl,
        TimestampErrorKind::Network { detail: "x".into() },
        TimestampErrorKind::Rng { detail: "x".into() },
        TimestampErrorKind::ResponseTooLarge,
        TimestampErrorKind::TokenTooLarge,
        TimestampErrorKind::Http { status: 500 },
        TimestampErrorKind::Rejected { status: 2 },
        TimestampErrorKind::Malformed,
        TimestampErrorKind::NonceMismatch,
        TimestampErrorKind::ImprintMismatch,
        TimestampErrorKind::BadSignature,
        TimestampErrorKind::SigningCertMismatch,
        TimestampErrorKind::UntrustedCert,
    ];
    for kind in kinds {
        let mut en = String::new();
        with_locale("en", || en = Error::Timestamp(kind.clone()).to_string());
        let mut es = String::new();
        with_locale("es", || es = Error::Timestamp(kind.clone()).to_string());
        assert_ne!(en, es, "{kind:?} has no Spanish translation");
    }
    let info = openpdf_core::SignatureInfo::default().with_timestamp(Some(
        openpdf_core::TimestampInfo::new(Some("2026-10-10T16:00:00Z".into()), true, None),
    ));
    let mut en = String::new();
    with_locale("en", || en = format_verify_result(&info));
    let mut es = String::new();
    with_locale("es", || es = format_verify_result(&info));
    assert_ne!(en, es);
}

#[test]
fn bt_with_softhsm_end_to_end() {
    let url = spawn_tsa(Mode::Good);
    let Some(()) = with_softhsm(move || {
        let env = test_env();
        let config = Pkcs11Config::with_module(&env.module)
            .with_tsa_url(url.clone())
            .with_tsa_trust_anchor(ROOT_CERT.to_vec());
        let signer = Pkcs11Signer::new(config, StaticPinProvider::new(USER_PIN)).unwrap();
        let cert = signer.list_certificates().unwrap()[0].clone();
        let pdf = std::fs::read(corpus("hello.pdf")).unwrap();
        let request = SignRequest::new(cert.id.clone())
            .with_level(PadesLevel::BT)
            .with_reason("B-T test");
        let signed = signer.sign(&pdf, &request).expect("B-T sign via PKCS#11");
        let infos = signer.verify(&signed).unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].valid, Some(true));
        assert!(infos[0].timestamp.as_ref().is_some_and(|t| t.valid));
        let infos = verify_pdf_with_tsa_trust(&signed, &[ROOT_CERT.to_vec()]).unwrap();
        assert!(infos[0].timestamp.as_ref().is_some_and(|t| t.valid));

        // No URL anywhere: refused before the PIN prompt (PanicPin).
        let no_url = Pkcs11Signer::new(Pkcs11Config::with_module(&env.module), PanicPin).unwrap();
        let err = no_url
            .sign(
                &pdf,
                &SignRequest::new(cert.id.clone()).with_level(PadesLevel::BT),
            )
            .unwrap_err();
        assert!(matches!(
            err,
            Error::Timestamp(TimestampErrorKind::UrlMissing)
        ));

        // A URL on the request overrides the config one.
        let evil = spawn_tsa(Mode::Untrusted);
        let err = signer
            .sign(&pdf, &request.clone().with_tsa_url(evil))
            .unwrap_err();
        assert!(matches!(
            err,
            Error::Timestamp(TimestampErrorKind::UntrustedCert)
        ));
    }) else {
        return;
    };
}

/// Tokens made by `openssl ts -reply` with the test TSA key (ESSCertIDv2 and
/// the older SHA-1 ESSCertID), over `openssl-token-data.bin`.
#[test]
fn openssl_ts_tokens_validate() {
    let data = include_bytes!("../tests/fixtures/tsa/openssl-token-data.bin");
    for tok in [
        &include_bytes!("../tests/fixtures/tsa/openssl-token-essv2.der")[..],
        &include_bytes!("../tests/fixtures/tsa/openssl-token-essv1.der")[..],
    ] {
        let r = crate::tsp::validate_token(tok, data, None, &trusted(), true);
        assert!(r.is_ok(), "{r:?}");
        let r = crate::tsp::validate_token(tok, b"other", None, &trusted(), true);
        assert_eq!(r, Err(TimestampErrorKind::ImprintMismatch));
        let r = crate::tsp::validate_token(tok, data, None, &[], true);
        assert_eq!(r, Err(TimestampErrorKind::UntrustedCert));
    }
}

#[test]
fn bt_cancel_while_waiting_for_tsa_returns_fast_and_writes_nothing() {
    // The TSA holds its reply for 5 s. Cancel 100 ms after the request
    // reaches it; signing must return within 500 ms of the request arriving.
    let (arrived_tx, arrived_rx) = std::sync::mpsc::channel();
    let url = spawn_tsa_notify(Mode::Slow, Some(arrived_tx));
    let pdf = std::fs::read(corpus("hello.pdf")).unwrap();
    let original = pdf.clone();
    let cancel = CancelToken::new();
    let request = SignRequest::new("soft")
        .with_level(PadesLevel::BT)
        .with_cancel(cancel.clone());
    let tsa = Tsa::new(Some(&url), Duration::from_secs(20), trusted()).unwrap();
    let canceller = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            arrived_rx
                .recv_timeout(Duration::from_secs(30))
                .expect("TSA request");
            let at = std::time::Instant::now();
            std::thread::sleep(Duration::from_millis(100));
            cancel.cancel();
            at
        })
    };
    let r = pades::sign_incremental(&pdf, &request, "Test", Some(&tsa), soft_cms);
    let returned = std::time::Instant::now();
    let arrived = canceller.join().unwrap();
    match r {
        Err(e) => assert!(e.is_cancelled(), "expected Cancelled, got {e:?}"),
        Ok(_) => panic!("cancelled B-T must not produce a signature"),
    }
    let elapsed = returned - arrived;
    assert!(elapsed < Duration::from_millis(500), "took {elapsed:?}");
    assert_eq!(pdf, original, "input bytes changed");
    assert_eq!(std::fs::read(corpus("hello.pdf")).unwrap(), original);
}

#[test]
fn bt_cancel_before_tsa_request_sends_nothing() {
    // Unroutable port: if a request were sent it would fail as Network, not Cancelled.
    let cancel = CancelToken::new();
    cancel.cancel();
    let pdf = std::fs::read(corpus("hello.pdf")).unwrap();
    let request = SignRequest::new("soft")
        .with_level(PadesLevel::BT)
        .with_cancel(cancel);
    let tsa = Tsa::new(
        Some("http://127.0.0.1:9/tsa"),
        Duration::from_secs(5),
        trusted(),
    )
    .unwrap();
    let r = pades::sign_incremental(&pdf, &request, "Test", Some(&tsa), soft_cms);
    assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
}

#[test]
fn cancel_before_pin_prompt_never_asks_for_pin() {
    with_softhsm(|| {
        let env = test_env();
        let signer = Pkcs11Signer::new(Pkcs11Config::with_module(&env.module), PanicPin).unwrap();
        let cert = signer.list_certificates().unwrap()[0].clone();
        let cancel = CancelToken::new();
        cancel.cancel();
        let pdf = std::fs::read(corpus("hello.pdf")).unwrap();
        let r = signer.sign(&pdf, &SignRequest::new(cert.id.clone()).with_cancel(cancel));
        assert!(matches!(r, Err(Error::Cancelled)), "{r:?}");
    });
}

#[test]
fn cancelled_error_is_translated() {
    with_locale("es", || {
        // Spanish lives only in po/es.po; just check it is translated.
        assert_ne!(Error::Cancelled.to_string(), "operation cancelled")
    });
    with_locale("en", || {
        assert_eq!(Error::Cancelled.to_string(), "operation cancelled")
    });
}

#[test]
fn cancel_token_is_clone_send_sync() {
    fn assert_traits<T: Clone + Send + Sync + 'static>() {}
    assert_traits::<CancelToken>();
}

#[test]
fn bt_eku_must_be_critical_and_time_stamping_only() {
    for mode in [Mode::EkuNonCritical, Mode::EkuMulti, Mode::EkuMissing] {
        let url = spawn_tsa(mode);
        assert_eq!(
            ts_kind(sign_bt(&url)),
            TimestampErrorKind::UntrustedCert,
            "{mode:?}"
        );
    }
}

#[test]
fn bt_ess_mismatch_is_refused() {
    let url = spawn_tsa(Mode::EssMismatch);
    assert_eq!(
        ts_kind(sign_bt(&url)),
        TimestampErrorKind::SigningCertMismatch
    );
}

#[test]
fn bt_tsa_cert_expired_at_gen_time_is_refused() {
    let url = spawn_tsa(Mode::ExpiredAtGenTime);
    assert_eq!(ts_kind(sign_bt(&url)), TimestampErrorKind::UntrustedCert);
}

#[test]
fn bt_response_over_256_kib_is_refused() {
    let url = spawn_tsa(Mode::Huge);
    assert_eq!(ts_kind(sign_bt(&url)), TimestampErrorKind::ResponseTooLarge);
}

#[test]
fn bt_redirect_is_not_followed() {
    let (hit_tx, hit_rx) = std::sync::mpsc::channel();
    let target = spawn_tsa_notify(Mode::Good, Some(hit_tx));
    let port: u16 = target
        .trim_start_matches("http://127.0.0.1:")
        .trim_end_matches("/tsa")
        .parse()
        .unwrap();
    let url = spawn_tsa(Mode::Redirect(port));
    assert_eq!(
        ts_kind(sign_bt(&url)),
        TimestampErrorKind::Http { status: 302 }
    );
    assert!(
        hit_rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "redirect target must not be contacted"
    );
}

#[test]
fn bt_intermediate_without_key_cert_sign_is_refused() {
    let url = spawn_tsa(Mode::NoKeyCertSign);
    assert_eq!(ts_kind(sign_bt(&url)), TimestampErrorKind::UntrustedCert);
}

#[test]
fn bt_path_len_constraint_is_enforced() {
    let url = spawn_tsa(Mode::PathLenExceeded);
    assert_eq!(ts_kind(sign_bt(&url)), TimestampErrorKind::UntrustedCert);
    // pathLen 0 still allows the leaf directly below it.
    let url = spawn_tsa(Mode::PathLenOk);
    sign_bt(&url).expect("pathLen 0 CA may issue the TSA leaf");
}

#[test]
fn bt_token_too_large_for_contents_fails_cleanly() {
    let url = spawn_tsa(Mode::LongChain);
    let pdf = std::fs::read(corpus("hello.pdf")).unwrap();
    assert_eq!(ts_kind(sign_bt(&url)), TimestampErrorKind::TokenTooLarge);
    assert_eq!(std::fs::read(corpus("hello.pdf")).unwrap(), pdf);
}

#[test]
fn bt_network_error_never_shows_url_credentials() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in l.incoming().flatten() {
            drop(stream);
        }
    });
    for url in [
        format!("https://user:pass@127.0.0.1:{port}/tsa"),
        format!("http://user:pass@127.0.0.1:{port}/tsa"),
    ] {
        let err = sign_bt(&url).unwrap_err();
        let mut text = String::new();
        with_locale("en", || text = err.to_string());
        let dbg = format!("{err:?}");
        for shown in [&text, &dbg] {
            assert!(!shown.contains("pass"), "{shown}");
            assert!(!shown.contains("user:"), "{shown}");
        }
        assert!(
            matches!(err, Error::Timestamp(TimestampErrorKind::Network { .. })),
            "{err:?}"
        );
    }
}

#[test]
fn url_credentials_are_redacted_from_details() {
    let url = "https://user:pass@tsa.example:8443/ts?x=1";
    let detail = "io: Connection refused for https://user:pass@tsa.example:8443/ts?x=1";
    let out = crate::tsp::redact(detail, url);
    assert_eq!(
        out,
        "io: Connection refused for https://tsa.example:8443/ts?x=1"
    );
    assert_eq!(crate::tsp::redact("plain", "https://tsa.example/"), "plain");
}
