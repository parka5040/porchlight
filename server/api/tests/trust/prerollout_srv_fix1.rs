use porchlight_api::{
    blobs,
    testing::{TestClient, TestDevice, TestOptions, TestServer, wait_blocked_behind},
};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use uuid::Uuid;

const WITHIN: Duration = Duration::from_secs(30);

fn upload(d: &TestDevice, path: &str) -> hyper::Request<http_body_util::Full<hyper::body::Bytes>> {
    let body = b"--m6\r\nContent-Disposition: form-data; name=\"file\"; filename=\"old.txt\"\r\nContent-Type: text/plain\r\n\r\nold\r\n--m6--\r\n";
    let mut r = d
        .request("POST", path, json!({}), None)
        .map(|_| http_body_util::Full::new(hyper::body::Bytes::from_static(body)));
    r.headers_mut().insert(
        "content-type",
        "multipart/form-data; boundary=m6".parse().unwrap(),
    );
    r
}

fn id(v: &serde_json::Value) -> Uuid {
    v["id"].as_str().unwrap().parse().unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn idle_gc_holds_shard_gate_until_unlink_returns(pool: PgPool) {
    const GATE: &str = "prerollout-gc-gate";
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
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let item = c
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/items",
                json!({"title":"GC cancellation"}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(item.status, 201);
    let path = format!("/v1/items/{}/attachments", id(&item.json()));
    let first = c.request(s.lan, upload(&d, &path)).await.unwrap();
    assert_eq!(first.status, 201);
    let first = id(&first.json());
    let hash: String =
        sqlx::query_scalar("SELECT blob_sha256 FROM porchlight.attachments WHERE id=$1")
            .bind(first)
            .fetch_one(&pool)
            .await
            .unwrap();
    let now = chrono::Utc::now();
    sqlx::query("UPDATE porchlight.attachments SET deleted_at=$2 WHERE id=$1")
        .bind(first)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE porchlight.blobs SET last_referenced_at=$2 WHERE sha256=$1")
        .bind(&hash)
        .bind(now - chrono::Duration::days(31))
        .execute(&pool)
        .await
        .unwrap();
    let root = s.state.settings.data_dir.clone();
    let file = blobs::blob_path(&root, &hash).unwrap();
    assert!(file.exists());

    // Production shape: a one-connection sweep pool plus a separate metadata pool.
    let sweep = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            (*pool.connect_options())
                .clone()
                .application_name(GATE)
                .options(porchlight_api::db::APP_SETTINGS.iter().copied())
                .options([("idle_in_transaction_session_timeout", "100ms")]),
        )
        .await
        .unwrap();
    let metadata_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            (*pool.connect_options())
                .clone()
                .options(porchlight_api::db::APP_SETTINGS.iter().copied()),
        )
        .await
        .unwrap();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel::<()>();
    let hook = Mutex::new(Some((entered_tx, release_rx, finished_tx)));
    let unlink: blobs::Unlink = Arc::new(move |target: &Path| {
        let (entered, release, finished) = hook.lock().unwrap().take().expect("one unlink");
        entered.send(()).unwrap();
        release
            .recv_timeout(Duration::from_secs(60))
            .expect("test releases the unlink");
        let result = std::fs::remove_file(target);
        finished.send(()).unwrap();
        result
    });
    let gc = {
        let (sweep, metadata, root) = (sweep.clone(), metadata_pool.clone(), root.clone());
        tokio::spawn(
            async move { blobs::gc_with_unlink(&sweep, &metadata, &root, now, unlink).await },
        )
    };
    tokio::time::timeout(WITHIN, entered_rx)
        .await
        .unwrap()
        .unwrap();
    let deleted: bool =
        sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM porchlight.blobs WHERE sha256=$1)")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(deleted, "metadata DELETE commits before the unlink");
    let gate: i32 = sqlx::query_scalar(
        "SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND application_name=$1",
    )
    .bind(GATE)
    .fetch_one(&pool)
    .await
    .unwrap();
    // Queue the same lock as upload, without its production 5 s lock timeout.
    // An explicit database acquisition barrier, not elapsed time, proves ownership.
    let mut acquirer = porchlight_db::tx::begin(&pool).await.unwrap();
    for setting in ["SET LOCAL lock_timeout=0", "SET LOCAL statement_timeout=0"] {
        sqlx::query(setting).execute(&mut *acquirer).await.unwrap();
    }
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *acquirer)
        .await
        .unwrap();
    let shard = hash[..2].to_owned();
    let acquired = tokio::spawn(async move {
        sqlx::query("SELECT shard FROM porchlight.blob_storage_locks WHERE shard=$1 FOR UPDATE")
            .bind(shard)
            .execute(&mut *acquirer)
            .await
            .unwrap();
        acquirer
    });
    wait_blocked_behind(&pool, gate, 1, WITHIN)
        .await
        .expect("same-shard acquirer queues on the GC gate");
    gc.abort(); // The scheduler's shutdown branch drops the active tick future.
    assert!(gc.await.unwrap_err().is_cancelled());
    tokio::time::sleep(Duration::from_millis(350)).await;
    let blockers: Vec<i32> = sqlx::query_scalar("SELECT pg_blocking_pids($1)")
        .bind(pid)
        .fetch_one(&pool)
        .await
        .unwrap();
    // Release before assertions so even a regression cannot strand the unlink thread.
    release_tx.send(()).unwrap();
    tokio::time::timeout(WITHIN, finished_rx)
        .await
        .unwrap()
        .unwrap();
    let acquirer = tokio::time::timeout(WITHIN, acquired)
        .await
        .unwrap()
        .unwrap();
    acquirer.commit().await.unwrap();
    assert_eq!(
        blockers,
        vec![gate],
        "cancelled sweep must retain its shard gate until unlink"
    );
    let response = c.request(s.lan, upload(&d, &path)).await.unwrap();
    assert_eq!(response.status, 201);
    let download = c
        .request(
            s.lan,
            d.request(
                "GET",
                &format!("/v1/attachments/{}", id(&response.json())),
                json!(null),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(download.status, 200);
    assert_eq!(download.body.as_ref(), b"old");
    assert!(
        file.exists(),
        "cancelled GC unlinked a blob referenced after its gate was released"
    );
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.attachments WHERE blob_sha256=$1 AND deleted_at IS NULL",
    )
    .bind(&hash)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live, 1);
    let metadata: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM porchlight.blobs WHERE sha256=$1)")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(metadata);
    let idle: String = sqlx::query_scalar("SHOW idle_in_transaction_session_timeout")
        .fetch_one(&sweep)
        .await
        .unwrap();
    assert_eq!(idle, "100ms");
    sweep.close().await;
    metadata_pool.close().await;
    s.stop().await;
}
