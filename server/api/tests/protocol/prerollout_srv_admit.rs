use axum::{body::Body, extract::State, response::IntoResponse};
use porchlight_api::{
    error::ApiError,
    extract::{ApiQuery, Caller},
    routes::{
        exports::{self, MemberParams},
        reads,
    },
    testing::{TestOptions, TestServer},
};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

fn caller(s: &TestServer, device: Uuid) -> Caller {
    Caller::Member {
        member_id: s.init.admin_member_id,
        user_id: Uuid::nil(),
        role: "super_admin".into(),
        device_id: device,
        session_id: Uuid::nil(),
        household_id: s.init.household_id,
        client_addr: None,
        request_id: Uuid::now_v7(),
    }
}
async fn server(pool: &PgPool) -> TestServer {
    let mut settings = porchlight_api::config::Settings::for_tests();
    settings.snapshot_wait = Duration::from_millis(40);
    TestServer::start_with(
        pool,
        TestOptions {
            settings,
            background_derivation: false,
            ..Default::default()
        },
    )
    .await
}
#[sqlx::test(migrations = "../db/migrations")]
async fn snapshot_admission_lives_with_body(pool: PgPool) {
    let s = server(&pool).await;
    let d = Uuid::now_v7();
    let first = reads::snapshot(State(s.state.clone()), caller(&s, d))
        .await
        .unwrap()
        .into_response();
    let second = reads::snapshot(State(s.state.clone()), caller(&s, d))
        .await
        .into_response();
    assert_eq!(
        second.status(),
        429,
        "unread snapshot must retain its device permit"
    );
    assert_eq!(second.headers()["retry-after"], "1");
    let other = reads::snapshot(State(s.state.clone()), caller(&s, Uuid::now_v7()))
        .await
        .unwrap()
        .into_response();
    let third = reads::snapshot(State(s.state.clone()), caller(&s, Uuid::now_v7()))
        .await
        .unwrap();
    let fourth = reads::snapshot(State(s.state.clone()), caller(&s, Uuid::now_v7()))
        .await
        .into_response();
    assert_eq!(fourth.status(), 429, "member snapshot wait timeout");
    assert_eq!(fourth.headers()["retry-after"], "2");
    let idle: i64 = sqlx::query_scalar("select count(*) from pg_stat_activity where datname=current_database() and state='idle in transaction'").fetch_one(&pool).await.unwrap();
    assert_eq!(idle, 0, "snapshot body must not retain its transaction");
    drop(first);
    let next = reads::snapshot(State(s.state.clone()), caller(&s, d))
        .await
        .unwrap()
        .into_response();
    drop((next, other, third));
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn export_download_device_cap(pool: PgPool) {
    let s = server(&pool).await;
    let d = Uuid::now_v7();
    let mut held = Vec::new();
    for _ in 0..3 {
        held.push(
            exports::member(
                State(s.state.clone()),
                caller(&s, d),
                ApiQuery(MemberParams { member_id: None }),
            )
            .await
            .unwrap(),
        );
    }
    let third = exports::member(
        State(s.state.clone()),
        caller(&s, d),
        ApiQuery(MemberParams { member_id: None }),
    )
    .await
    .into_response();
    assert_eq!(
        third.status(),
        429,
        "unread export bodies must retain download quota"
    );
    drop(held);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn ingest_defaults_to_one(pool: PgPool) {
    let s = server(&pool).await;
    assert_eq!(
        s.state.ingest_slots.available_permits(),
        1,
        "512 MiB image decode must be single flight"
    );
    s.stop().await;
}
#[test]
fn pool_timeout_is_retryable() {
    let error = ApiError::from(sqlx::Error::PoolTimedOut);
    assert_eq!(
        error.status(),
        503,
        "pool exhaustion is overload, not an internal error"
    );
    assert_eq!(error.retry_after(), Some(1));
    assert!(error.is_transient());
}
#[test]
fn ordinary_transactions_have_idle_deadline() {
    assert!(
        porchlight_api::db::APP_SETTINGS.contains(&("idle_in_transaction_session_timeout", "60s"))
    );
}
#[test]
fn projector_does_not_log_raw_errors() {
    assert!(
        !include_str!("../../src/projector.rs").contains("error = %e"),
        "projector must not log SQL error messages"
    );
}
#[test]
fn docker_context_excludes_runtime_state() {
    assert!(
        include_str!("../../../.dockerignore")
            .lines()
            .any(|l| l == "var/"),
        "runtime state must not enter image layers"
    );
}
#[sqlx::test(migrations = "../db/migrations")]
async fn readonly_calendar_rejected_before_polling_body(pool: PgPool) {
    use axum::{Extension, http::Request};
    use porchlight_api::{
        server::{ConnInfo, Transport},
        testing::trust,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use tower::ServiceExt;
    let s = server(&pool).await;
    let (_, token) = trust::seed_pat(&pool, &s, &["read"]).await;
    let polled = Arc::new(AtomicBool::new(false));
    let observed = polled.clone();
    let body = Body::from_stream(futures_util::stream::poll_fn(move |_| {
        observed.store(true, Ordering::SeqCst);
        std::task::Poll::Pending::<Option<Result<axum::body::Bytes, std::io::Error>>>
    }));
    let req = Request::post("/v1/imports/ics")
        .header("porchlight-client-version", "0.1.0")
        .header("idempotency-key", "readonly-calendar")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "text/calendar")
        .body(body)
        .unwrap();
    let response = porchlight_api::routes::agent_router(s.state.clone())
        .layer(Extension(ConnInfo {
            transport: Transport::Agent,
            client_addr: None,
            client_cert_fingerprint: None,
        }))
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        403,
        "read-only import must be denied before body extraction"
    );
    assert!(!polled.load(Ordering::SeqCst));
    s.stop().await;
}

#[tokio::test]
async fn body_frames_keep_admission_even_after_body_eof() {
    use http_body_util::BodyExt;
    let limits = porchlight_api::concurrency::Limits::new(1, Some(1));
    let (device, member) = (Uuid::now_v7(), Uuid::now_v7());
    let permit = limits.acquire(device, member).unwrap();
    let mut body =
        porchlight_api::concurrency::with_body("payload".into_response(), permit).into_body();
    let frame = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert!(body.frame().await.is_none());
    drop(body);
    assert!(
        limits.acquire(device, member).is_err(),
        "network still owns the frame"
    );
    drop(frame);
    assert!(limits.acquire(device, member).is_ok());
}
#[sqlx::test(migrations = "../db/migrations")]
async fn nested_lists_share_snapshot_admission_and_global_cap(pool: PgPool) {
    use axum::extract::{Path, Query};
    use porchlight_api::{
        list_types::{ListQuery, ListsQuery},
        routes::lists,
    };
    let s = server(&pool).await;
    let d = Uuid::now_v7();
    let first = reads::snapshot(State(s.state.clone()), caller(&s, d))
        .await
        .unwrap();
    let collection = lists::list(
        State(s.state.clone()),
        caller(&s, d),
        Query(ListsQuery::default()),
    )
    .await
    .into_response();
    assert_eq!(collection.status(), 429);
    let detail = lists::get(
        State(s.state.clone()),
        caller(&s, d),
        Path(Uuid::now_v7()),
        Query(ListQuery::default()),
    )
    .await
    .into_response();
    assert_eq!(detail.status(), 429);
    drop(first);
    let mut permits = Vec::new();
    for _ in 0..4 {
        let mut c = caller(&s, Uuid::now_v7());
        if let Caller::Member { member_id, .. } = &mut c {
            *member_id = Uuid::now_v7();
        }
        permits.push(s.state.admit_snapshot(&c).await.unwrap());
    }
    assert_eq!(
        s.state
            .admit_snapshot(&caller(&s, d))
            .await
            .err()
            .unwrap()
            .status(),
        429
    );
    drop(permits);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn attachments_share_device_and_member_download_caps(pool: PgPool) {
    use axum::extract::Path;
    use porchlight_api::{blobs, routes::collaboration};
    use sha2::{Digest, Sha256};
    let s = server(&pool).await;
    let bytes = b"attachment quota fixture";
    let hash = hex::encode(Sha256::digest(bytes));
    blobs::durable_write(&s.state.settings.data_dir, &hash, bytes).unwrap();
    let item: Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,kind,title,time_mode,starts_at,ends_at,tzid) values($1,$2,'appointment','Fixture','fixed','2026-01-01T09:00Z','2026-01-01T10:00Z','UTC') returning id")
        .bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.blobs(household_id,sha256,size,content_type) values($1,$2,$3,'text/plain')").bind(s.init.household_id).bind(&hash).bind(bytes.len() as i64).execute(&pool).await.unwrap();
    let id: Uuid = sqlx::query_scalar("insert into porchlight.attachments(household_id,item_id,blob_sha256,thumbnail_sha256,original_name,uploader_id) values($1,$2,$3,$3,'fixture.txt',$4) returning id")
        .bind(s.init.household_id).bind(item).bind(&hash).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    let devices = [Uuid::now_v7(), Uuid::now_v7()];
    let mut bodies = Vec::new();
    for d in devices {
        bodies.push(
            collaboration::download(State(s.state.clone()), caller(&s, d), Path(id))
                .await
                .unwrap(),
        );
        bodies.push(
            collaboration::thumbnail(State(s.state.clone()), caller(&s, d), Path(id))
                .await
                .unwrap(),
        );
        bodies.push(
            collaboration::thumbnail(State(s.state.clone()), caller(&s, d), Path(id))
                .await
                .unwrap(),
        );
        assert_eq!(
            collaboration::download(State(s.state.clone()), caller(&s, d), Path(id))
                .await
                .into_response()
                .status(),
            429
        );
    }
    assert_eq!(
        collaboration::download(State(s.state.clone()), caller(&s, Uuid::now_v7()), Path(id))
            .await
            .into_response()
            .status(),
        429
    );
    let other = Caller::Agent {
        token_id: Uuid::now_v7(),
        member_id: Uuid::now_v7(),
        household_id: s.init.household_id,
        label: "fixture".into(),
        scopes: vec!["admin".into()],
        request_id: Uuid::now_v7(),
    };
    assert_eq!(
        collaboration::download(State(s.state.clone()), other, Path(id))
            .await
            .unwrap()
            .status(),
        200
    );
    drop(bodies);
    assert_eq!(
        collaboration::download(State(s.state.clone()), caller(&s, devices[0]), Path(id))
            .await
            .unwrap()
            .status(),
        200
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn request_pool_exhaustion_is_bounded_and_retryable(pool: PgPool) {
    use sqlx::postgres::PgPoolOptions;
    let requests = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_millis(30))
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    let held = requests.acquire().await.unwrap();
    let start = std::time::Instant::now();
    let error = ApiError::from(requests.acquire().await.unwrap_err());
    assert_eq!(error.status(), 503);
    assert_eq!(error.retry_after(), Some(1));
    assert!(start.elapsed() < Duration::from_secs(1));
    drop(held);
    assert!(requests.acquire().await.is_ok());
}
#[sqlx::test(migrations = "../db/migrations")]
async fn calendar_member_cannot_occupy_both_global_body_slots(pool: PgPool) {
    use axum::{Extension, http::Request};
    use porchlight_api::{
        server::{ConnInfo, Transport},
        testing::trust,
    };
    use std::sync::Arc;
    use tower::ServiceExt;
    let s = server(&pool).await;
    let (_, first) = trust::seed_pat(&pool, &s, &["write"]).await;
    let (other_id, other) = trust::seed_pat(&pool, &s, &["write"]).await;
    let (_, member) = trust::user_named(&pool, &s, "sam", "Sam Rivera").await;
    sqlx::query("update porchlight.household_members set role='super_admin' where id=$1")
        .bind(member)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update porchlight.api_tokens set owner_member_id=$1 where id=$2")
        .bind(member)
        .bind(other_id)
        .execute(&pool)
        .await
        .unwrap();
    let router = porchlight_api::routes::agent_router(s.state.clone()).layer(Extension(ConnInfo {
        transport: Transport::Agent,
        client_addr: None,
        client_cert_fingerprint: None,
    }));
    let request = |token: &str, started: Arc<tokio::sync::Notify>| {
        let body = Body::from_stream(futures_util::stream::poll_fn(move |_| {
            started.notify_one();
            std::task::Poll::Pending::<Option<Result<axum::body::Bytes, std::io::Error>>>
        }));
        Request::post("/v1/imports/ics")
            .header("porchlight-client-version", "0.1.0")
            .header("idempotency-key", Uuid::now_v7().to_string())
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "text/calendar")
            .body(body)
            .unwrap()
    };
    let started = Arc::new(tokio::sync::Notify::new());
    let a = tokio::spawn(router.clone().oneshot(request(&first, started.clone())));
    tokio::time::timeout(Duration::from_secs(1), started.notified())
        .await
        .unwrap();
    let denied = router
        .clone()
        .oneshot(request(&first, Arc::new(tokio::sync::Notify::new())))
        .await
        .unwrap();
    assert_eq!(denied.status(), 429);
    let other_started = Arc::new(tokio::sync::Notify::new());
    let b = tokio::spawn(router.oneshot(request(&other, other_started.clone())));
    tokio::time::timeout(Duration::from_secs(1), other_started.notified())
        .await
        .unwrap();
    assert_eq!(
        s.state.calendar_body_slots.available_permits(),
        0,
        "other member was admitted independently"
    );
    a.abort();
    b.abort();
    let _ = a.await;
    let _ = b.await;
    assert_eq!(s.state.calendar_body_slots.available_permits(), 2);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unread_downloads_timeout_on_lan_and_post_proxy_connections(pool: PgPool) {
    use porchlight_api::{
        blobs,
        testing::{TestClient, proxy_v2_header, trust},
    };
    use porchlight_core::auth::session::Platform;
    use sha2::{Digest, Sha256};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
    };
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            max_connections: 4,
            write_stall: Duration::from_millis(500),
            ..Default::default()
        },
    )
    .await;
    let mut a = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    assert_eq!(s.login(&mut a, &s.admin_password).await.status, 200);
    let (_, member) = trust::user_named(&pool, &s, "sam", "Sam Rivera").await;
    let mut b = s.issue_device(member, Platform::Desktop).await;
    assert_eq!(s.login(&mut b, &s.admin_password).await.status, 200);
    let bytes = vec![b'x'; 16 * 1024 * 1024];
    let hash = hex::encode(Sha256::digest(&bytes));
    blobs::durable_write(&s.state.settings.data_dir, &hash, &bytes).unwrap();
    let item: Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,kind,title,time_mode,starts_at,ends_at,tzid) values($1,$2,'appointment','Fixture','fixed','2026-01-01T09:00Z','2026-01-01T10:00Z','UTC') returning id")
        .bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.blobs(household_id,sha256,size,content_type) values($1,$2,$3,'text/plain')").bind(s.init.household_id).bind(&hash).bind(bytes.len() as i64).execute(&pool).await.unwrap();
    let id: Uuid = sqlx::query_scalar("insert into porchlight.attachments(household_id,item_id,blob_sha256,original_name,uploader_id) values($1,$2,$3,'fixture.txt',$4) returning id")
        .bind(s.init.household_id).bind(item).bind(&hash).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    let path = format!("/v1/attachments/{id}");
    for (addr, proxy) in [(s.lan, false), (s.funnel, true)] {
        let prefix = if proxy {
            proxy_v2_header("192.168.77.2:1234".parse().unwrap(), addr)
        } else {
            Vec::new()
        };
        let client = TestClient::new(&s.ca_pem, Some(&a));
        let mut unread = Vec::new();
        for _ in 0..3 {
            let mut socket = TcpStream::connect(addr).await.unwrap();
            socket.write_all(&prefix).await.unwrap();
            let mut stream = client.connect_stream(socket).await.unwrap();
            let request = format!(
                "GET {path} HTTP/1.1\r\nHost: localhost\r\nPorchlight-Client-Version: 0.1.0\r\nAuthorization: Bearer {}\r\n\r\n",
                a.session.as_ref().unwrap()
            );
            stream.write_all(request.as_bytes()).await.unwrap();
            stream.flush().await.unwrap();
            unread.push(stream);
        }
        // Wait for all three handlers to own their permits, not merely for the TLS handshake.
        tokio::time::timeout(Duration::from_millis(400), async {
            loop {
                if s.state
                    .downloads
                    .acquire(a.device_id, s.init.admin_member_id)
                    .is_err()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        let denied = client
            .request_with_prefix(
                addr,
                &prefix,
                a.request("GET", &path, serde_json::Value::Null, None),
            )
            .await
            .unwrap();
        assert_eq!(denied.status, 429);
        let served = TestClient::new(&s.ca_pem, Some(&b))
            .request_with_prefix(
                addr,
                &prefix,
                b.request("GET", "/v1/members", serde_json::Value::Null, None),
            )
            .await
            .unwrap();
        assert_eq!(
            served.status, 200,
            "one member must not exhaust the listener"
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(first) = s
                    .state
                    .downloads
                    .acquire(a.device_id, s.init.admin_member_id)
                    && let Ok(second) = s
                        .state
                        .downloads
                        .acquire(a.device_id, s.init.admin_member_id)
                    && let Ok(third) = s
                        .state
                        .downloads
                        .acquire(a.device_id, s.init.admin_member_id)
                {
                    drop((first, second, third));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("stalled connection retained download or listener admission");
        for mut stream in unread {
            let mut received = Vec::new();
            let _closed =
                tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut received))
                    .await
                    .unwrap();
            assert!(
                received.len() < bytes.len(),
                "unread download completed instead of stalling"
            );
        }
    }
    s.stop().await;
}
