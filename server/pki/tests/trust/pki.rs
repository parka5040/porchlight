use std::{collections::HashSet, sync::Arc, time::Duration};

use porchlight_pki::*;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, PKCS_ED25519,
};
use rustls::client::danger::ServerCertVerifier;
use rustls::{RootCertStore, client::WebPkiServerVerifier, server::WebPkiClientVerifier};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use x509_parser::prelude::*;

fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_791_158_400)
}

fn sans() -> ServerSans {
    ServerSans {
        dns: vec!["localhost".into()],
        ips: vec!["127.0.0.1".parse().unwrap(), "10.0.2.2".parse().unwrap()],
    }
}

fn roots(ca: &HouseholdCa) -> Arc<RootCertStore> {
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(ca.cert().cert_der.clone()))
        .unwrap();
    Arc::new(roots)
}

fn verify_server(ca: &HouseholdCa, cert: &IssuedCert, name: &str) -> Result<(), rustls::Error> {
    let verifier = WebPkiServerVerifier::builder_with_provider(
        roots(ca),
        Arc::new(rustls::crypto::ring::default_provider()),
    )
    .build()
    .unwrap();
    verifier
        .verify_server_cert(
            &CertificateDer::from(cert.cert_der.clone()),
            &[],
            &ServerName::try_from(name.to_owned()).unwrap(),
            &[],
            UnixTime::since_unix_epoch(now().duration_since(SystemTime::UNIX_EPOCH).unwrap()),
        )
        .map(|_| ())
}

fn assert_validity(info: &CertInfo, days: u64) {
    let actual = info.not_after.duration_since(info.not_before).unwrap();
    let expected = Duration::from_secs(days * 86400 + 300);
    assert!(actual.abs_diff(expected) <= Duration::from_secs(60));
    assert_eq!(info.not_before, now() - Duration::from_secs(300));
    assert_eq!(info.not_after, now() + Duration::from_secs(days * 86400));
}

fn assert_eku(cert: &IssuedCert, server: bool) {
    let (_, parsed) = parse_x509_certificate(&cert.cert_der).unwrap();
    let eku = &parsed.extended_key_usage().unwrap().unwrap().value;
    assert_eq!(eku.server_auth, server);
    assert_eq!(eku.client_auth, !server);
    assert!(
        !eku.any
            && !eku.code_signing
            && !eku.email_protection
            && !eku.time_stamping
            && !eku.ocsp_signing
    );
    assert!(eku.other.is_empty());
    let ku = &parsed.key_usage().unwrap().unwrap().value;
    assert_eq!(ku.flags, 1); // digitalSignature only
}

#[test]
fn ca_is_ca_and_self_signed() {
    let ca = HouseholdCa::generate("Park", now()).unwrap();
    let info = parse_cert_pem(&ca.cert().cert_pem).unwrap();
    assert!(info.is_ca);
    assert_eq!(info.common_name.as_deref(), Some("Porchlight Park CA"));
    assert_validity(&info, 3650);
    assert_eq!(fingerprint(&ca.cert().cert_der), ca.cert().fingerprint);
    let (_, cert) = parse_x509_certificate(&ca.cert().cert_der).unwrap();
    assert_eq!(cert.subject(), cert.issuer());
    cert.verify_signature(None).unwrap();
    let bc = &cert.basic_constraints().unwrap().unwrap().value;
    assert_eq!(bc.path_len_constraint, Some(0));
    let ku = &cert.key_usage().unwrap().unwrap().value;
    assert!(ku.key_cert_sign() && ku.crl_sign());
    assert_eq!(ku.flags, (1 << 5) | (1 << 6));
    assert_eq!(
        cert.subject()
            .iter_organization()
            .next()
            .unwrap()
            .as_str()
            .unwrap(),
        "Porchlight"
    );
}

#[test]
fn fingerprint_format_roundtrip() {
    let fp = fingerprint(b"certificate DER");
    let formatted = format_fingerprint(&fp);
    assert_eq!(formatted.len(), 71);
    assert_eq!(parse_fingerprint(&formatted).unwrap(), fp);
    assert_eq!(
        parse_fingerprint(&format!("sha256:{}", hex::encode_upper(fp))).unwrap(),
        fp
    );
    for bad in [
        hex::encode(fp),
        format!("sha256:{}", "a".repeat(63)),
        format!("sha256:{}", "a".repeat(65)),
        format!("sha256:{}", "g".repeat(64)),
    ] {
        assert!(parse_fingerprint(&bad).is_err());
    }
}

