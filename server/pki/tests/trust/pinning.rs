use porchlight_pki::{HouseholdCa, PinnedRootVerifier, ServerSans, enrollment_target};
use rustls::client::danger::ServerCertVerifier;
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use std::time::{Duration, SystemTime};
#[test]
fn pinned_verifier_refuses_impostors() {
    let now = SystemTime::now();
    let ca = HouseholdCa::generate("home", now).unwrap();
    let other = HouseholdCa::generate("impostor", now).unwrap();
    let sans = ServerSans {
        dns: vec!["localhost".into()],
        ips: vec![],
    };
    let leaf = ca.issue_server(&sans, now, 730).unwrap();
    let fake = other.issue_server(&sans, now, 730).unwrap();
    let expired = ca
        .issue_server(&sans, now - Duration::from_secs(3 * 86400), 1)
        .unwrap();
    let verifier = PinnedRootVerifier::new(ca.cert().fingerprint);
    let root = CertificateDer::from(ca.cert().cert_der.clone());
    let name = ServerName::try_from("localhost").unwrap();
    let time = UnixTime::since_unix_epoch(now.duration_since(SystemTime::UNIX_EPOCH).unwrap());
    assert!(
        verifier
            .verify_server_cert(
                &CertificateDer::from(leaf.cert.cert_der.clone()),
                std::slice::from_ref(&root),
                &name,
                &[],
                time
            )
            .is_ok()
    );
    assert!(
        verifier
            .verify_server_cert(
                &CertificateDer::from(fake.cert.cert_der.clone()),
                std::slice::from_ref(&root),
                &name,
                &[],
                time
            )
            .is_err()
    );
    assert!(
        verifier
            .verify_server_cert(
                &CertificateDer::from(fake.cert.cert_der),
                &[CertificateDer::from(other.cert().cert_der.clone())],
                &name,
                &[],
                time
            )
            .is_err()
    );
    assert!(
        verifier
            .verify_server_cert(
                &CertificateDer::from(leaf.cert.cert_der.clone()),
                std::slice::from_ref(&root),
                &ServerName::try_from("wrong.example").unwrap(),
                &[],
                time
            )
            .is_err()
    );
    assert!(
        verifier
            .verify_server_cert(
                &CertificateDer::from(expired.cert.cert_der),
                &[root],
                &name,
                &[],
                time
            )
            .is_err()
    );
    assert!(
        verifier
            .verify_server_cert(
                &CertificateDer::from(leaf.cert.cert_der),
                &[],
                &name,
                &[],
                time
            )
            .is_err()
    );
}
#[test]
fn enrollment_targets() {
    assert_eq!(
        enrollment_target("10.0.2.2").unwrap(),
        ("10.0.2.2".into(), "10.0.2.2:8444".into())
    );
    assert_eq!(
        enrollment_target("[fd00::1]:9000").unwrap(),
        ("fd00::1".into(), "[fd00::1]:9000".into())
    );
    assert_eq!(
        enrollment_target("[::1]").unwrap(),
        ("::1".into(), "[::1]:8444".into())
    );
    assert_eq!(
        enrollment_target("porchlight.lan:8444").unwrap(),
        ("porchlight.lan".into(), "porchlight.lan:8444".into())
    );
    for invalid in [
        "",
        "https://host",
        "host/path",
        "user@host",
        "host:0",
        "host:65536",
        "fd00::1",
        "[bad]:9000",
        "host?token=secret",
        "host:abc",
        "host:8444#",
        " host",
    ] {
        assert!(enrollment_target(invalid).is_err());
    }
}
