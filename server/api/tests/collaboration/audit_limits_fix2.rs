use crate::common;
use axum::{
    Extension,
    body::{Body, Bytes},
};
use porchlight_api::{
    server::{ConnInfo, Transport},
    state::AppState,
    testing::{TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;

fn upload(
    d: &porchlight_api::testing::TestDevice,
    path: &str,
    key: &str,
    bytes: &[u8],
) -> hyper::Request<Body> {
    let mut body = b"--quota\r\nContent-Disposition: form-data; name=\"file\"; filename=\"fixture.txt\"\r\nContent-Type: text/plain\r\n\r\n".to_vec();
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n--quota--\r\n");
    let mut request = d
        .request("POST", path, json!({}), Some(key))
        .map(|_| Body::from(body));
    request.headers_mut().insert(
        "content-type",
        "multipart/form-data; boundary=quota".parse().unwrap(),
    );
    request
}

#[sqlx::test(migrations = "../db/migrations")]
async fn gc_reconciles_fresh_files_and_retires_only_removed_bytes(pool: PgPool) {
    let mut options = TestOptions {
        background_derivation: false,
        ..Default::default()
    };
    options.settings.attachment_quota_bytes = 24;
    let s = TestServer::start_with(&pool, options).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let item = common::post(&s, &d, "/v1/items", json!({"title":"GC quota"}))
        .await
        .json();
    let root = &s.state.settings.data_dir;
    // External fixtures deliberately appear after startup: GC must reconcile all files,
    // not just old canonical blobs eligible for removal.
    porchlight_api::blobs::durable_write(root, &"a".repeat(64), b"orphan!!").unwrap();
    let temp_dir = root.join("blobs/tmp");
    std::fs::create_dir_all(&temp_dir).unwrap();
    let old_temp = temp_dir.join(".old.tmp");
    std::fs::write(&old_temp, b"old temp").unwrap();
    let fresh = root.join("blobs/fresh");
    std::fs::write(&fresh, b"fresh!!!").unwrap();
    let now = chrono::Utc::now() + chrono::Duration::days(31);
    std::fs::File::options()
        .write(true)
        .open(&fresh)
        .unwrap()
        .set_modified(now.into())
        .unwrap();
    assert_eq!(
        s.state
            .disk_budget
            .gc(&s.state.pool, &pool, root, now)
            .await
            .unwrap(),
        1
    );
    assert!(!old_temp.exists());
    assert!(fresh.exists());
    assert!(
        !porchlight_api::blobs::blob_path(root, &"a".repeat(64))
            .unwrap()
            .exists()
    );
    let app = porchlight_api::routes::mtls_router(s.state.clone()).layer(Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(d.fingerprint),
    }));
    let path = format!("/v1/items/{}/attachments", item["id"].as_str().unwrap());
    assert_eq!(
        app.clone()
            .oneshot(upload(&d, &path, "reclaimed", b"1234567890123456"))
            .await
            .unwrap()
            .status(),
        201,
        "GC and upload temp cleanup must release their actual physical bytes"
    );
    assert_eq!(
        app.oneshot(upload(&d, &path, "fresh-counted", b"x"))
            .await
            .unwrap()
            .status(),
        507,
        "fresh unknown files must remain charged after GC"
    );
    assert_eq!(
        s.state.disk_budget.scan_count(),
        2,
        "only startup and GC reconcile"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn cancelled_staging_releases_bytes_without_a_scan(pool: PgPool) {
    let mut options = TestOptions {
        background_derivation: false,
        ..Default::default()
    };
    options.settings.attachment_quota_bytes = 8;
    let s = TestServer::start_with(&pool, options).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let item = common::post(&s, &d, "/v1/items", json!({"title":"Cancelled staging"}))
        .await
        .json();
    let app = porchlight_api::routes::mtls_router(s.state.clone()).layer(Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(d.fingerprint),
    }));
    let path = format!("/v1/items/{}/attachments", item["id"].as_str().unwrap());
    let prefix = Bytes::from_static(b"--quota\r\nContent-Disposition: form-data; name=\"file\"; filename=\"fixture.txt\"\r\nContent-Type: text/plain\r\n\r\n12345678");
    let stream = futures_util::StreamExt::chain(
        futures_util::stream::once(async { Ok::<_, std::io::Error>(prefix) }),
        futures_util::stream::pending(),
    );
    let mut request = upload(&d, &path, "cancelled", b"");
    *request.body_mut() = Body::from_stream(stream);
    let task = tokio::spawn(app.clone().oneshot(request));
    let tmp = s.state.settings.data_dir.join("blobs/tmp");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if tmp.exists()
                && std::fs::read_dir(&tmp)
                    .unwrap()
                    .any(|entry| entry.unwrap().metadata().unwrap().len() == 8)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("first staging chunk written");
    assert_eq!(
        app.clone()
            .oneshot(upload(&d, &path, "while-staged", b"x"))
            .await
            .unwrap()
            .status(),
        507
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(
        app.clone()
            .oneshot(upload(&d, &path, "after-cancel", b"abcdefgh"))
            .await
            .unwrap()
            .status(),
        201
    );
    assert_eq!(
        app.oneshot(upload(&d, &path, "canonical-counted", b"x"))
            .await
            .unwrap()
            .status(),
        507
    );
    assert_eq!(s.state.disk_budget.scan_count(), 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn large_inventory_multichunk_upload_never_rescans(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let item = common::post(&s, &d, "/v1/items", json!({"title":"Quota inventory"}))
        .await
        .json();
    let root = s.state.settings.data_dir.join("blobs");
    let inventory = root.join("inventory");
    std::fs::create_dir_all(&inventory).unwrap();
    for i in 0..20_000 {
        std::fs::write(inventory.join(format!("{i}.txt")), b"x").unwrap();
    }
    // Fresh unknown files count at startup, before any multipart admission.
    let mut settings = s.state.settings.clone();
    settings.attachment_quota_bytes = 20_000 + 64 * 4096;
    let state = AppState::new(
        s.state.pool.clone(),
        s.state.pki.clone(),
        settings,
        &[0; 32],
        s.state.hasher.clone(),
        Default::default(),
    )
    .await
    .unwrap();
    assert_eq!(state.disk_budget.scan_count(), 1);
    let app = porchlight_api::routes::mtls_router(state.clone()).layer(Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(d.fingerprint),
    }));
    let mut chunks = vec![Bytes::from_static(b"--quota\r\nContent-Disposition: form-data; name=\"file\"; filename=\"fixture.txt\"\r\nContent-Type: text/plain\r\n\r\n")];
    chunks.extend((0..64).map(|_| Bytes::from(vec![b'a'; 4096])));
    chunks.push(Bytes::from_static(b"\r\n--quota--\r\n"));
    let stream = futures_util::stream::unfold(chunks.into_iter(), |mut chunks| async {
        let chunk = chunks.next()?;
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        Some((Ok::<_, std::io::Error>(chunk), chunks))
    });
    let path = format!("/v1/items/{}/attachments", item["id"].as_str().unwrap());
    let mut request = d
        .request("POST", &path, json!({}), Some("multichunk"))
        .map(|_| Body::from_stream(stream));
    request.headers_mut().insert(
        "content-type",
        "multipart/form-data; boundary=quota".parse().unwrap(),
    );
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 201);
    let response = app.oneshot(upload(&d, &path, "full", b"x")).await.unwrap();
    assert_eq!(
        response.status(),
        507,
        "startup inventory and canonical bytes both count"
    );
    assert_eq!(
        state.disk_budget.scan_count(),
        1,
        "multipart chunks and canonical admission must not walk the blob inventory"
    );
}