#[test]
fn ca_pem_roundtrip() {
    let ca = HouseholdCa::generate("Park", now()).unwrap();
    let loaded = HouseholdCa::from_pem(&ca.cert().cert_pem, &ca.key_pem()).unwrap();
    assert_eq!(loaded.cert().fingerprint, ca.cert().fingerprint);
    assert_eq!(loaded.cert().cert_der, ca.cert().cert_der);
    let server = loaded.issue_server(&sans(), now(), 730).unwrap();
    verify_server(&ca, &server.cert, "porchlight.lan").unwrap();
}

#[test]
fn server_cert_has_sans_and_eku() {
    let ca = HouseholdCa::generate("Park", now()).unwrap();
    let server = ca.issue_server(&sans(), now(), 730).unwrap();
    let info = parse_cert_der(&server.cert.cert_der).unwrap();
    assert!(!info.is_ca);
    assert_eq!(info.common_name.as_deref(), Some("porchlight.lan"));
    assert!(info.sans.dns.contains(&"porchlight.lan".to_owned()));
    assert!(info.sans.dns.contains(&"localhost".to_owned()));
    assert_eq!(info.sans.ips, sans().ips);
    assert_validity(&info, 730);
    assert_eku(&server.cert, true);
    let mut names = sans();
    names.dns.push("porchlight.lan".into());
    let server = ca.issue_server(&names, now(), 730).unwrap();
    let info = parse_cert_der(&server.cert.cert_der).unwrap();
    assert_eq!(
        info.sans
            .dns
            .iter()
            .filter(|s| *s == "porchlight.lan")
            .count(),
        1
    );
}

#[test]
fn server_cert_verifies_with_rustls_webpki() {
    let ca = HouseholdCa::generate("Park", now()).unwrap();
    let server = ca.issue_server(&sans(), now(), 730).unwrap();
    verify_server(&ca, &server.cert, "porchlight.lan").unwrap();
    verify_server(&ca, &server.cert, "127.0.0.1").unwrap();
    assert!(verify_server(&ca, &server.cert, "example.com").is_err());
}

#[test]
fn device_cert_from_csr() {
    let ca = HouseholdCa::generate("Park", now()).unwrap();
    let (csr, key_pem) = generate_csr("ignored").unwrap();
    assert_eq!(validate_csr(&csr).unwrap().as_deref(), Some("ignored"));
    let cert = ca
        .issue_device(&csr, "0192aabb-device-uuid", now(), 365)
        .unwrap();
    let info = parse_cert_pem(&cert.cert_pem).unwrap();
    assert_eq!(info.common_name.as_deref(), Some("0192aabb-device-uuid"));
    assert!(!info.is_ca);
    assert_validity(&info, 365);
    assert_eku(&cert, false);
    let verifier = WebPkiClientVerifier::builder_with_provider(
        roots(&ca),
        Arc::new(rustls::crypto::ring::default_provider()),
    )
    .build()
    .unwrap();
    verifier
        .verify_client_cert(
            &CertificateDer::from(cert.cert_der.clone()),
            &[],
            UnixTime::since_unix_epoch(now().duration_since(SystemTime::UNIX_EPOCH).unwrap()),
        )
        .unwrap();
    let (_, parsed) = parse_x509_certificate(&cert.cert_der).unwrap();
    let key = KeyPair::from_pem(&key_pem).unwrap();
    assert_eq!(
        parsed.public_key().raw,
        rcgen::PublicKeyData::subject_public_key_info(&key)
    );
}

#[test]
fn device_cert_ignores_requested_ca_and_sans() {
    let ca = HouseholdCa::generate("Park", now()).unwrap();
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params = CertificateParams::new(vec!["evil.example".to_owned()]).unwrap();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.distinguished_name.push(DnType::CommonName, "evil");
    let csr = params.serialize_request(&key).unwrap().pem().unwrap();
    let cert = ca.issue_device(&csr, "device", now(), 365).unwrap();
    let info = parse_cert_pem(&cert.cert_pem).unwrap();
    assert!(!info.is_ca);
    assert!(info.sans.dns.is_empty() && info.sans.ips.is_empty());
    assert_eq!(info.common_name.as_deref(), Some("device"));
    assert_eku(&cert, false);
}

