//! Enrollment helpers use only pinned trust, never an accept-any verifier.
use super::{TestClient, TestDevice, TestServer, agent_request};
use porchlight_core::auth::{enroll_uri::EnrollUri, session::Platform};
use porchlight_pki::{PinnedRootVerifier, enrollment_target, generate_csr, parse_cert_pem};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;
impl TestClient {
    pub fn pinned(pin: [u8; 32]) -> Self {
        Self::pinned_for(pin, "localhost")
    }
    pub fn pinned_for(pin: [u8; 32], server_name: &str) -> Self {
        let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(PinnedRootVerifier::new(pin))
        .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Self {
            config: Arc::new(config),
            server_name: server_name.to_owned(),
            timeout: super::RESPONSE_TIMEOUT,
        }
    }
}
fn request(
    path: &str,
    body: serde_json::Value,
) -> axum::http::Request<http_body_util::Full<hyper::body::Bytes>> {
    axum::http::Request::builder()
        .header("porchlight-client-version", porchlight_core::VERSION)
        .method("POST")
        .uri(path)
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("idempotency-key", Uuid::now_v7().to_string())
        .body(http_body_util::Full::new(hyper::body::Bytes::from(
            serde_json::to_vec(&body).unwrap(),
        )))
        .unwrap()
}
impl TestServer {
    pub fn bootstrap_token(&self) -> &str {
        &self.init.bootstrap_token
    }
    pub async fn enroll(&self, username: &str, password: &str, platform: Platform) -> TestDevice {
        let r = agent_request(
            &self.agent_socket,
            Some(self.bootstrap_token()),
            request(
                "/v1/admin/enrollment-tokens",
                json!({"username":username,"device_label":"Test device"}),
            ),
        )
        .await;
        assert_eq!(r.status, 201, "{r:?}");
        let uri = EnrollUri::parse(r.json()["uri"].as_str().unwrap()).unwrap();
        let (name, target) = enrollment_target(&uri.host).unwrap();
        let (csr, key) = generate_csr("Test device").unwrap();
        let tcp = tokio::net::TcpStream::connect(target).await.unwrap();
        let tls = TestClient::pinned_for(uri.ca_fingerprint, &name)
            .connect_stream(tcp)
            .await
            .unwrap();
        let r = TestClient::send(tls, request("/v1/enrollment/complete",json!({"token":uri.token,"password":password,"csr":csr,"platform":platform.as_str()}))).await.unwrap();
        assert_eq!(r.status, 201, "{r:?}");
        let j = r.json();
        let cert = j["certificate"].as_str().unwrap().to_owned();
        let fingerprint = parse_cert_pem(&cert).unwrap().fingerprint;
        TestDevice {
            device_id: Uuid::parse_str(j["device_id"].as_str().unwrap()).unwrap(),
            cert_pem: cert,
            key_pem: key.to_string(),
            fingerprint,
            session: Some(j["session"]["token"].as_str().unwrap().to_owned()),
        }
    }
}
