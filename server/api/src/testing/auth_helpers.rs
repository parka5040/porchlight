//! Auth helpers split from the listener harness to keep it focused.
use super::{TestClient, TestDevice, TestResponse, TestServer};
use axum::http::Request;
use chrono::{Duration, Utc};
use http_body_util::Full;
use hyper::body::Bytes;
use porchlight_core::auth::{
    reset_code::{ResetCode, hash_reset_code},
    session::{Platform, RESET_CODE_MINUTES},
};
use porchlight_db::{
    auth,
    tx::{Actor, begin_as},
};
use serde_json::json;
use uuid::Uuid;
impl TestDevice {
    /// Reads carry the session but no replay key. Mutations default to a fresh UUID key.
    pub fn request(
        &self,
        method: &str,
        path: &str,
        body: serde_json::Value,
        key: Option<&str>,
    ) -> Request<Full<Bytes>> {
        let mut b = Request::builder()
            .header("porchlight-client-version", porchlight_core::VERSION)
            .method(method)
            .uri(path)
            .header("host", "localhost")
            .header("content-type", "application/json");
        if matches!(method, "POST" | "PUT" | "PATCH" | "DELETE") {
            b = b.header(
                "idempotency-key",
                key.map(str::to_owned)
                    .unwrap_or_else(|| Uuid::now_v7().to_string()),
            );
        }
        if let Some(token) = &self.session {
            b = b.header("authorization", format!("Bearer {token}"));
        }
        b.body(Full::new(Bytes::from(serde_json::to_vec(&body).unwrap())))
            .unwrap()
    }
}
impl TestServer {
    pub async fn login(&self, device: &mut TestDevice, password: &str) -> TestResponse {
        let response = TestClient::new(&self.ca_pem, Some(device))
            .request(
                self.lan,
                device.request("POST", "/v1/auth/login", json!({"password":password}), None),
            )
            .await
            .unwrap();
        if response.status == 200 {
            device.session = Some(
                response.json()["session"]["token"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
        }
        response
    }
    /// Uses the harness's shared initial password; additional users can call issue_device + login explicitly.
    pub async fn session_device(&self, member_id: Uuid, platform: Platform) -> TestDevice {
        let mut d = self.issue_device(member_id, platform).await;
        assert_eq!(self.login(&mut d, &self.admin_password).await.status, 200);
        d
    }
    pub async fn reset_code(&self, user_id: Uuid) -> String {
        let code = ResetCode::generate();
        let mut tx = begin_as(
            &self.state.pool,
            &Actor {
                member_id: Some(self.init.admin_member_id),
                device_id: None,
                request_id: Uuid::now_v7(),
                agent_label: None,
                token_id: None,
            },
        )
        .await
        .unwrap();
        auth::create_password_reset(
            &mut tx,
            self.init.household_id,
            user_id,
            &hash_reset_code(code.expose()).unwrap(),
            self.init.admin_member_id,
            Utc::now() + Duration::minutes(RESET_CODE_MINUTES),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        code.expose().to_owned()
    }
}
