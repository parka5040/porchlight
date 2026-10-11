use crate::common;
use porchlight_api::{
    collaboration::parse_mentions,
    testing::{TestClient, TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use tracing::instrument::WithSubscriber;
use uuid::Uuid;

fn upload(
    d: &porchlight_api::testing::TestDevice,
    path: &str,
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> hyper::Request<http_body_util::Full<hyper::body::Bytes>> {
    let mut body = format!("--fix1\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: {mime}\r\n\r\n").into_bytes();
    body.extend(bytes);
    body.extend(b"\r\n--fix1--\r\n");
    let mut r = d
        .request("POST", path, json!({}), None)
        .map(|_| http_body_util::Full::new(body.into()));
    r.headers_mut().insert(
        "content-type",
        "multipart/form-data; boundary=fix1".parse().unwrap(),
    );
    r
}

#[test]
fn mentions_are_lenient_case_insensitive_and_longest() {
    let alice = Uuid::now_v7();
    let smith = Uuid::now_v7();
    let members = [(alice, "Alice".into()), (smith, "Alice Smith".into())];
    for body in [
        "milk @ 5pm",
        "@home",
        "@everyone",
        "email alice@example.org",
        "@AliceSmith",
    ] {
        assert!(parse_mentions(body, &members).unwrap().is_empty(), "{body}");
    }
    assert_eq!(parse_mentions("@aLiCe", &members).unwrap(), vec![alice]);
    assert_eq!(
        parse_mentions("@alice smith!", &members).unwrap(),
        vec![smith]
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn gif_and_check_equal_k6_emitted_types(pool: PgPool) {
    // Derive from the exhaustive MIME match in K6, not a hand-maintained API list.
    let core = include_str!("../../../core/src/ingest.rs");
    let mime_match = core
        .split("fn mime(self)")
        .nth(1)
        .unwrap()
        .split("fn format(self)")
        .next()
        .unwrap();
    let mut emitted: Vec<_> = mime_match
        .lines()
        .filter_map(|line| {
            line.split("=> \"")
                .nth(1)
                .map(|s| s.split('"').next().unwrap().to_owned())
        })
        .collect();
    emitted.sort();
    let check: String = sqlx::query_scalar("SELECT pg_get_constraintdef(oid) FROM pg_constraint WHERE conrelid='porchlight.blobs'::regclass AND conname='blobs_content_type_check'").fetch_one(&pool).await.unwrap();
    let mut admitted: Vec<_> = check
        .split('\'')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s.to_owned())
        .collect();
    admitted.sort();
    assert_eq!(admitted, emitted);
    for mime in emitted {
        assert!(
            !porchlight_api::blobs::sanitize_name("file", &mime).ends_with(".txt")
                || mime == "text/plain"
        );
    }
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
            d.request("POST", "/v1/items", json!({"title":"GIF"}), None),
        )
        .await
        .unwrap()
        .json();
    // Valid one-pixel GIF89a.
    let gif = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff\x21\xf9\x04\x01\x00\x00\x00\x00\x2c\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00\x3b";
    let r = c
        .request(
            s.lan,
            upload(
                &d,
                &format!("/v1/items/{}/attachments", item["id"].as_str().unwrap()),
                "flyer.gif",
                "image/gif",
                gif,
            ),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 201);
    let get = c
        .request(
            s.lan,
            d.request(
                "GET",
                &format!("/v1/attachments/{}", r.json()["id"].as_str().unwrap()),
                json!(null),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(get.status, 200);
    assert_eq!(get.headers["content-type"], "image/gif");
    assert!(
        get.headers["content-disposition"]
            .to_str()
            .unwrap()
            .contains("flyer.gif")
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn comment_attachment_is_author_edit_and_noop_patch(pool: PgPool) {
    let clock = std::sync::Arc::new(std::sync::Mutex::new(chrono::Utc::now()));
    let read = clock.clone();
    let f = porchlight_api::testing::HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(move || *read.lock().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let s = &f.server;
    let d = &f.admin.device;
    let c = TestClient::new(&s.ca_pem, Some(d));
    let item = c
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/items",
                json!({"title":"Comments", "visibility":"household"}),
                None,
            ),
        )
        .await
        .unwrap()
        .json();
    let comment = c
        .request(
            s.lan,
            d.request(
                "POST",
                &format!("/v1/items/{}/comments", item["id"].as_str().unwrap()),
                json!({"body":"Original"}),
                None,
            ),
        )
        .await
        .unwrap()
        .json();
    let path = format!(
        "/v1/comments/{}/attachments",
        comment["id"].as_str().unwrap()
    );
    let other = &f.users[0].device;
    let client = TestClient::new(&s.ca_pem, Some(other));
    assert_eq!(
        client
            .request(s.lan, upload(other, &path, "x.txt", "text/plain", b"x"))
            .await
            .unwrap()
            .status,
        403
    );
    let mut req = d.request(
        "PATCH",
        &format!("/v1/comments/{}", comment["id"].as_str().unwrap()),
        json!({"body":"Original"}),
        None,
    );
    req.headers_mut().insert("if-match", "1".parse().unwrap());
    let noop = c.request(s.lan, req).await.unwrap();
    assert_eq!(noop.status, 200);
    assert_eq!(noop.json(), comment);
    let r = c
        .request(s.lan, upload(d, &path, "x.txt", "text/plain", b"x"))
        .await
        .unwrap();
    assert_eq!(r.status, 201);
    let version: i32 = sqlx::query_scalar("SELECT version FROM porchlight.comments WHERE id=$1")
        .bind(Uuid::parse_str(comment["id"].as_str().unwrap()).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version, 2);
    *clock.lock().unwrap() =
        chrono::DateTime::parse_from_rfc3339(comment["created_at"].as_str().unwrap())
            .unwrap()
            .with_timezone(&chrono::Utc)
            + chrono::Duration::minutes(15)
            + chrono::Duration::microseconds(1);
    assert_eq!(
        c.request(s.lan, upload(d, &path, "late.txt", "text/plain", b"late"))
            .await
            .unwrap()
            .status,
        403
    );
    let hidden = format!("/v1/comments/{}/attachments", Uuid::now_v7());
    assert_eq!(
        c.request(s.lan, upload(d, &hidden, "x.txt", "text/plain", b"x"))
            .await
            .unwrap()
            .status,
        404
    );
    let mut req = d.request(
        "DELETE",
        &format!("/v1/comments/{}", comment["id"].as_str().unwrap()),
        json!({}),
        None,
    );
    req.headers_mut().insert("if-match", "2".parse().unwrap());
    assert_eq!(c.request(s.lan, req).await.unwrap().status, 204);
    assert_eq!(
        c.request(
            s.lan,
            d.request(
                "GET",
                &format!("/v1/attachments/{}", r.json()["id"].as_str().unwrap()),
                json!(null),
                None
            )
        )
        .await
        .unwrap()
        .status,
        404
    );
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn token_purge_preserves_collaboration(pool: PgPool) {
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
            d.request("POST", "/v1/items", json!({"title":"PAT"}), None),
        )
        .await
        .unwrap()
        .json();
    let item = Uuid::parse_str(item["id"].as_str().unwrap()).unwrap();
    let token:Uuid = sqlx::query_scalar("INSERT INTO porchlight.api_tokens(household_id,owner_member_id,name,token_hash,scopes,expires_at) VALUES($1,$2,'purge',$3,ARRAY['write'],now()+interval '1 day') RETURNING id").bind(s.init.household_id).bind(s.init.admin_member_id).bind(vec![1u8;32]).fetch_one(&pool).await.unwrap();
    let id:Uuid = sqlx::query_scalar("INSERT INTO porchlight.comments(household_id,item_id,author_token_id,body) VALUES($1,$2,$3,'PAT comment') RETURNING id").bind(s.init.household_id).bind(item).bind(token).fetch_one(&pool).await.unwrap();
    let hash = "b".repeat(64);
    sqlx::query("INSERT INTO porchlight.blobs(household_id,sha256,size,content_type) VALUES($1,$2,1,'text/plain')").bind(s.init.household_id).bind(&hash).execute(&pool).await.unwrap();
    let attachment:Uuid = sqlx::query_scalar("INSERT INTO porchlight.attachments(household_id,item_id,blob_sha256,original_name,uploader_token_id) VALUES($1,$2,$3,'PAT.txt',$4) RETURNING id").bind(s.init.household_id).bind(item).bind(hash).bind(token).fetch_one(&pool).await.unwrap();
    // Ordinary raw attribution edits remain forbidden, even to NULL.
    assert!(
        sqlx::query("UPDATE porchlight.comments SET author_token_id=NULL WHERE id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
    sqlx::query("DELETE FROM porchlight.api_tokens WHERE id=$1")
        .bind(token)
        .execute(&pool)
        .await
        .unwrap();
    let author: Option<Uuid> =
        sqlx::query_scalar("SELECT author_token_id FROM porchlight.comments WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(author.is_none());
    let uploader: Option<Uuid> =
        sqlx::query_scalar("SELECT uploader_token_id FROM porchlight.attachments WHERE id=$1")
            .bind(attachment)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(uploader.is_none());
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn gc_age_reference_lease_and_guard_keep_bytes(pool: PgPool) {
    let logs = common::WarnSink::default();
    gc_age_reference_lease_and_guard_keep_bytes_body(pool)
        .with_subscriber(logs.dispatch())
        .await;
    assert!(
        !logs.text().contains("there is no transaction in progress"),
        "stray transaction notice: {}",
        logs.text()
    );
}
async fn gc_age_reference_lease_and_guard_keep_bytes_body(pool: PgPool) {
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
    let hash = "a".repeat(64);
    porchlight_api::blobs::durable_write(root, &hash, b"old").unwrap();
    let path = porchlight_api::blobs::blob_path(root, &hash).unwrap();
    sqlx::query("INSERT INTO porchlight.blobs(household_id,sha256,size,content_type,last_referenced_at) VALUES($1,$2,3,'text/plain',$3)").bind(s.init.household_id).bind(&hash).bind(now-chrono::Duration::days(29)).execute(&pool).await.unwrap();
    // Own top-level connection: GC must commit metadata before unlinking bytes.
    assert_eq!(
        porchlight_api::blobs::gc(&pool, &pool, root, now)
            .await
            .unwrap(),
        0
    );
    assert!(path.exists());
    sqlx::query("UPDATE porchlight.blobs SET last_referenced_at=$1")
        .bind(now - chrono::Duration::days(31))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE FUNCTION porchlight.test_veto_blob() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test veto'; END $$; CREATE TRIGGER zz_test_veto BEFORE DELETE ON porchlight.blobs FOR EACH ROW EXECUTE FUNCTION porchlight.test_veto_blob()").execute(&pool).await.unwrap();
    assert!(
        porchlight_api::blobs::gc(&pool, &pool, root, now)
            .await
            .is_err()
    );
    assert!(path.exists(), "DB veto must leave bytes intact");
    sqlx::query("DROP TRIGGER zz_test_veto ON porchlight.blobs")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        porchlight_api::blobs::gc(&pool, &pool, root, now)
            .await
            .unwrap(),
        1
    );
    assert!(!path.exists());
    s.stop().await;
}

async fn streamed(
    c: &TestClient,
    s: &TestServer,
    req: hyper::Request<axum::body::Body>,
) -> (u16, hyper::body::Bytes) {
    use http_body_util::BodyExt;
    let io = c.connect(s.lan).await.unwrap();
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(io))
            .await
            .unwrap();
    let task = tokio::spawn(conn);
    let response =
        tokio::time::timeout(std::time::Duration::from_secs(45), sender.send_request(req))
            .await
            .unwrap()
            .unwrap();
    let status = response.status().as_u16();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    task.abort();
    (status, bytes)
}
#[sqlx::test(migrations = "../db/migrations")]
async fn upload_progress_idle_cleanup_and_permit_queue(pool: PgPool) {
    use std::time::Duration;
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
            d.request("POST", "/v1/items", json!({"title":"Slow"}), None),
        )
        .await
        .unwrap()
        .json();
    let path = format!("/v1/items/{}/attachments", item["id"].as_str().unwrap());
    let pre = b"--fix1\r\nContent-Disposition: form-data; name=\"file\"; filename=\"x.txt\"\r\nContent-Type: text/plain\r\n\r\n";
    let mut pieces = vec![(Duration::ZERO, pre.to_vec())];
    for _ in 0..11 {
        pieces.push((Duration::from_secs(1), b"steady ".to_vec()));
    }
    pieces.push((Duration::ZERO, b"\r\n--fix1--\r\n".to_vec()));
    let body = axum::body::Body::from_stream(futures_util::stream::unfold(
        pieces.into_iter(),
        |mut it| async move {
            let (delay, bytes) = it.next()?;
            tokio::time::sleep(delay).await;
            Some((Ok::<_, std::io::Error>(hyper::body::Bytes::from(bytes)), it))
        },
    ));
    let req = upload(&d, &path, "x.txt", "text/plain", b"").map(|_| body);
    assert_eq!(
        streamed(&c, &s, req).await.0,
        201,
        "steady upload beyond 10 seconds"
    );
    let stall =
        axum::body::Body::from_stream(futures_util::stream::unfold(0, move |step| async move {
            if step == 0 {
                Some((
                    Ok::<_, std::io::Error>(hyper::body::Bytes::copy_from_slice(pre)),
                    1,
                ))
            } else {
                tokio::time::sleep(Duration::from_secs(40)).await;
                None
            }
        }));
    let req = upload(&d, &path, "x.txt", "text/plain", b"").map(|_| stall);
    assert_eq!(streamed(&c, &s, req).await.0, 408);
    assert_eq!(
        std::fs::read_dir(s.state.settings.data_dir.join("blobs/tmp"))
            .unwrap()
            .count(),
        0
    );
    let permits = s
        .state
        .ingest_slots
        .clone()
        .acquire_many_owned(2)
        .await
        .unwrap();
    let req = upload(&d, &path, "queued.txt", "text/plain", b"queue").map(axum::body::Body::new);
    let release = async {
        tokio::time::sleep(Duration::from_secs(2)).await;
        drop(permits);
    };
    let (r, ()) = tokio::join!(streamed(&c, &s, req), release);
    assert_eq!(r.0, 201);
    assert_eq!(
        std::fs::read_dir(s.state.settings.data_dir.join("blobs/tmp"))
            .unwrap()
            .count(),
        0
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn live_old_blob_download_lease_and_upload_regressions(pool: PgPool) {
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
            d.request("POST", "/v1/items", json!({"title":"GC live"}), None),
        )
        .await
        .unwrap()
        .json();
    let path = format!("/v1/items/{}/attachments", item["id"].as_str().unwrap());
    let r = c
        .request(
            s.lan,
            upload(
                &d,
                &path,
                "Ölwechsel.notes.txt",
                "text/plain",
                "Ölwechsel".as_bytes(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 201);
    let id = Uuid::parse_str(r.json()["id"].as_str().unwrap()).unwrap();
    let hash: String =
        sqlx::query_scalar("SELECT blob_sha256 FROM porchlight.attachments WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let file = porchlight_api::blobs::blob_path(&s.state.settings.data_dir, &hash).unwrap();
    // Fresh ingest repairs stale foreign/orphan metadata on hash dedupe.
    sqlx::query(
        "UPDATE porchlight.blobs SET size=1,content_type='application/pdf' WHERE sha256=$1",
    )
    .bind(&hash)
    .execute(&pool)
    .await
    .unwrap();
    let refreshed = c
        .request(
            s.lan,
            upload(
                &d,
                &path,
                "Ölwechsel.notes.txt",
                "text/plain",
                "Ölwechsel".as_bytes(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(refreshed.status, 201);
    assert_eq!(refreshed.json()["content_type"], "text/plain");
    assert_eq!(refreshed.json()["size"], "Ölwechsel".len());
    let refreshed_id = refreshed.json()["id"].as_str().unwrap().to_owned();
    let mut remove = d.request(
        "DELETE",
        &format!("/v1/attachments/{refreshed_id}"),
        json!({}),
        None,
    );
    remove
        .headers_mut()
        .insert("if-match", "1".parse().unwrap());
    assert_eq!(c.request(s.lan, remove).await.unwrap().status, 204);
    let now = chrono::Utc::now();
    sqlx::query("UPDATE porchlight.blobs SET last_referenced_at=$1")
        .bind(now - chrono::Duration::days(31))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        porchlight_api::blobs::gc(&pool, &pool, &s.state.settings.data_dir, now)
            .await
            .unwrap(),
        0
    );
    assert!(file.exists());
    let get = c
        .request(
            s.lan,
            d.request("GET", &format!("/v1/attachments/{id}"), json!(null), None),
        )
        .await
        .unwrap();
    assert_eq!(get.status, 200);
    assert_eq!(get.headers["content-type"], "text/plain; charset=utf-8");
    assert_eq!(get.headers["content-security-policy"], "sandbox");
    assert!(
        get.headers["content-disposition"]
            .to_str()
            .unwrap()
            .contains("filename*=UTF-8''%C3%96lwechsel.notes.txt")
    );
    // Validate duration limits without relying on live references or HTTP.
    for duration in ["0 seconds", "-1 seconds", "25 hours"] {
        assert!(
            sqlx::query_scalar::<_, Uuid>(
                "SELECT porchlight.pl_take_blob_lease('invalid',$1::text::interval,$2)"
            )
            .bind(duration)
            .bind(now)
            .fetch_one(&pool)
            .await
            .is_err()
        );
    }
    let lease: Uuid =
        sqlx::query_scalar("SELECT porchlight.pl_take_blob_lease('pin',interval '1 hour',$1)")
            .bind(now)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut req = d.request("DELETE", &format!("/v1/attachments/{id}"), json!({}), None);
    req.headers_mut().insert("if-match", "1".parse().unwrap());
    assert_eq!(c.request(s.lan, req).await.unwrap().status, 204);
    assert_eq!(
        c.request(
            s.lan,
            d.request("GET", &format!("/v1/attachments/{id}"), json!(null), None)
        )
        .await
        .unwrap()
        .status,
        404
    );
    sqlx::query("UPDATE porchlight.blobs SET last_referenced_at=$1")
        .bind(now - chrono::Duration::days(31))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        porchlight_api::blobs::gc(&pool, &pool, &s.state.settings.data_dir, now)
            .await
            .unwrap(),
        0
    );
    assert!(file.exists());
    sqlx::query("DELETE FROM porchlight.blob_leases WHERE id=$1")
        .bind(lease)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        porchlight_api::blobs::gc(&pool, &pool, &s.state.settings.data_dir, now)
            .await
            .unwrap(),
        1
    );
    assert!(!file.exists());
    let mut first = upload(&d, &path, "one.txt", "text/plain", b"one");
    first
        .headers_mut()
        .insert("idempotency-key", "same-file-key".parse().unwrap());
    assert_eq!(c.request(s.lan, first).await.unwrap().status, 201);
    let mut different = upload(&d, &path, "one.txt", "text/plain", b"two");
    different
        .headers_mut()
        .insert("idempotency-key", "same-file-key".parse().unwrap());
    let r = c.request(s.lan, different).await.unwrap();
    assert_eq!(r.status, 422);
    assert_eq!(
        r.json()["type"],
        "urn:porchlight:problem:idempotency-key-reused"
    );
    let filename = format!("{}.txt", "x".repeat(251));
    // This is a size/admission regression, not a throughput benchmark. Scope
    // the watchdog to this maximum-size transfer, leaving ordinary requests strict.
    let large_upload =
        TestClient::new(&s.ca_pem, Some(&d)).with_timeout(std::time::Duration::from_secs(60));
    let r = large_upload
        .request(
            s.lan,
            upload(
                &d,
                &path,
                &filename,
                "text/plain",
                &vec![b'x'; porchlight_api::blobs::MAX_UPLOAD],
            ),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 201, "25 MiB plus 255-byte filename");
    assert_eq!(
        std::fs::read_dir(s.state.settings.data_dir.join("blobs/tmp"))
            .unwrap()
            .count(),
        0
    );
    struct Fixed(chrono::DateTime<chrono::Utc>);
    impl porchlight_api::scheduler::Clock for Fixed {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            self.0
        }
    }
    let scheduler =
        porchlight_api::scheduler::Scheduler::from_state(&s.state, std::sync::Arc::new(Fixed(now)))
            .await
            .unwrap();
    assert_eq!(scheduler.tick().await.unwrap(), 9); // Six built-ins plus daily blob GC.
    let jobs: Vec<String> =
        sqlx::query_scalar("SELECT name FROM porchlight.scheduler_jobs ORDER BY name")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        jobs,
        [
            "backup_stale",
            "blob_gc",
            "extend_horizon",
            "notification_dependents",
            "notification_reminders",
            "refresh_dispositions",
            "refresh_ics_sources",
            "retention",
            "schedule_suggestions"
        ]
    );
    let registered:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM porchlight.scheduler_jobs WHERE name='blob_gc' AND last_success_at IS NOT NULL)").fetch_one(&pool).await.unwrap();
    assert!(registered);
    s.stop().await;
}
