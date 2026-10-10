//! M6 adversarial-review regressions: one read snapshot for comment collections (I1), GC
//! cancellation never releases the shard gate before its unlink returns (I2), and the comment
//! edit deadline is decided with the clock read after lock waits (M1).
use porchlight_api::{
    blobs,
    testing::{
        HouseholdFixture, TestClient, TestDevice, TestOptions, TestServer, wait_blocked_behind,
    },
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

/// I1: the ACL check and the comment SELECT share one snapshot. The revoking transaction holds
/// the comments table so the reader is parked exactly between its ACL check and its data read.
#[sqlx::test(migrations = "../db/migrations")]
async fn comment_collection_reads_acl_and_rows_in_one_snapshot(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let (owner, viewer) = (&f.users[0], &f.users[1]);
    let c = TestClient::new(&f.server.ca_pem, Some(&owner.device));
    let item = c
        .request(
            f.server.lan,
            owner.device.request(
                "POST",
                "/v1/items",
                json!({"title":"Shared","visibility":"household"}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(item.status, 201);
    let item = id(&item.json());
    let path = format!("/v1/items/{item}/comments");
    let public = c
        .request(
            f.server.lan,
            owner
                .device
                .request("POST", &path, json!({"body":"Public"}), None),
        )
        .await
        .unwrap();
    assert_eq!(public.status, 201);

    let mut revoke = pool.begin().await.unwrap();
    let holder: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *revoke)
        .await
        .unwrap();
    // The item ACL check does not touch comments; only the comment SELECT queues here.
    sqlx::query("LOCK TABLE porchlight.comments IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *revoke)
        .await
        .unwrap();
    let reader = TestClient::new(&f.server.ca_pem, Some(&viewer.device));
    let req = viewer.device.request("GET", &path, json!(null), None);
    let lan = f.server.lan;
    let read = tokio::spawn(async move { reader.request(lan, req).await.unwrap() });
    wait_blocked_behind(&pool, holder, 1, WITHIN)
        .await
        .expect("comment read queues behind the revoking transaction");
    sqlx::query("UPDATE porchlight.items SET visibility='private' WHERE id=$1")
        .bind(item)
        .execute(&mut *revoke)
        .await
        .unwrap();
    sqlx::query("INSERT INTO porchlight.comments(household_id,item_id,author_id,body) VALUES($1,$2,$3,'Secret after revocation')")
        .bind(f.server.init.household_id)
        .bind(item)
        .bind(owner.member_id)
        .execute(&mut *revoke)
        .await
        .unwrap();
    revoke.commit().await.unwrap();

    let response = tokio::time::timeout(WITHIN, read).await.unwrap().unwrap();
    assert_eq!(response.status, 200);
    let page = response.json();
    let bodies: Vec<&str> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["body"].as_str().unwrap())
        .collect();
    assert_eq!(
        bodies,
        ["Public"],
        "a comment created after the revocation escaped"
    );
    let later = TestClient::new(&f.server.ca_pem, Some(&viewer.device))
        .request(
            f.server.lan,
            viewer.device.request("GET", &path, json!(null), None),
        )
        .await
        .unwrap();
    assert_eq!(later.status, 404);
    f.server.stop().await;
}

#[derive(Clone, Copy, Debug)]
enum Edit {
    /// PATCH /v1/comments/{id}, waiting on the item scope.
    Patch,
    /// POST /v1/comments/{id}/attachments, waiting on the item scope.
    Attach,
    /// POST /v1/comments/{id}/attachments, waiting on the filesystem shard gate.
    AttachShard,
    /// DELETE /v1/attachments/{id} of a comment attachment, waiting on the item scope.
    Detach,
}

/// M1: the request starts inside the inclusive window, waits on a lock while the injected clock
/// passes the deadline, and must then be refused without effects.
async fn deadline_after_lock_wait(pool: PgPool, edit: Edit) {
    let clock = Arc::new(Mutex::new(chrono::Utc::now()));
    let read_clock = clock.clone();
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(move || *read_clock.lock().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let d = &f.users[0].device;
    let c = TestClient::new(&f.server.ca_pem, Some(d));
    let item = c
        .request(
            f.server.lan,
            d.request("POST", "/v1/items", json!({"title":"Deadline"}), None),
        )
        .await
        .unwrap();
    assert_eq!(item.status, 201);
    let item = id(&item.json());
    let comment = c
        .request(
            f.server.lan,
            d.request(
                "POST",
                &format!("/v1/items/{item}/comments"),
                json!({"body":"Original"}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(comment.status, 201);
    let comment: porchlight_api::collaboration::Comment =
        serde_json::from_slice(&comment.body).unwrap();
    let attachments = format!("/v1/comments/{}/attachments", comment.id);
    let req = match edit {
        Edit::Patch => {
            let mut r = d.request(
                "PATCH",
                &format!("/v1/comments/{}", comment.id),
                json!({"body":"Late"}),
                None,
            );
            r.headers_mut().insert("if-match", "1".parse().unwrap());
            r
        }
        Edit::Attach | Edit::AttachShard => upload(d, &attachments),
        Edit::Detach => {
            let a = c
                .request(f.server.lan, upload(d, &attachments))
                .await
                .unwrap();
            assert_eq!(a.status, 201);
            let a = id(&a.json());
            let mut r = d.request("DELETE", &format!("/v1/attachments/{a}"), json!({}), None);
            r.headers_mut().insert("if-match", "1".parse().unwrap());
            r
        }
    };
    let deadline = comment.created_at + chrono::Duration::minutes(15);
    *clock.lock().unwrap() = deadline - chrono::Duration::seconds(1);

    let mut held = pool.begin().await.unwrap();
    let holder: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let lock = match edit {
        // Every shard row: the upload's hash shard queues here after the item scope.
        Edit::AttachShard => {
            sqlx::query("SELECT shard FROM porchlight.blob_storage_locks ORDER BY shard FOR UPDATE")
        }
        _ => {
            sqlx::query("SELECT id FROM porchlight.items WHERE id=$1 FOR NO KEY UPDATE").bind(item)
        }
    };
    lock.execute(&mut *held).await.unwrap();
    let lan = f.server.lan;
    let sent = tokio::spawn(async move { c.request(lan, req).await.unwrap() });
    wait_blocked_behind(&pool, holder, 1, WITHIN)
        .await
        .expect("edit queues behind the held lock");
    *clock.lock().unwrap() = deadline + chrono::Duration::seconds(3);
    held.rollback().await.unwrap();

    let response = tokio::time::timeout(WITHIN, sent).await.unwrap().unwrap();
    assert_eq!(
        response.status,
        403,
        "{edit:?} decided the deadline with a pre-wait clock: {}",
        String::from_utf8_lossy(&response.body)
    );
    assert_eq!(
        response.json()["type"],
        "urn:porchlight:problem:comment-edit-expired"
    );
    let (body, version): (String, i64) =
        sqlx::query_as("SELECT body, version::bigint FROM porchlight.comments WHERE id=$1")
            .bind(comment.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(body, "Original");
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.attachments WHERE comment_id=$1 AND deleted_at IS NULL",
    )
    .bind(comment.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    // Detach: the earlier in-window upload bumped the comment; the refused delete changes nothing.
    let expected = if matches!(edit, Edit::Detach) {
        (1, 2)
    } else {
        (0, 1)
    };
    assert_eq!((live, version), expected);
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn comment_patch_decides_deadline_after_scope_wait(pool: PgPool) {
    deadline_after_lock_wait(pool, Edit::Patch).await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn comment_attach_decides_deadline_after_scope_wait(pool: PgPool) {
    deadline_after_lock_wait(pool, Edit::Attach).await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn comment_attach_decides_deadline_after_shard_gate_wait(pool: PgPool) {
    deadline_after_lock_wait(pool, Edit::AttachShard).await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn comment_detach_decides_deadline_after_scope_wait(pool: PgPool) {
    deadline_after_lock_wait(pool, Edit::Detach).await;
}

/// I2: GC has committed the metadata DELETE of an unreferenced hash and its unlink is in flight
/// when the sweep future is dropped (scheduler shutdown) while a same-hash upload waits on the
/// shard gate. The gate must stay held until the unlink returns, so the upload's bytes survive.
#[sqlx::test(migrations = "../db/migrations")]
async fn cancelled_gc_holds_shard_gate_until_unlink_returns(pool: PgPool) {
    const GATE: &str = "m6fix-gc-gate";
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
        .connect_with((*pool.connect_options()).clone().application_name(GATE))
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
        let (sweep, metadata, root) = (sweep.clone(), pool.clone(), root.clone());
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
    let req = upload(&d, &path);
    let lan = s.lan;
    let mut again = tokio::spawn(async move { c.request(lan, req).await.unwrap() });
    wait_blocked_behind(&pool, gate, 1, WITHIN)
        .await
        .expect("same-hash upload queues on the GC shard gate");

    gc.abort(); // The scheduler's shutdown branch drops the active tick future.
    assert!(gc.await.unwrap_err().is_cancelled());
    // A dropped gate rolls back and returns the sweep pool's only connection. The fix keeps the
    // gate (and that connection) in an owned task until the unlink returns, so this probe stays
    // pending; it is far below the upload's 5 s lock_timeout.
    let early = match tokio::time::timeout(Duration::from_secs(2), sweep.acquire()).await {
        Ok(conn) => {
            drop(conn.unwrap());
            // Released gate: the upload commits a live reference before the pending unlink.
            Some(
                tokio::time::timeout(WITHIN, &mut again)
                    .await
                    .unwrap()
                    .unwrap(),
            )
        }
        Err(_) => {
            assert!(!again.is_finished(), "upload must wait for the GC gate");
            None
        }
    };
    release_tx.send(()).unwrap();
    tokio::time::timeout(WITHIN, finished_rx)
        .await
        .unwrap()
        .unwrap();
    let response = match early {
        Some(r) => r,
        None => tokio::time::timeout(WITHIN, again).await.unwrap().unwrap(),
    };
    assert_eq!(response.status, 201);
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
    sweep.close().await;
    s.stop().await;
}