#[test]
fn csr_rejects_bad_input() {
    let ca = HouseholdCa::generate("Park", now()).unwrap();
    let (csr, _) = generate_csr("valid").unwrap();
    let mut tampered = csr.clone().into_bytes();
    // Change a base64 character near the end, within the signature (not the padding).
    let end = csr.find("-----END").unwrap();
    let index = (end - 12..end)
        .find(|&i| tampered[i].is_ascii_alphanumeric())
        .unwrap();
    tampered[index] = if tampered[index] == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(tampered).unwrap();
    let ed = KeyPair::generate_for(&PKCS_ED25519).unwrap();
    let ed_csr = CertificateParams::default()
        .serialize_request(&ed)
        .unwrap()
        .pem()
        .unwrap();
    for bad in [&tampered, &ed_csr, "garbage PEM"] {
        assert!(validate_csr(bad).is_err());
        assert!(ca.issue_device(bad, "device", now(), 365).is_err());
    }
    let p384 = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P384_SHA384).unwrap();
    let csr = CertificateParams::default()
        .serialize_request(&p384)
        .unwrap()
        .pem()
        .unwrap();
    assert!(matches!(validate_csr(&csr), Err(PkiError::UnsupportedKey)));
}

#[test]
fn serials_are_unique_and_positive() {
    let ca = HouseholdCa::generate("Park", now()).unwrap();
    let (csr, _) = generate_csr("device").unwrap();
    let mut serials = HashSet::new();
    for _ in 0..50 {
        let cert = ca.issue_device(&csr, "device", now(), 365).unwrap();
        let serial = hex::decode(&cert.serial_hex).unwrap();
        assert_eq!(serial.len(), 16);
        assert!(serial[0] > 0 && serial[0] < 0x80);
        assert!(serials.insert(cert.serial_hex));
    }
}

#[test]
fn debug_never_prints_keys() {
    let ca = HouseholdCa::generate("Park", now()).unwrap();
    let key = ca.issue_server(&sans(), now(), 730).unwrap();
    let debug = format!("{key:?}");
    assert!(!debug.contains("PRIVATE KEY"));
    assert!(!debug.contains(key.key_pem.as_str()));
}

#[test]
fn invalid_certificate_and_ca_inputs_are_rejected() {
    assert!(parse_cert_pem("garbage").is_err());
    assert!(parse_cert_der(b"garbage").is_err());
    let ca = HouseholdCa::generate("Park", now()).unwrap();
    let other = HouseholdCa::generate("Other", now()).unwrap();
    assert!(HouseholdCa::from_pem(&ca.cert().cert_pem, &other.key_pem()).is_err());
    let server = ca.issue_server(&sans(), now(), 730).unwrap();
    assert!(HouseholdCa::from_pem(&server.cert.cert_pem, &server.key_pem).is_err());
    let err = HouseholdCa::from_pem(&ca.cert().cert_pem, "SECRET BAD KEY")
        .err()
        .unwrap();
    assert!(!format!("{err:?}: {err}").contains("SECRET BAD KEY"));
    let mut trailing = ca.cert().cert_der.clone();
    trailing.push(0);
    assert!(parse_cert_der(&trailing).is_err());
}

#[test]
fn device_ignores_unknown_requested_extensions() {
    let ca = HouseholdCa::generate("Park", now()).unwrap();
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params = CertificateParams::default();
    params
        .custom_extensions
        .push(rcgen::CustomExtension::from_oid_content(
            &[1, 2, 3, 4],
            vec![5, 0],
        ));
    let csr = params.serialize_request(&key).unwrap().pem().unwrap();
    assert!(validate_csr(&csr).is_ok());
    let cert = ca.issue_device(&csr, "device", now(), 365).unwrap();
    let (_, parsed) = parse_x509_certificate(&cert.cert_der).unwrap();
    assert!(
        !parsed
            .extensions()
            .iter()
            .any(|ext| ext.oid.to_id_string() == "1.2.3.4")
    );
}
