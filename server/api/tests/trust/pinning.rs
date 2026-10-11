use porchlight_api::testing::TestClient;
use porchlight_api::testing::trust;
use porchlight_pki::{HouseholdCa, ServerSans};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};
use tokio::{io::AsyncReadExt, net::TcpListener};
use tokio_rustls::TlsAcceptor;
#[tokio::test]
async fn pinned_verifier_refuses_impostors_before_application_data() {
    let now = SystemTime::now();
    let ca = HouseholdCa::generate("genuine", now).unwrap();
    let other = HouseholdCa::generate("impostor", now).unwrap();
    let sans = ServerSans {
        dns: vec!["localhost".into()],
        ips: vec![],
    };
    for case in [
        "appended-genuine-root",
        "other-household",
        "wrong-name",
        "expired",
    ] {
        let leaf = match case {
            "appended-genuine-root" | "other-household" => {
                other.issue_server(&sans, now, 730).unwrap()
            }
            "expired" => ca
                .issue_server(&sans, now - Duration::from_secs(3 * 86400), 1)
                .unwrap(),
            _ => ca.issue_server(&sans, now, 730).unwrap(),
        };
        let root = if case == "other-household" {
            other.cert()
        } else {
            ca.cert()
        };
        let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![
                CertificateDer::from(leaf.cert.cert_der),
                CertificateDer::from(root.cert_der.clone()),
            ],
            PrivateKeyDer::from_pem_slice(leaf.key_pem.as_bytes()).unwrap(),
        )
        .unwrap();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
        config.send_tls13_tickets = 0;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let bytes = Arc::new(AtomicUsize::new(0));
        let observed = bytes.clone();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            if let Ok(Ok(mut stream)) = tokio::time::timeout(
                Duration::from_secs(3),
                TlsAcceptor::from(Arc::new(config)).accept(stream),
            )
            .await
            {
                let mut buffer = [0; 1024];
                if let Ok(Ok(n)) =
                    tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buffer)).await
                {
                    observed.fetch_add(n, Ordering::SeqCst);
                }
            }
        });
        let name = if case == "wrong-name" {
            "wrong.example"
        } else {
            "localhost"
        };
        let r = TestClient::pinned_for(ca.cert().fingerprint, name)
            .request(
                addr,
                trust::req(
                    "POST",
                    "/v1/enrollment/complete",
                    json!({"password":"must never reach impostor"}),
                    "pinning",
                ),
            )
            .await;
        assert!(r.is_err(), "{case}");
        task.await.unwrap();
        assert_eq!(bytes.load(Ordering::SeqCst), 0, "{case}");
    }
}
