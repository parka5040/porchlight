use porchlight_api::testing::{TestClient, TestDevice, TestOptions, TestServer};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;

fn upload(
    d: &TestDevice,
    path: &str,
    key: &str,
    bytes: &[u8],
) -> hyper::Request<http_body_util::Full<hyper::body::Bytes>> {
    upload_mime(d, path, key, bytes, "text/plain")
}
fn upload_mime(
    d: &TestDevice,
    path: &str,
    key: &str,
    bytes: &[u8],
    mime: &str,
) -> hyper::Request<http_body_util::Full<hyper::body::Bytes>> {
    let mut body = b"--limits\r\nContent-Disposition: form-data; name=\"file\"; filename=\"fixture.txt\"\r\nContent-Type: text/plain\r\n\r\n".to_vec();
    body = String::from_utf8(body)
        .unwrap()
        .replace("text/plain", mime)
        .replace(
            "fixture.txt",
            if mime == "image/jpeg" {
                "fixture.jpg"
            } else {
                "fixture.txt"
            },
        )
        .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n--limits--\r\n");
    let mut req = d
        .request("POST", path, json!({}), Some(key))
        .map(|_| http_body_util::Full::new(hyper::body::Bytes::from(body)));
    req.headers_mut().insert(
        "content-type",
        "multipart/form-data; boundary=limits".parse().unwrap(),
    );
    req
}

#[sqlx::test(migrations = "../db/migrations")]
async fn audit_limits_household_quota_refuses_before_blob_persistence(pool: PgPool) {
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
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let item = client
        .request(
            s.lan,
            d.request("POST", "/v1/items", json!({"title":"Storage"}), None),
        )
        .await
        .unwrap()
        .json();
    let path = format!("/v1/items/{}/attachments", item["id"].as_str().unwrap());
    sqlx::query("insert into porchlight.blobs(household_id,sha256,size,content_type) select $1,lpad(to_hex(n),64,'0'),26214400,'text/plain' from generate_series(1,410) n")
        .bind(s.init.household_id).execute(&pool).await.unwrap();
    let response = client
        .request(s.lan, upload(&d, &path, "quota", b"extra bytes"))
        .await
        .unwrap();
    assert_eq!(
        response.status, 507,
        "household quota must refuse persistence"
    );
    assert!(
        response.json()["type"]
            .as_str()
            .unwrap()
            .ends_with("storage-full")
    );
    let count: i64 = sqlx::query_scalar("select count(*) from porchlight.blobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 410);
    let ingested = porchlight_core::ingest::ingest(
        b"extra bytes",
        Some("fixture.txt"),
        Some("text/plain"),
        &porchlight_core::ingest::Limits::production(),
    )
    .unwrap();
    assert!(
        !porchlight_api::blobs::blob_path(&s.state.settings.data_dir, &ingested.sha256)
            .unwrap()
            .exists()
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn audit_limits_quota_races_deduplicate_and_retain_deleted_bytes(pool: PgPool) {
    let mut options = TestOptions {
        background_derivation: false,
        ..Default::default()
    };
    options.settings.attachment_quota_bytes = 8;
    let s = TestServer::start_with(&pool, options).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let mut paths = Vec::new();
    for title in ["First", "Second"] {
        let item = client
            .request(
                s.lan,
                d.request("POST", "/v1/items", json!({"title":title}), None),
            )
            .await
            .unwrap()
            .json();
        paths.push(format!(
            "/v1/items/{}/attachments",
            item["id"].as_str().unwrap()
        ));
    }
    let (a, b) = tokio::join!(
        client.request(s.lan, upload(&d, &paths[0], "race-a", b"12345678")),
        client.request(s.lan, upload(&d, &paths[1], "race-b", b"abcdefgh")),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    let mut statuses = [a.status, b.status];
    statuses.sort();
    assert_eq!(
        statuses,
        [201, 507],
        "only one reservation fits across different items"
    );
    let bytes: &[u8] = if a.status == 201 {
        b"12345678"
    } else {
        b"abcdefgh"
    };
    let replay = client
        .request(s.lan, upload(&d, &paths[0], "dedupe", bytes))
        .await
        .unwrap();
    assert_eq!(
        replay.status, 507,
        "even duplicate uploads need quota headroom for staging before the hash is known"
    );
    let used: i64 = sqlx::query_scalar("select sum(size)::bigint from porchlight.blobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(used, 8);
    sqlx::query("update porchlight.attachments set deleted_at=now()")
        .execute(&pool)
        .await
        .unwrap();
    let refused = client
        .request(
            s.lan,
            upload(&d, &paths[0], "deleted-still-charged", b"new"),
        )
        .await
        .unwrap();
    assert_eq!(refused.status, 507);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn audit_limits_thumbnail_counts_toward_quota(pool: PgPool) {
    let bytes = include_bytes!("../../../core/tests/fixtures/progressive-camera.jpg");
    let image = porchlight_core::ingest::ingest(
        bytes,
        Some("fixture.jpg"),
        Some("image/jpeg"),
        &porchlight_core::ingest::Limits::production(),
    )
    .unwrap();
    assert_ne!(image.sha256, image.thumbnail.as_ref().unwrap().sha256);
    let mut options = TestOptions {
        background_derivation: false,
        ..Default::default()
    };
    options.settings.attachment_quota_bytes = image.bytes.len() as u64;
    let s = TestServer::start_with(&pool, options).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client =
        TestClient::new(&s.ca_pem, Some(&d)).with_timeout(std::time::Duration::from_secs(30));
    let item = client
        .request(
            s.lan,
            d.request("POST", "/v1/items", json!({"title":"Image"}), None),
        )
        .await
        .unwrap()
        .json();
    let path = format!("/v1/items/{}/attachments", item["id"].as_str().unwrap());
    let response = client
        .request(
            s.lan,
            upload_mime(&d, &path, "thumbnail", bytes, "image/jpeg"),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 507);
    assert!(
        !porchlight_api::blobs::blob_path(&s.state.settings.data_dir, &image.sha256)
            .unwrap()
            .exists()
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn audit_limits_disk_reserve_refuses_staging_without_files(pool: PgPool) {
    let mut options = TestOptions {
        background_derivation: false,
        ..Default::default()
    };
    options.settings.disk_reserve_bytes = u64::MAX;
    let s = TestServer::start_with(&pool, options).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let item = client
        .request(
            s.lan,
            d.request("POST", "/v1/items", json!({"title":"Reserve"}), None),
        )
        .await
        .unwrap()
        .json();
    let path = format!("/v1/items/{}/attachments", item["id"].as_str().unwrap());
    let response = client
        .request(s.lan, upload(&d, &path, "reserve", b"small"))
        .await
        .unwrap();
    assert_eq!(response.status, 507);
    assert_eq!(
        std::fs::read_dir(s.state.settings.data_dir.join("blobs/tmp"))
            .unwrap()
            .count(),
        0
    );
    let count: i64 = sqlx::query_scalar("select count(*) from porchlight.blobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
