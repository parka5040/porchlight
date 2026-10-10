#![allow(dead_code)] // Integration test binaries use different subsets of these shared helpers.
use axum::http::Request;
use http_body_util::Full;
use hyper::body::Bytes;
use porchlight_api::testing::{TestClient, TestDevice, TestResponse, TestServer};
use serde_json::{Value, json};
use uuid::Uuid;

pub fn request(
    d: &TestDevice,
    method: &str,
    path: &str,
    body: Value,
    key: Option<&str>,
) -> Request<Full<Bytes>> {
    let mut request = d.request(method, path, body, key);
    // Unlike the convenience harness helper, None explicitly tests a missing key.
    if key.is_none() {
        request.headers_mut().remove("idempotency-key");
    }
    request
}
pub async fn send(
    s: &TestServer,
    d: &TestDevice,
    method: &str,
    path: &str,
    body: Value,
    key: Option<&str>,
) -> TestResponse {
    TestClient::new(&s.ca_pem, Some(d))
        .request(s.lan, request(d, method, path, body, key))
        .await
        .unwrap()
}
pub async fn post(s: &TestServer, d: &TestDevice, path: &str, body: Value) -> TestResponse {
    send(s, d, "POST", path, body, Some(&Uuid::now_v7().to_string())).await
}
pub async fn login(s: &TestServer, d: &mut TestDevice) -> TestResponse {
    let r = post(s, d, "/v1/auth/login", json!({"password":s.admin_password})).await;
    assert_eq!(r.status, 200, "{r:?}");
    d.session = Some(r.json()["session"]["token"].as_str().unwrap().to_owned());
    r
}
pub async fn get(s: &TestServer, d: &TestDevice, path: &str) -> TestResponse {
    send(s, d, "GET", path, json!({}), None).await
}
pub fn problem(r: &TestResponse, status: u16, slug: &str) {
    assert_eq!(r.status, status);
    assert_eq!(r.headers["content-type"], "application/problem+json");
    assert_eq!(r.json()["type"], format!("urn:porchlight:problem:{slug}"));
    assert_eq!(r.json()["status"], status);
    assert!(r.json()["title"].as_str().is_some());
}

/// Scoped WARN capture: use WithSubscriber on a future, never install a global subscriber.
#[derive(Clone, Default)]
pub struct WarnSink(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for WarnSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl WarnSink {
    pub fn dispatch(&self) -> tracing::Dispatch {
        let writer = self.clone();
        tracing::Dispatch::new(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::WARN)
                .with_ansi(false)
                .without_time()
                .with_writer(move || writer.clone())
                .finish(),
        )
    }
    pub fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}
