use crate::common;
use porchlight_api::{
    blobs,
    testing::{HouseholdFixture, TestClient, TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use tracing::instrument::WithSubscriber;
use uuid::Uuid;

fn upload(
    d: &porchlight_api::testing::TestDevice,
    path: &str,
) -> hyper::Request<http_body_util::Full<hyper::body::Bytes>> {
    let body = b"--fix2\r\nContent-Disposition: form-data; name=\"file\"; filename=\"x.txt\"\r\nContent-Type: text/plain\r\n\r\nx\r\n--fix2--\r\n";
    let mut r = d
        .request("POST", path, json!({}), None)
        .map(|_| http_body_util::Full::new(hyper::body::Bytes::from_static(body)));
    r.headers_mut().insert(
        "content-type",
        "multipart/form-data; boundary=fix2".parse().unwrap(),
    );
    r
}

fn delete(
    d: &porchlight_api::testing::TestDevice,
    path: &str,
) -> hyper::Request<http_body_util::Full<hyper::body::Bytes>> {
    let mut r = d.request("DELETE", path, json!({}), None);
    r.headers_mut().insert("if-match", "\"1\"".parse().unwrap());
    r
}

#[test]
fn stored_names_strip_unicode_controls() {
    assert_eq!(
        blobs::sanitize_name(
            "Öl\u{202e}\u{202a}\u{2066}\u{2069}\u{200d}\u{ad}\u{feff}\u{1}.exe",
            "text/plain"
        ),
        "Öl.txt"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn raw_agent_can_update_token_attributed_rows(pool: PgPool) {
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
            d.request("POST", "/v1/items", json!({"title":"Agent"}), None),
        )
        .await
        .unwrap()
        .json();
    let item = Uuid::parse_str(item["id"].as_str().unwrap()).unwrap();
    let token:Uuid = sqlx::query_scalar("INSERT INTO porchlight.api_tokens(household_id,owner_member_id,name,token_hash,scopes,expires_at) VALUES($1,$2,'regression',$3,ARRAY['write'],now()+interval '1 day') RETURNING id").bind(s.init.household_id).bind(s.init.admin_member_id).bind(vec![2u8;32]).fetch_one(&pool).await.unwrap();
    let id:Uuid = sqlx::query_scalar("INSERT INTO porchlight.comments(household_id,item_id,author_token_id,body) VALUES($1,$2,$3,'token') RETURNING id").bind(s.init.household_id).bind(item).bind(token).fetch_one(&pool).await.unwrap();
    let hash = "a".repeat(64);
    sqlx::query("INSERT INTO porchlight.blobs(household_id,sha256,size,content_type) VALUES($1,$2,1,'text/plain')").bind(s.init.household_id).bind(&hash).execute(&pool).await.unwrap();
    let a:Uuid = sqlx::query_scalar("INSERT INTO porchlight.attachments(household_id,item_id,blob_sha256,original_name,uploader_token_id) VALUES($1,$2,$3,'token.txt',$4) RETURNING id").bind(s.init.household_id).bind(item).bind(hash).bind(token).fetch_one(&pool).await.unwrap();
    let mut agent = porchlight_db::testing::connect_as(
        &pool,
        porchlight_db::testing::Role::Agent,
        Some("agent:fix2"),
    )
    .await
    .unwrap();
    assert_eq!(sqlx::query("UPDATE porchlight.comments SET body='edited',edited_at=now(),deleted_at=now() WHERE id=$1").bind(id).execute(&mut agent).await.unwrap().rows_affected(), 1);
    assert_eq!(
        sqlx::query("UPDATE porchlight.attachments SET deleted_at=now() WHERE id=$1")
            .bind(a)
            .execute(&mut agent)
            .await
            .unwrap()
            .rows_affected(),
        1
    );
    let e = sqlx::query(
        "INSERT INTO porchlight.comments(household_id,item_id,body) VALUES($1,$2,'no identity')",
    )
    .bind(s.init.household_id)
    .bind(item)
    .execute(&mut agent)
    .await
    .unwrap_err();
    assert_eq!(
        e.as_database_error().unwrap().code().as_deref(),
        Some("PL422")
    );
    for query in [
        "UPDATE porchlight.comments SET author_token_id=NULL WHERE id=$1",
        "UPDATE porchlight.attachments SET uploader_token_id=NULL WHERE id=$1",
    ] {
        let target = if query.contains("comments") { id } else { a };
        let e = sqlx::query(query)
            .bind(target)
            .execute(&pool)
            .await
            .unwrap_err();
        assert_eq!(
            e.as_database_error().unwrap().code().as_deref(),
            Some("PL422")
        );
    }
    // SECURITY DEFINER is scoped to the guard: the agent still cannot read tokens.
    assert!(
        sqlx::query("SELECT id FROM porchlight.api_tokens")
            .execute(&mut agent)
            .await
            .is_err()
    );
    let (definer, owner, config):(bool,String,Vec<String>) = sqlx::query_as("SELECT p.prosecdef,r.rolname,p.proconfig FROM pg_proc p JOIN pg_roles r ON r.oid=p.proowner WHERE p.oid='porchlight.pl_collaboration_token_guard'::regproc").fetch_one(&pool).await.unwrap();
    assert!(definer);
    assert_eq!(owner, "porchlight_owner");
    assert!(
        config
            .iter()
            .any(|v| v == "search_path=pg_catalog, porchlight, pg_temp")
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn comment_attachment_delete_is_comment_edit_or_moderation(pool: PgPool) {
    let clock = std::sync::Arc::new(std::sync::Mutex::new(chrono::Utc::now()));
    let read = clock.clone();
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(move || *read.lock().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let s = &f.server;
    let author = &f.users[0].device;
    let editor = &f.users[1].device;
    let c = TestClient::new(&s.ca_pem, Some(author));
    let e = TestClient::new(&s.ca_pem, Some(editor));
    let admin = &f.admin.device;
    let ac = TestClient::new(&s.ca_pem, Some(admin));
    let item = e
        .request(
            s.lan,
            editor.request(
                "POST",
                "/v1/items",
                json!({"title":"Comment attachment", "visibility":"household"}),
                None,
            ),
        )
        .await
        .unwrap()
        .json();
    let comment_path = format!("/v1/items/{}/comments", item["id"].as_str().unwrap());
    let comment = c
        .request(
            s.lan,
            author.request("POST", &comment_path, json!({"body":"author"}), None),
        )
        .await
        .unwrap()
        .json();
    let path = format!(
        "/v1/comments/{}/attachments",
        comment["id"].as_str().unwrap()
    );
    let a = c
        .request(s.lan, upload(author, &path))
        .await
        .unwrap()
        .json();
    let b = c
        .request(s.lan, upload(author, &path))
        .await
        .unwrap()
        .json();
    let delete_path = format!("/v1/attachments/{}", a["id"].as_str().unwrap());
    assert_eq!(
        e.request(s.lan, delete(editor, &delete_path))
            .await
            .unwrap()
            .status,
        403
    );
    assert_eq!(
        c.request(s.lan, delete(author, &delete_path))
            .await
            .unwrap()
            .status,
        204
    );
    let version: i32 = sqlx::query_scalar("SELECT version FROM porchlight.comments WHERE id=$1")
        .bind(Uuid::parse_str(comment["id"].as_str().unwrap()).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, 4, "two attaches and one delete bump parent");
    *clock.lock().unwrap() =
        chrono::DateTime::parse_from_rfc3339(comment["created_at"].as_str().unwrap())
            .unwrap()
            .with_timezone(&chrono::Utc)
            + chrono::Duration::minutes(16);
    let path = format!("/v1/attachments/{}", b["id"].as_str().unwrap());
    assert_eq!(
        c.request(s.lan, delete(author, &path))
            .await
            .unwrap()
            .status,
        403
    );
    assert_eq!(
        ac.request(s.lan, delete(admin, &path))
            .await
            .unwrap()
            .status,
        204
    );
    let version: i32 = sqlx::query_scalar("SELECT version FROM porchlight.comments WHERE id=$1")
        .bind(Uuid::parse_str(comment["id"].as_str().unwrap()).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, 5);
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn gc_skips_held_shard_and_continues(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let now = chrono::Utc::now();
    let root = &s.state.settings.data_dir;
    for hash in ["a".repeat(64), "b".repeat(64)] {
        blobs::durable_write(root, &hash, b"old").unwrap();
        sqlx::query("INSERT INTO porchlight.blobs(household_id,sha256,size,content_type,last_referenced_at) VALUES($1,$2,3,'text/plain',$3)").bind(s.init.household_id).bind(hash).bind(now-chrono::Duration::days(31)).execute(&pool).await.unwrap();
    }
    let mut held = pool.begin().await.unwrap();
    sqlx::query("SELECT shard FROM porchlight.blob_storage_locks WHERE shard='aa' FOR UPDATE")
        .execute(&mut *held)
        .await
        .unwrap();
    let sweep = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(
            (*pool.connect_options())
                .clone()
                .options([("lock_timeout", "50ms")]),
        )
        .await
        .unwrap();
    let logs = common::WarnSink::default();
    assert_eq!(
        blobs::gc(&sweep, &pool, root, now)
            .with_subscriber(logs.dispatch())
            .await
            .unwrap(),
        1
    );
    assert!(logs.text().contains("category=\"shard_lock_timeout\""));
    assert!(blobs::blob_path(root, &"a".repeat(64)).unwrap().exists());
    assert!(!blobs::blob_path(root, &"b".repeat(64)).unwrap().exists());
    held.rollback().await.unwrap();
    assert_eq!(blobs::gc(&sweep, &pool, root, now).await.unwrap(), 1);
    sweep.close().await;
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn upload_permit_timeout_is_transient_without_pinning_pool(pool: PgPool) {
    use std::time::Duration;
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            db_pool_size: 1,
            upload_limits: blobs::UploadLimits {
                permit_wait: Duration::from_millis(400),
                ..Default::default()
            },
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
            d.request("POST", "/v1/items", json!({"title":"Queue"}), None),
        )
        .await
        .unwrap()
        .json();
    let path = format!("/v1/items/{}/attachments", item["id"].as_str().unwrap());
    let permits = s
        .state
        .ingest_slots
        .clone()
        .acquire_many_owned(2)
        .await
        .unwrap();
    let req = upload(&d, &path);
    let retry = req.headers()["idempotency-key"].clone();
    let response = c.request(s.lan, req);
    let probe = async {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let conn = tokio::time::timeout(Duration::from_millis(100), s.state.pool.acquire())
            .await
            .expect("queued upload must not pin DB connection")
            .unwrap();
        drop(conn);
    };
    let (r, ()) = tokio::join!(response, probe);
    let r = r.unwrap();
    assert_eq!(r.status, 409);
    assert_eq!(r.headers["retry-after"], "1");
    assert_eq!(
        std::fs::read_dir(s.state.settings.data_dir.join("blobs/tmp"))
            .unwrap()
            .count(),
        0
    );
    drop(permits);
    let mut req = upload(&d, &path);
    req.headers_mut().insert("idempotency-key", retry);
    assert_eq!(
        c.request(s.lan, req).await.unwrap().status,
        201,
        "timeout must not seal key"
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn upload_total_cap_and_per_device_stream_cap(pool: PgPool) {
    use http_body_util::BodyExt;
    use std::time::Duration;
    assert_eq!(
        blobs::UploadLimits::default().total,
        Duration::from_secs(600)
    );
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            upload_limits: blobs::UploadLimits {
                total: Duration::from_millis(900),
                idle: Duration::from_secs(2),
                ..Default::default()
            },
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
            d.request("POST", "/v1/items", json!({"title":"Cap"}), None),
        )
        .await
        .unwrap()
        .json();
    let path = format!("/v1/items/{}/attachments", item["id"].as_str().unwrap());
    let send = |req| async {
        let io = c.connect(s.lan).await.unwrap();
        let (mut sender, conn) =
            hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(io))
                .await
                .unwrap();
        let task = tokio::spawn(conn);
        let r = tokio::time::timeout(Duration::from_secs(2), sender.send_request(req))
            .await
            .expect("overall upload cap must terminate steady progress")
            .unwrap();
        let status = r.status().as_u16();
        let _ = r.into_body().collect().await;
        task.abort();
        status
    };
    let slow = || {
        axum::body::Body::from_stream(futures_util::stream::unfold(0, |step| async move {
            if step == 0 {
                Some((Ok::<_,std::io::Error>(hyper::body::Bytes::from_static(b"--fix2\r\nContent-Disposition: form-data; name=\"file\"; filename=\"x.txt\"\r\nContent-Type: text/plain\r\n\r\n")), 1))
            } else {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Some((Ok(hyper::body::Bytes::from_static(b"progress")), step + 1))
            }
        }))
    };
    let first = send(upload(&d, &path).map(|_| slow()));
    let second = send(upload(&d, &path).map(|_| slow()));
    let third = async {
        tokio::time::timeout(Duration::from_millis(700), async {
            loop {
                if std::fs::read_dir(s.state.settings.data_dir.join("blobs/tmp"))
                    .map_or(0, |r| r.count())
                    == 2
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("two active streams");
        let capped = c.request(s.lan, upload(&d, &path)).await.unwrap();
        assert_eq!(
            capped.status, 429,
            "third stream rejected before temp creation"
        );
        assert_eq!(capped.headers["retry-after"], "1");
        assert_eq!(capped.json()["type"], "urn:porchlight:problem:rate-limited");
        assert_eq!(
            s.state.ingest_slots.available_permits(),
            2,
            "streaming must not occupy ingest"
        );
    };
    let (a, b, ()) = tokio::join!(first, second, third);
    assert_eq!((a, b), (408, 408), "overall cap despite steady progress");
    assert_eq!(
        std::fs::read_dir(s.state.settings.data_dir.join("blobs/tmp"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        c.request(s.lan, upload(&d, &path)).await.unwrap().status,
        201,
        "stream permits returned"
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn upload_rate_limit_precedes_body_poll(pool: PgPool) {
    use http_body_util::BodyExt;
    use std::sync::Arc;
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            limits: porchlight_api::ratelimit::RateLimits {
                write_device: 1,
                ..porchlight_api::ratelimit::RateLimits::generous()
            },
            rate_clock: Arc::new({
                let now = std::time::Instant::now();
                move || now
            }),
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
            d.request("POST", "/v1/items", json!({"title":"Rate"}), None),
        )
        .await
        .unwrap()
        .json();
    let path = format!("/v1/items/{}/attachments", item["id"].as_str().unwrap());
    let body = axum::body::Body::from_stream(futures_util::stream::pending::<
        Result<hyper::body::Bytes, std::io::Error>,
    >());
    let req = upload(&d, &path).map(|_| body);
    // A real HTTP request with a never-ending body must still reject immediately.
    let io = c.connect(s.lan).await.unwrap();
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(io))
            .await
            .unwrap();
    let task = tokio::spawn(conn);
    let r = tokio::time::timeout(std::time::Duration::from_secs(1), sender.send_request(req))
        .await
        .expect("rate check before streaming")
        .unwrap();
    assert_eq!(r.status(), 429);
    let _ = r.into_body().collect().await;
    task.abort();
    // Client transport may poll the body; temp absence proves no server-side stream started.
    assert!(!s.state.settings.data_dir.join("blobs/tmp").exists());
    s.stop().await;
}
