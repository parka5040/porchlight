use axum::{Extension, body::Body};
use porchlight_api::{
    server::{ConnInfo, Transport},
    state::AppState,
    testing::{TestClient, TestDevice, TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;
use tower::ServiceExt;

fn upload(d: &TestDevice, path: &str, key: &str, bytes: &str) -> axum::http::Request<Body> {
    let body = format!(
        "--quota\r\nContent-Disposition: form-data; name=\"file\"; filename=\"fixture.txt\"\r\nContent-Type: text/plain\r\n\r\n{bytes}\r\n--quota--\r\n"
    );
    let mut request = d
        .request("POST", path, json!({}), Some(key))
        .map(|_| Body::from(body));
    request.headers_mut().insert(
        "content-type",
        "multipart/form-data; boundary=quota".parse().unwrap(),
    );
    request
}
fn router(state: AppState, d: &TestDevice) -> axum::Router {
    porchlight_api::routes::mtls_router(state).layer(Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(d.fingerprint),
    }))
}

#[sqlx::test(migrations = "../db/migrations")]
async fn cancelled_upload_remains_charged_including_after_restart(pool: PgPool) {
    let mut options = TestOptions {
        background_derivation: false,
        ..Default::default()
    };
    options.settings.attachment_quota_bytes = 8;
    let s = TestServer::start_with(&pool, options).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let item = TestClient::new(&s.ca_pem, Some(&d))
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/items",
                json!({"title":"Quota cancellation"}),
                None,
            ),
        )
        .await
        .unwrap()
        .json();
    let path = format!("/v1/items/{}/attachments", item["id"].as_str().unwrap());
    let ingest = porchlight_core::ingest::ingest(
        b"12345678",
        Some("fixture.txt"),
        Some("text/plain"),
        &porchlight_core::ingest::Limits::production(),
    )
    .unwrap();
    let canonical =
        porchlight_api::blobs::blob_path(&s.state.settings.data_dir, &ingest.sha256).unwrap();
    // Block attachment INSERT, which happens after the canonical file is durable.
    let mut gate = porchlight_db::tx::begin(&pool).await.unwrap();
    sqlx::query("LOCK TABLE porchlight.attachments IN SHARE MODE")
        .execute(&mut *gate)
        .await
        .unwrap();
    let task = tokio::spawn(router(s.state.clone(), &d).oneshot(upload(
        &d,
        &path,
        "cancelled",
        "12345678",
    )));
    tokio::time::timeout(Duration::from_secs(5), async {
        while !canonical.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("upload never reached canonical persistence");
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    gate.rollback().await.unwrap();
    // Await rollback, not merely the handler's cancellation signal.
    let mut tx = porchlight_db::tx::begin(&pool).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        sqlx::query("SELECT shard FROM porchlight.blob_storage_locks WHERE shard='00' FOR UPDATE")
            .execute(&mut *tx),
    )
    .await
    .unwrap()
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(std::fs::metadata(&canonical).unwrap().len(), 8);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.blobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "cancelled metadata must roll back");
    let restarted = AppState::new(
        s.state.pool.clone(),
        s.state.pki.clone(),
        s.state.settings.clone(),
        &[0; 32],
        s.state.hasher.clone(),
        Default::default(),
    )
    .await
    .unwrap();
    for (state, key) in [(s.state.clone(), "next"), (restarted, "restarted")] {
        let response = router(state, &d)
            .oneshot(upload(&d, &path, key, "abcdefgh"))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            507,
            "cancelled canonical bytes must remain charged before and after restart"
        );
    }
}
