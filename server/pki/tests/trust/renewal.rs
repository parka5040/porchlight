use porchlight_pki::*;
use std::time::{Duration, SystemTime};
#[test]
fn renewal_keeps_key_and_profile() {
    let now = SystemTime::now();
    let ca = HouseholdCa::generate("Home", now).unwrap();
    let sans = ServerSans {
        dns: vec!["localhost".into()],
        ips: vec!["127.0.0.1".parse().unwrap()],
    };
    let leaf = ca.issue_server(&sans, now, 730).unwrap();
    let new = ca
        .reissue_server(
            &leaf.key_pem,
            &sans,
            now + Duration::from_secs(680 * 86400),
            730,
        )
        .unwrap();
    assert!(key_matches_cert(&leaf.key_pem, &new.cert_pem).unwrap());
    assert_ne!(leaf.cert.fingerprint, new.fingerprint);
    assert_eq!(
        parse_cert_pem(&leaf.cert.cert_pem).unwrap().sans,
        parse_cert_pem(&new.cert_pem).unwrap().sans
    );
    let other = HouseholdCa::generate("Other", now).unwrap();
    assert!(!key_matches_cert(&other.key_pem(), &new.cert_pem).unwrap());
    assert!(key_matches_cert("bad", &new.cert_pem).is_err());
    use x509_parser::prelude::*;
    let (_, parsed) = X509Certificate::from_der(&new.cert_der).unwrap();
    let (_, root) = X509Certificate::from_der(&ca.cert().cert_der).unwrap();
    parsed.verify_signature(Some(root.public_key())).unwrap();
    assert!(
        parsed
            .extended_key_usage()
            .unwrap()
            .unwrap()
            .value
            .server_auth
    );
}
#[test]
fn reloaded_ca_is_canonical_pem() {
    let ca = HouseholdCa::generate("Home", SystemTime::now()).unwrap();
    let pem = format!("# comment\r\n{}", ca.cert().cert_pem.replace('\n', "\r\n"));
    let loaded = HouseholdCa::from_pem(&pem, &ca.key_pem()).unwrap();
    assert_eq!(loaded.cert().cert_pem, ca.cert().cert_pem);
}
