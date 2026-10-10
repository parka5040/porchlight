use axum::http::Request;
use chrono::{DateTime, Utc};
use http_body_util::Full;
use hyper::{body::Bytes, service::service_fn};
use hyper_util::rt::TokioIo;
use porchlight_api::{
    calendar::{fetch::Fetcher, jobs::refresh_source},
    testing::{TestClient, TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use uuid::Uuid;
fn now() -> DateTime<Utc> {
    "2026-10-06T12:00:00Z".parse().unwrap()
}
const EVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:shared\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nSUMMARY:Busy school\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

async fn fixture(
    s: &TestServer,
    responses: Vec<(u16, &'static str, &'static str)>,
) -> (
    String,
    Fetcher,
    tokio::task::JoinHandle<()>,
    Arc<Mutex<Vec<axum::http::HeaderMap>>>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(
        porchlight_api::tls::enroll_server_config(&s.state.pki).unwrap(),
    );
    let queue = Arc::new(Mutex::new(VecDeque::from(responses)));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let task = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            let queue = queue.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                let stream = acceptor.accept(stream).await.unwrap();
                let service = service_fn(move |request: Request<hyper::body::Incoming>| {
                    seen.lock().unwrap().push(request.headers().clone());
                    let (mut status, body, location) =
                        queue.lock().unwrap().pop_front().unwrap_or((500, "", ""));
                    // Sentinel for a standards-compliant conditional origin.
                    if status == 0 {
                        status = if request.headers().contains_key("if-none-match")
                            || request.headers().contains_key("if-modified-since")
                        {
                            304
                        } else {
                            200
                        };
                    }
                    async move {
                        let mut response = hyper::Response::builder()
                            .status(status)
                            .header("content-type", "text/calendar")
                            .header("etag", "\"calendar-v1\"")
                            .header("last-modified", "Tue, 06 Oct 2026 12:00:00 GMT");
                        if !location.is_empty() {
                            response = response.header("location", location);
                        }
                        Ok::<_, std::convert::Infallible>(
                            response
                                .body(Full::new(Bytes::from_static(body.as_bytes())))
                                .unwrap(),
                        )
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls_pki_types::CertificateDer::from(
            s.state.pki.ca.cert().cert_der.clone(),
        ))
        .unwrap();
    let tls = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    (
        format!("https://127.0.0.1:{}/calendar", address.port()),
        Fetcher::local_test(tls),
        task,
        requests,
    )
}
#[sqlx::test(migrations = "../db/migrations")]
async fn https_feed_conditional_failure_removal_and_app_readonly(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            db_pool_size: 1,
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let (url, fetcher, task, seen) = fixture(
        &s,
        vec![
            (200, EVENT, ""),
            (304, "", ""),
            (500, "", ""),
            (
                200,
                "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n",
                "",
            ),
        ],
    )
    .await;
    assert_eq!(
        Fetcher::default()
            .fetch(&url, false, None, None)
            .await
            .err()
            .unwrap()
            .0,
        "ssrf_refused"
    );
    let response = client
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/feeds",
                json!({"name":"school","url":url,"target_member_id":s.init.admin_member_id}),
                Some("source"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 201);
    let source: Uuid = serde_json::from_value(response.json()["id"].clone()).unwrap();
    assert!(
        refresh_source(&s.state.pool, s.init.household_id, source, now(), &fetcher)
            .await
            .unwrap()
    );
    let item: Uuid =
        sqlx::query_scalar("select item_id from porchlight.ics_uid_map where source_id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap();
    let path = format!("/v1/items/{item}");
    let response = client
        .request(s.lan, d.request("GET", &path, json!({}), None))
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    assert!(
        !response.json()["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "edit")
    );
    let mut request = d.request(
        "PATCH",
        &path,
        json!({"title":"mutated"}),
        Some("forbidden"),
    );
    request.headers_mut().insert(
        "if-match",
        response.json()["version"].to_string().parse().unwrap(),
    );
    assert_eq!(client.request(s.lan, request).await.unwrap().status, 403);
    let mut request = porchlight_api::testing::trust::req(
        "PATCH",
        &path,
        json!({"title":"agent edit"}),
        "agent-edit",
    );
    request.headers_mut().insert(
        "if-match",
        response.json()["version"].to_string().parse().unwrap(),
    );
    let edited =
        porchlight_api::testing::agent_request(&s.agent_socket, Some(s.bootstrap_token()), request)
            .await;
    assert_eq!(edited.status, 200);
    assert_eq!(edited.json()["title"], "agent edit");
    let busy: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.member_agenda where item_id=$1 and busy is not null",
    )
    .bind(item)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(busy, 1);
    for day in 1..=3 {
        assert!(
            refresh_source(
                &s.state.pool,
                s.init.household_id,
                source,
                now() + chrono::Duration::days(day),
                &fetcher
            )
            .await
            .unwrap()
        );
        let deleted: Option<DateTime<Utc>> =
            sqlx::query_scalar("select deleted_at from porchlight.items where id=$1")
                .bind(item)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(deleted.is_some(), day == 3);
    }
    assert_eq!(
        seen.lock().unwrap()[1].get("if-none-match").unwrap(),
        "\"calendar-v1\""
    );
    assert!(seen.lock().unwrap()[1].get("if-modified-since").is_some());
    task.abort();
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn malformed_refresh_keeps_events_and_redirect_rechecks_private(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let (url, fetcher, task, _) = fixture(&s, vec![(302, "", "https://10.1.2.3/calendar")]).await;
    assert_eq!(
        fetcher
            .fetch(&url, false, None, None)
            .await
            .err()
            .unwrap()
            .0,
        "ssrf_refused"
    );
    task.abort();
    let (url, fetcher, task, _) = fixture(
        &s,
        vec![
            (200, EVENT, ""),
            (
                200,
                "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID broken\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
                "",
            ),
        ],
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let added = client
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/feeds",
                json!({"name":"bad","url":url,"target_member_id":null}),
                Some("source"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(added.status, 201);
    let source: Uuid = serde_json::from_value(added.json()["id"].clone()).unwrap();
    refresh_source(&s.state.pool, s.init.household_id, source, now(), &fetcher)
        .await
        .unwrap();
    refresh_source(
        &s.state.pool,
        s.init.household_id,
        source,
        now() + chrono::Duration::days(1),
        &fetcher,
    )
    .await
    .unwrap();
    let live: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.items where ics_source_id=$1 and deleted_at is null",
    )
    .bind(source)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live, 1);
    let status: String =
        sqlx::query_scalar("select last_status from porchlight.ics_sources where id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "failed");
    task.abort();
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn transport_caps_redirects_and_body_size(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let (url, fetcher, task, seen) = fixture(&s, vec![(302, "", "/calendar"); 4]).await;
    assert_eq!(
        fetcher
            .fetch(&url, false, None, None)
            .await
            .err()
            .unwrap()
            .0,
        "redirect_limit"
    );
    assert_eq!(seen.lock().unwrap().len(), 4);
    task.abort();
    let large = Box::leak(
        "x".repeat(porchlight_api::calendar::fetch::MAX_BYTES + 1)
            .into_boxed_str(),
    );
    let (url, fetcher, task, _) = fixture(&s, vec![(200, large, "")]).await;
    assert_eq!(
        fetcher
            .fetch(&url, false, None, None)
            .await
            .err()
            .unwrap()
            .0,
        "too_large"
    );
    task.abort();
    s.stop().await;
}

#[test]
fn url_policy_rejects_http_credentials_and_non_http_schemes() {
    use porchlight_api::calendar::fetch::validate_url;
    for value in [
        "http://example.org/cal",
        "ftp://example.org/cal",
        "https://user:pass@example.org/cal",
        "https://example.org/cal#fragment",
    ] {
        assert!(validate_url(value, false).is_err());
    }
    assert!(validate_url("http://example.org/cal", true).is_ok());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn feed_visibility_remove_readd_and_unchanged_refresh(pool: PgPool) {
    const RECURRING: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:school\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\nEXDATE:20261013T090000Z\r\nSUMMARY:School\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let (_, member) =
        porchlight_api::testing::trust::user_named(&pool, &s, "feedtarget", "Target").await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let (url, fetcher, task, _) = fixture(&s, vec![(200, RECURRING, ""); 4]).await;
    let body = json!({"name":"private feed","url":url,"target_member_id":member});
    let added = c
        .request(
            s.lan,
            d.request("POST", "/v1/feeds", body.clone(), Some("private")),
        )
        .await
        .unwrap();
    assert_eq!(added.status, 201);
    assert_eq!(added.json()["visibility"], "private");
    let source: Uuid = serde_json::from_value(added.json()["id"].clone()).unwrap();
    refresh_source(&s.state.pool, s.init.household_id, source, now(), &fetcher)
        .await
        .unwrap();
    let item: Uuid =
        sqlx::query_scalar("select item_id from porchlight.ics_uid_map where source_id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap();
    let target_d = s.session_device(member, Platform::Desktop).await;
    let target_c = TestClient::new(&s.ca_pem, Some(&target_d));
    let read = target_c
        .request(
            s.lan,
            target_d.request("GET", &format!("/v1/items/{item}"), json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(read.status, 200);
    assert_eq!(read.json()["owner_id"], json!(member));
    assert_eq!(
        c.request(
            s.lan,
            d.request("GET", &format!("/v1/items/{item}"), json!({}), None)
        )
        .await
        .unwrap()
        .status,
        404
    );
    let before: serde_json::Value = sqlx::query_scalar("select jsonb_build_object('item',(select to_jsonb(i) from porchlight.items i where id=$1),'occurrences',(select jsonb_agg(to_jsonb(o) order by id) from porchlight.item_occurrences o where item_id=$1),'revisions',(select count(*) from porchlight.item_revisions),'outbox',(select count(*) from porchlight.outbox where kind<>'ics_source.changed'))").bind(item).fetch_one(&pool).await.unwrap();
    refresh_source(
        &s.state.pool,
        s.init.household_id,
        source,
        now() + chrono::Duration::days(1),
        &fetcher,
    )
    .await
    .unwrap();
    let after: serde_json::Value = sqlx::query_scalar("select jsonb_build_object('item',(select to_jsonb(i) from porchlight.items i where id=$1),'occurrences',(select jsonb_agg(to_jsonb(o) order by id) from porchlight.item_occurrences o where item_id=$1),'revisions',(select count(*) from porchlight.item_revisions),'outbox',(select count(*) from porchlight.outbox where kind<>'ics_source.changed'))").bind(item).fetch_one(&pool).await.unwrap();
    assert_eq!(before, after);
    let other = porchlight_api::testing::agent_request(
        &s.agent_socket,
        Some(s.bootstrap_token()),
        porchlight_api::testing::trust::req(
            "POST",
            "/v1/imports/ics",
            json!({"content":EVENT,"member_id":member,"scope":"overlapping"}),
            "overlap",
        ),
    )
    .await;
    assert_eq!(other.status, 200);
    let other_item: Uuid = serde_json::from_value(other.json()["item_ids"][0].clone()).unwrap();
    let mut conn = pool.acquire().await.unwrap();
    assert!(
        !porchlight_db::derive::conflicts(&mut conn, other_item, &[member])
            .await
            .unwrap()
            .is_empty()
    );
    drop(conn);
    let version: i32 = sqlx::query_scalar("select version from porchlight.ics_sources where id=$1")
        .bind(source)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut public = body.clone();
    public["visibility"] = "household".into();

    let mut req = d.request(
        "PUT",
        &format!("/v1/feeds/{source}"),
        public,
        Some("public"),
    );
    req.headers_mut()
        .insert("if-match", version.to_string().parse().unwrap());
    let updated = c.request(s.lan, req).await.unwrap();
    assert_eq!(updated.status, 200);
    let pending: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.item_derivation where requested_gen>applied_gen",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        pending, 0,
        "source configuration leaves derived item state settled"
    );
    assert_eq!(
        c.request(
            s.lan,
            d.request("GET", &format!("/v1/items/{item}"), json!({}), None)
        )
        .await
        .unwrap()
        .status,
        200
    );
    // Visibility-only PUT changes access without changing the owner.
    let row: Uuid = sqlx::query_scalar("select owner_id from porchlight.items where id=$1")
        .bind(item)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row, member);
    let (_, third) = porchlight_api::testing::trust::user_named(&pool, &s, "third", "Third").await;
    let third_d = s.session_device(third, Platform::Desktop).await;
    let third_c = TestClient::new(&s.ca_pem, Some(&third_d));
    assert_eq!(
        third_c
            .request(
                s.lan,
                third_d.request("GET", &format!("/v1/items/{item}"), json!({}), None)
            )
            .await
            .unwrap()
            .status,
        200
    );
    let mut household_body = body.clone();
    household_body["target_member_id"] = serde_json::Value::Null;
    let mut owner_req = d.request(
        "PUT",
        &format!("/v1/feeds/{source}"),
        household_body,
        Some("owner-only"),
    );
    owner_req.headers_mut().insert(
        "if-match",
        updated.json()["version"].to_string().parse().unwrap(),
    );
    let updated = c.request(s.lan, owner_req).await.unwrap();
    assert_eq!(updated.status, 200);
    let mut req = d.request(
        "DELETE",
        &format!("/v1/feeds/{source}"),
        json!({}),
        Some("remove"),
    );
    req.headers_mut().insert(
        "if-match",
        updated.json()["version"].to_string().parse().unwrap(),
    );
    assert_eq!(c.request(s.lan, req).await.unwrap().status, 200);
    let counts: (i64,i64,i64) = sqlx::query_as("select (select count(*) from porchlight.items where id=$1 and deleted_at is null),(select count(*) from porchlight.member_agenda where item_id=$1),(select count(*) from porchlight.ics_uid_map where source_id=$2)").bind(item).bind(source).fetch_one(&pool).await.unwrap();
    assert_eq!(counts, (0, 0, 0));
    let mut conn = pool.acquire().await.unwrap();
    assert!(
        porchlight_db::derive::conflicts(&mut conn, other_item, &[member])
            .await
            .unwrap()
            .is_empty()
    );
    drop(conn);
    let readded = c
        .request(s.lan, d.request("POST", "/v1/feeds", body, Some("readd")))
        .await
        .unwrap();
    assert_eq!(readded.status, 201);
    let new_source: Uuid = serde_json::from_value(readded.json()["id"].clone()).unwrap();
    refresh_source(
        &s.state.pool,
        s.init.household_id,
        new_source,
        now(),
        &fetcher,
    )
    .await
    .unwrap();
    let live: i64 =
        sqlx::query_scalar("select count(*) from porchlight.items where deleted_at is null and ics_source_id is not null")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(live, 1);
    let household = c
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/feeds",
                json!({"name":"household","url":url,"target_member_id":null}),
                Some("household"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(household.json()["visibility"], "household");
    let household_id: Uuid = serde_json::from_value(household.json()["id"].clone()).unwrap();
    refresh_source(
        &s.state.pool,
        s.init.household_id,
        household_id,
        now(),
        &fetcher,
    )
    .await
    .unwrap();
    let household_item: Uuid = sqlx::query_scalar(
        "select item_id from porchlight.ics_uid_map where source_id=$1 and recurrence_key=''",
    )
    .bind(household_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    for (client, device) in [(&c, &d), (&target_c, &target_d)] {
        let read = client
            .request(
                s.lan,
                device.request(
                    "GET",
                    &format!("/v1/items/{household_item}"),
                    json!({}),
                    None,
                ),
            )
            .await
            .unwrap();
        assert_eq!(read.status, 200);
        assert_eq!(read.json()["owner_id"], json!(s.init.admin_member_id));
    }
    task.abort();
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn polling_batches_selection_failure_isolation_and_retry(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let (url, fetcher, task, seen) = fixture(&s, vec![(200, EVENT, ""); 55]).await;
    let mut tx = porchlight_db::tx::begin_as(&pool, &porchlight_db::tx::Actor::system())
        .await
        .unwrap();
    for index in 0..54 {
        sqlx::query("insert into porchlight.ics_sources(household_id,owner_member_id,target_member_id,kind,import_key,name,url,enabled,deleted_at,next_fetch_at) values($1,$2,$2,'feed',$3,'batch',$4,$5,$6,$7)")
            .bind(s.init.household_id).bind(s.init.admin_member_id).bind(index.to_string()).bind(if index==0 { "https://127.0.0.1:1/fail" } else { &url }).bind(index!=52).bind(if index==53 {Some(now())} else {None}).bind(if index==51 {now()+chrono::Duration::days(1)} else {now()}).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    porchlight_api::calendar::jobs::run_due(&s.state.pool, now(), &fetcher)
        .await
        .unwrap();
    let processed: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.ics_sources where last_fetch_at is not null",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(processed, 50);
    porchlight_api::calendar::jobs::run_due(&s.state.pool, now(), &fetcher)
        .await
        .unwrap();
    assert_eq!(seen.lock().unwrap().len(), 50);
    let failed: (String, chrono::DateTime<Utc>) = sqlx::query_as(
        "select last_status,next_fetch_at from porchlight.ics_sources where import_key='0'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(failed.0, "failed");
    assert!(
        failed.1 <= now() + chrono::Duration::minutes(5),
        "transport failures retry promptly"
    );
    let untouched:i64=sqlx::query_scalar("select count(*) from porchlight.ics_sources where import_key in ('51','52','53') and last_fetch_at is null").fetch_one(&pool).await.unwrap();
    assert_eq!(untouched, 3);
    task.abort();
    s.stop().await;
}

// A blocked producer proves configuration does not wait on a source row lock.
#[sqlx::test(migrations = "../db/migrations")]
async fn shipped_scheduler_uses_state_fetcher_manual_pickup_and_transient_retry(pool: PgPool) {
    use porchlight_api::scheduler::{Clock, Scheduler};
    struct Fixed;
    impl Clock for Fixed {
        fn now(&self) -> DateTime<Utc> {
            now() + chrono::Duration::minutes(1)
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://127.0.0.1:{}/calendar",
        listener.local_addr().unwrap().port()
    );
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let e = entered.clone();
    let r = release.clone();
    let server = tokio::spawn(async move {
        let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let e = e.clone();
            let r = r.clone();
            let first = first.clone();
            tokio::spawn(async move {
                let service = service_fn(move |_| {
                    let e = e.clone();
                    let r = r.clone();
                    let wait = first.swap(false, std::sync::atomic::Ordering::AcqRel);
                    async move {
                        if wait {
                            e.notify_one();
                            r.notified().await;
                        }
                        Ok::<_, std::convert::Infallible>(
                            hyper::Response::builder()
                                .header("content-type", "text/calendar")
                                .body(Full::new(Bytes::from_static(EVENT.as_bytes())))
                                .unwrap(),
                        )
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    let tls = rustls::ClientConfig::builder()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            calendar_fetcher: Fetcher::local_test(tls),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let added=c.request(s.lan,d.request("POST","/v1/feeds",json!({"name":"poll","url":url,"target_member_id":s.init.admin_member_id,"allow_http":true}),Some("poll"))).await.unwrap();
    let id: Uuid = serde_json::from_value(added.json()["id"].clone()).unwrap();
    let p = s.state.pool.clone();
    let fetcher = s.state.calendar_fetcher.clone();
    let h = s.init.household_id;
    let fetching = tokio::spawn(async move { refresh_source(&p, h, id, now(), &fetcher).await });
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    let mut req = d.request(
        "POST",
        &format!("/v1/feeds/{id}/refresh"),
        json!({}),
        Some("manual"),
    );
    req.headers_mut().insert(
        "if-match",
        added.json()["version"].to_string().parse().unwrap(),
    );
    let queued = tokio::time::timeout(std::time::Duration::from_secs(2), c.request(s.lan, req))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(queued.status, 202, "fetch must not hold the source lock");
    release.notify_one();
    let error = fetching.await.unwrap().unwrap_err();
    assert!(error.is_transient());
    let before: (Option<DateTime<Utc>>, DateTime<Utc>) = sqlx::query_as(
        "select last_fetch_at,next_fetch_at from porchlight.ics_sources where id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, (None, now()));
    let scheduler = Scheduler::from_state(&s.state, Arc::new(Fixed))
        .await
        .unwrap();
    scheduler.tick().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let status: Option<String> =
                sqlx::query_scalar("select last_status from porchlight.ics_sources where id=$1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            if status.as_deref() == Some("ok") {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let version: i32 = sqlx::query_scalar("select version from porchlight.ics_sources where id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut disabled=d.request("PUT",&format!("/v1/feeds/{id}"),json!({"name":"poll","url":url,"target_member_id":s.init.admin_member_id,"allow_http":true,"enabled":false}),Some("disable"));
    disabled
        .headers_mut()
        .insert("if-match", version.to_string().parse().unwrap());
    let disabled = c.request(s.lan, disabled).await.unwrap();
    assert_eq!(disabled.status, 200);
    let mut req = d.request(
        "POST",
        &format!("/v1/feeds/{id}/refresh"),
        json!({}),
        Some("disabled-refresh"),
    );
    req.headers_mut().insert(
        "if-match",
        disabled.json()["version"].to_string().parse().unwrap(),
    );
    assert_eq!(c.request(s.lan, req).await.unwrap().status, 422);
    server.abort();
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn streaming_cap_dns_private_and_removed_target_status(pool: PgPool) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://127.0.0.1:{}/chunked",
        listener.local_addr().unwrap().port()
    );
    let stream = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut req = [0u8; 4096];
        let count = socket.read(&mut req).await.unwrap();
        assert!(count > 0, "fixture expects a request");
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/calendar\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
        let chunk = vec![b'a'; 64 * 1024];
        for _ in 0..81 {
            if socket.write_all(b"10000\r\n").await.is_err() {
                return;
            }
            if socket.write_all(&chunk).await.is_err() {
                return;
            }
            if socket.write_all(b"\r\n").await.is_err() {
                return;
            }
        }
        let _ = socket.write_all(b"0\r\n\r\n").await;
    });
    let tls = rustls::ClientConfig::builder()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    assert_eq!(
        Fetcher::local_test(tls)
            .fetch(&url, true, None, None)
            .await
            .err()
            .unwrap()
            .0,
        "too_large"
    );
    assert_eq!(
        Fetcher::default()
            .fetch("https://localhost:443/calendar", false, None, None)
            .await
            .err()
            .unwrap()
            .0,
        "ssrf_refused"
    );
    stream.abort();
    let (_, target) =
        porchlight_api::testing::trust::user_named(&pool, &s, "removedfeedtarget", "Target").await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let added=c.request(s.lan,d.request("POST","/v1/feeds",json!({"name":"removed member","url":"https://example.invalid/calendar","target_member_id":target}),Some("removed-member"))).await.unwrap();
    assert_eq!(added.status, 201);
    let id: Uuid = serde_json::from_value(added.json()["id"].clone()).unwrap();
    let mut tx = porchlight_db::tx::begin_as(&pool, &porchlight_db::tx::Actor::system())
        .await
        .unwrap();
    sqlx::query("update porchlight.household_members set deleted_at=$2 where id=$1")
        .bind(target)
        .bind(now())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    porchlight_api::calendar::jobs::run_due(&s.state.pool, now(), &s.state.calendar_fetcher)
        .await
        .unwrap();
    let status: (bool, Option<String>, Option<String>) = sqlx::query_as(
        "select enabled,last_status,last_error from porchlight.ics_sources where id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        status,
        (false, Some("failed".into()), Some("member_removed".into()))
    );
    s.stop().await;
}

async fn add_fixture_source(s: &TestServer, url: &str, target: Option<Uuid>) -> Uuid {
    let r = porchlight_api::testing::agent_request(
        &s.agent_socket,
        Some(s.bootstrap_token()),
        porchlight_api::testing::trust::req(
            "POST",
            "/v1/admin/feeds",
            json!({"name":"fix2","url":url,"target_member_id":target}),
            &Uuid::now_v7().to_string(),
        ),
    )
    .await;
    assert_eq!(r.status, 201);
    serde_json::from_value(r.json()["id"].clone()).unwrap()
}
#[sqlx::test(migrations = "../db/migrations")]
async fn failures_backoff_each_step_cap_and_success_reset(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    // Both HTTP and whole-calendar parse failures take the same exponential path.
    let (url, fetcher, task, seen) = fixture(
        &s,
        vec![
            (500, "", ""),
            (200, "not a calendar", ""),
            (500, "", ""),
            (500, "", ""),
            (500, "", ""),
            (200, EVENT, ""),
            (500, "", ""),
        ],
    )
    .await;
    let id = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
    let mut tick = now();
    for (index, minutes) in [5, 10, 20, 40, 80].into_iter().enumerate() {
        porchlight_api::calendar::jobs::run_due(&s.state.pool, tick, &fetcher)
            .await
            .unwrap();
        assert_eq!(seen.lock().unwrap().len(), index + 1);
        let next: DateTime<Utc> =
            sqlx::query_scalar("select next_fetch_at from porchlight.ics_sources where id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(next, tick + chrono::Duration::minutes(minutes));
        for t in [
            tick,
            tick + chrono::Duration::seconds(1),
            next - chrono::Duration::seconds(1),
        ] {
            porchlight_api::calendar::jobs::run_due(&s.state.pool, t, &fetcher)
                .await
                .unwrap();
        }
        assert_eq!(
            seen.lock().unwrap().len(),
            index + 1,
            "broken source fetched once per backoff step"
        );
        tick = next;
    }
    porchlight_api::calendar::jobs::run_due(&s.state.pool, tick, &fetcher)
        .await
        .unwrap();
    let next: DateTime<Utc> =
        sqlx::query_scalar("select next_fetch_at from porchlight.ics_sources where id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    porchlight_api::calendar::jobs::run_due(&s.state.pool, next, &fetcher)
        .await
        .unwrap();
    let retry: DateTime<Utc> =
        sqlx::query_scalar("select next_fetch_at from porchlight.ics_sources where id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        retry,
        next + chrono::Duration::minutes(5),
        "success resets failures"
    );
    // Saturate a short source interval without waiting in real time.
    let mut tx = porchlight_db::tx::begin_as(&pool, &porchlight_db::tx::Actor::system())
        .await
        .unwrap();
    sqlx::query("update porchlight.ics_sources set refresh_seconds=600 where id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for t in [
        retry,
        retry + chrono::Duration::minutes(10),
        retry + chrono::Duration::minutes(20),
    ] {
        porchlight_api::calendar::jobs::run_due(&s.state.pool, t, &fetcher)
            .await
            .unwrap();
        let next: DateTime<Utc> =
            sqlx::query_scalar("select next_fetch_at from porchlight.ics_sources where id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            next,
            t + chrono::Duration::minutes(10),
            "capped at interval"
        );
    }
    task.abort();
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn partial_feed_preserves_validators_and_nightly_schedule(pool: PgPool) {
    const PARTIAL: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:shared\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nSUMMARY:Busy school\r\nRRULE:FREQ=DAILY;COUNT=2\r\nRDATE:20261013T090000Z\r\nEXRULE:FREQ=DAILY\r\nEXDATE:not-a-date\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:unknown-zone\r\nDTSTART;TZID=NoSuch/Zone:20261012T090000\r\nDTEND;TZID=NoSuch/Zone:20261012T100000\r\nSUMMARY:Unknown zone\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let (url, fetcher, task, seen) = fixture(&s, vec![(200, PARTIAL, ""), (304, "", "")]).await;
    let id = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
    porchlight_api::calendar::jobs::run_due(&s.state.pool, now(), &fetcher)
        .await
        .unwrap();
    let source: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(s) from porchlight.ics_sources s where id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(source["last_status"], "partial");
    assert!(
        source["last_warnings"].to_string().contains("RDATE"),
        "warnings listed: {}",
        source["last_warnings"]
    );
    for warning in ["EXRULE", "Invalid EXDATE", "UnknownTimezone"] {
        assert!(
            source["last_warnings"].to_string().contains(warning),
            "missing warning {warning}"
        );
    }
    assert_eq!(source["etag"], "\"calendar-v1\"");
    assert!(source["last_modified"].is_string());
    let zone: String = sqlx::query_scalar("select tzid from porchlight.households where id=$1")
        .bind(s.init.household_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let zone = zone.parse().unwrap();
    let local = porchlight_core::time::to_local(now(), zone);
    let expected = porchlight_core::time::resolve_local(
        (local.date() + chrono::Duration::days(1))
            .and_hms_opt(2, 30, 0)
            .unwrap(),
        zone,
    );
    let next: DateTime<Utc> = serde_json::from_value(source["next_fetch_at"].clone()).unwrap();
    assert_eq!(next, expected);
    let items: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.items where ics_source_id=$1 and deleted_at is null",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        items, 2,
        "good event and core-normalized unknown-zone event apply"
    );
    porchlight_api::calendar::jobs::run_due(
        &s.state.pool,
        now() + chrono::Duration::minutes(5),
        &fetcher,
    )
    .await
    .unwrap();
    assert_eq!(seen.lock().unwrap().len(), 1);
    porchlight_api::calendar::jobs::run_due(&s.state.pool, next, &fetcher)
        .await
        .unwrap();
    assert_eq!(seen.lock().unwrap()[1]["if-none-match"], "\"calendar-v1\"");
    let status: String =
        sqlx::query_scalar("select last_status from porchlight.ics_sources where id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "partial", "304 must not erase outstanding warnings");
    task.abort();
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn dependent_feed_and_household_join_no_assignees(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let mut tx = porchlight_db::tx::begin_as(&pool, &porchlight_db::tx::Actor::system())
        .await
        .unwrap();
    let child:Uuid=sqlx::query_scalar("insert into porchlight.household_members(household_id,role,display_name,color,tzid) values($1,'dependent','Child','#AABBCC','America/New_York') returning id").bind(s.init.household_id).fetch_one(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let (url, fetcher, task, _) = fixture(&s, vec![(200, EVENT, ""); 3]).await;
    let dependent = add_fixture_source(&s, &url, Some(child)).await;
    let household = add_fixture_source(&s, &url, None).await;
    porchlight_api::calendar::jobs::run_due(&s.state.pool, now(), &fetcher)
        .await
        .unwrap();
    let row: (Uuid, Option<Uuid>, String) = sqlx::query_as(
        "select owner_id,subject_member_id,visibility from porchlight.items where ics_source_id=$1",
    )
    .bind(dependent)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        (s.init.admin_member_id, Some(child), "household".into())
    );
    let assignees:i64=sqlx::query_scalar("select count(*) from porchlight.item_assignees a join porchlight.items i on i.id=a.item_id where i.ics_source_id=$1").bind(household).fetch_one(&pool).await.unwrap();
    assert_eq!(assignees, 0);
    let (_, joined) =
        porchlight_api::testing::trust::user_named(&pool, &s, "latejoin", "Late Join").await;
    let d = s.session_device(joined, Platform::Desktop).await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let id: Uuid = sqlx::query_scalar("select id from porchlight.items where ics_source_id=$1")
        .bind(household)
        .fetch_one(&pool)
        .await
        .unwrap();
    let read = c
        .request(
            s.lan,
            d.request("GET", &format!("/v1/items/{id}"), json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(
        read.status, 200,
        "later member can see household feed without synchronization"
    );
    let members:Vec<Uuid>=sqlx::query_scalar("select distinct member_id from porchlight.member_agenda where item_id=$1 order by member_id").bind(id).fetch_all(&pool).await.unwrap();
    assert_eq!(
        members,
        vec![s.init.admin_member_id],
        "R2 agenda follows owner, not household visibility"
    );
    task.abort();
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn changed_feed_event_leaves_every_other_item_occurrence_unchanged(pool: PgPool) {
    const CAL: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:school\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\nEXDATE:20261013T090000Z\r\nSUMMARY:School\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:other\r\nDTSTART:20261015T090000Z\r\nDTEND:20261015T100000Z\r\nSUMMARY:Other\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let changed: &'static str = Box::leak(
        CAL.replace("SUMMARY:Other", "SUMMARY:Changed")
            .into_boxed_str(),
    );
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let (url, fetcher, task, _) = fixture(&s, vec![(200, CAL, ""), (200, changed, "")]).await;
    let source = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
    refresh_source(&s.state.pool, s.init.household_id, source, now(), &fetcher)
        .await
        .unwrap();
    async fn rows(pool: &PgPool, source: Uuid) -> serde_json::Value {
        sqlx::query_scalar("select jsonb_build_object('items',(select jsonb_agg(to_jsonb(i) order by i.id) from porchlight.items i where i.id in (select item_id from porchlight.ics_uid_map where source_id=$1 and uid='school')),'occurrences',(select jsonb_agg(to_jsonb(o) order by o.id) from porchlight.item_occurrences o where o.item_id in (select item_id from porchlight.ics_uid_map where source_id=$1 and uid='school')))").bind(source).fetch_one(pool).await.unwrap()
    }
    let before = rows(&pool, source).await;
    refresh_source(
        &s.state.pool,
        s.init.household_id,
        source,
        now() + chrono::Duration::days(1),
        &fetcher,
    )
    .await
    .unwrap();
    assert_eq!(before, rows(&pool, source).await);
    let title: String = sqlx::query_scalar(
        "select title from porchlight.items where ics_source_id=$1 and title='Changed'",
    )
    .bind(source)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(title, "Changed");
    task.abort();
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn feed_rename_is_not_rewritten_on_refresh(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let (url, fetcher, task, _) = fixture(&s, vec![(200, EVENT, ""); 2]).await;
    let source = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
    refresh_source(&s.state.pool, s.init.household_id, source, now(), &fetcher)
        .await
        .unwrap();
    let version: i32 = sqlx::query_scalar("select version from porchlight.ics_sources where id=$1")
        .bind(source)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut req = porchlight_api::testing::trust::req(
        "PUT",
        &format!("/v1/admin/feeds/{source}"),
        json!({"name":"renamed","url":url,"target_member_id":s.init.admin_member_id}),
        "rename",
    );
    req.headers_mut()
        .insert("if-match", version.to_string().parse().unwrap());
    let renamed =
        porchlight_api::testing::agent_request(&s.agent_socket, Some(s.bootstrap_token()), req)
            .await;
    assert_eq!(renamed.status, 200);
    async fn rows(pool: &PgPool, source: Uuid) -> serde_json::Value {
        sqlx::query_scalar("select jsonb_build_object('items',(select jsonb_agg(to_jsonb(i) order by i.id) from porchlight.items i where ics_source_id=$1),'occurrences',(select jsonb_agg(to_jsonb(o) order by o.id) from porchlight.item_occurrences o join porchlight.items i on i.id=o.item_id where i.ics_source_id=$1),'revisions',(select count(*) from porchlight.item_revisions),'outbox',(select count(*) from porchlight.outbox where kind<>'ics_source.changed'))").bind(source).fetch_one(pool).await.unwrap()
    }
    let before = rows(&pool, source).await;
    assert_eq!(before["items"][0]["source"], "ics:renamed");
    refresh_source(
        &s.state.pool,
        s.init.household_id,
        source,
        now() + chrono::Duration::days(1),
        &fetcher,
    )
    .await
    .unwrap();
    assert_eq!(
        before,
        rows(&pool, source).await,
        "rename does not invalidate event hashes"
    );
    task.abort();
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn fix3_retarget_plans_dependent_agenda_and_put_resets_state(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let mut tx = porchlight_db::tx::begin_as(&pool, &porchlight_db::tx::Actor::system())
        .await
        .unwrap();
    let child: Uuid = sqlx::query_scalar("insert into porchlight.household_members(household_id,role,display_name,color,tzid) values($1,'dependent','Child','#AABBCC','America/New_York') returning id").bind(s.init.household_id).fetch_one(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let (url, fetcher, task, _) = fixture(&s, vec![(200, EVENT, ""); 2]).await;
    for original in [Some(s.init.admin_member_id), None] {
        let source = add_fixture_source(&s, &url, original).await;
        refresh_source(&s.state.pool, s.init.household_id, source, now(), &fetcher)
            .await
            .unwrap();
        for (step, target) in [Some(child), original].into_iter().enumerate() {
            sqlx::query("update porchlight.ics_sources set failure_count=5,last_warnings='[{\"code\":\"InvalidEvent\",\"uid\":null,\"message\":\"old\"}]',next_fetch_at=$2 where id=$1").bind(source).bind(now()+chrono::Duration::hours(10)).execute(&pool).await.unwrap();
            let version: i32 =
                sqlx::query_scalar("select version from porchlight.ics_sources where id=$1")
                    .bind(source)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            let mut req = d.request(
                "PUT",
                &format!("/v1/feeds/{source}"),
                json!({"name":"retarget","url":url,"target_member_id":target}),
                Some(&format!("retarget-{source}-{step}")),
            );
            req.headers_mut()
                .insert("if-match", version.to_string().parse().unwrap());
            let response = c.request(s.lan, req).await.unwrap();
            assert_eq!(response.status, 200, "retarget must plan the new subject");
            assert_eq!(response.json()["failure_count"], 0);
            assert_eq!(response.json()["last_warnings"], json!([]));
            assert_eq!(response.json()["next_fetch_at"], json!(now()));
            let (item, owner, subject): (Uuid,Uuid,Option<Uuid>) = sqlx::query_as("select id,owner_id,subject_member_id from porchlight.items where ics_source_id=$1 and deleted_at is null").bind(source).fetch_one(&pool).await.unwrap();
            assert_eq!(owner, s.init.admin_member_id);
            assert_eq!(subject, target);
            let agenda: Vec<Uuid> = sqlx::query_scalar(
                "select distinct member_id from porchlight.member_agenda where item_id=$1",
            )
            .bind(item)
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(agenda, vec![target.unwrap_or(owner)]);
        }
    }
    task.abort();
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn fix3_feed_event_validation_is_partial_and_preserves_invalid_uid(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let cases = [
        format!("SUMMARY:{}", "s".repeat(201)),
        format!("DESCRIPTION:{}", "d".repeat(20001)),
        format!("LOCATION:{}", "l".repeat(501)),
        "DTEND:20261012T100000".into(),
        "SUMMARY:   ".into(),
    ];
    for bad in cases {
        let bad_event = if bad.starts_with("DTEND") {
            format!(
                "BEGIN:VEVENT\r\nUID:shared\r\nDTSTART:20261012T090000Z\r\n{bad}\r\nSUMMARY:Bad\r\nEND:VEVENT\r\n"
            )
        } else {
            format!(
                "BEGIN:VEVENT\r\nUID:shared\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\n{bad}\r\nEND:VEVENT\r\n"
            )
        };
        let body: &'static str = Box::leak(format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{bad_event}BEGIN:VEVENT\r\nUID:good\r\nDTSTART:20261013T090000Z\r\nDTEND:20261013T100000Z\r\nSUMMARY:Good\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").into_boxed_str());
        let (url, fetcher, task, _) =
            fixture(&s, vec![(200, EVENT, ""), (200, body, ""), (200, body, "")]).await;
        let source = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
        refresh_source(&s.state.pool, s.init.household_id, source, now(), &fetcher)
            .await
            .unwrap();
        let before: serde_json::Value =
            sqlx::query_scalar("select to_jsonb(i) from porchlight.items i where ics_source_id=$1")
                .bind(source)
                .fetch_one(&pool)
                .await
                .unwrap();
        refresh_source(
            &s.state.pool,
            s.init.household_id,
            source,
            now() + chrono::Duration::days(1),
            &fetcher,
        )
        .await
        .unwrap();
        let (status, count, warnings): (String,i32,serde_json::Value) = sqlx::query_as("select last_status,failure_count,last_warnings from porchlight.ics_sources where id=$1").bind(source).fetch_one(&pool).await.unwrap();
        assert_eq!(status, "partial", "one event must not fail the source");
        assert_eq!(count, 0);
        assert!(
            warnings
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w["code"] == "InvalidEvent" && w["uid"] == "shared")
        );
        let after: serde_json::Value =
            sqlx::query_scalar("select to_jsonb(i) from porchlight.items i where id=$1")
                .bind(Uuid::parse_str(before["id"].as_str().unwrap()).unwrap())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            before, after,
            "invalid UID is neither rewritten nor removed"
        );
        let good: i64 = sqlx::query_scalar("select count(*) from porchlight.items where ics_source_id=$1 and title='Good' and deleted_at is null").bind(source).fetch_one(&pool).await.unwrap();
        assert_eq!(good, 1);
        let fresh = add_fixture_source(&s, &url, None).await;
        refresh_source(&s.state.pool, s.init.household_id, fresh, now(), &fetcher)
            .await
            .unwrap();
        let (status, items): (String, i64) = sqlx::query_as("select s.last_status,(select count(*) from porchlight.items i where i.ics_source_id=s.id and i.deleted_at is null) from porchlight.ics_sources s where s.id=$1").bind(fresh).fetch_one(&pool).await.unwrap();
        assert_eq!(status, "partial");
        assert_eq!(items, 1, "a new feed applies just the good event");
        task.abort();
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn fix3_zero_duration_is_informational(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let body: &'static str = Box::leak(
        EVENT
            .replace("DTEND:20261012T100000Z", "DTEND:20261012T090000Z")
            .into_boxed_str(),
    );
    let (url, fetcher, task, _) = fixture(&s, vec![(200, body, ""), (304, "", "")]).await;
    let source = add_fixture_source(&s, &url, None).await;
    for (day, expected) in [(0, "ok"), (1, "not_modified")] {
        refresh_source(
            &s.state.pool,
            s.init.household_id,
            source,
            now() + chrono::Duration::days(day),
            &fetcher,
        )
        .await
        .unwrap();
        let (status, warnings): (String, serde_json::Value) = sqlx::query_as(
            "select last_status,last_warnings from porchlight.ics_sources where id=$1",
        )
        .bind(source)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, expected);
        assert!(
            warnings
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w["code"] == "DurationNormalized")
        );
    }
    task.abort();
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn s4b_feed_errors_daily_admin_dedupe_and_expansion_retry(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let (_, second_admin) =
        porchlight_api::testing::trust::user_named(&pool, &s, "second-admin", "Second Admin").await;
    sqlx::query("update porchlight.household_members set role='super_admin' where id=$1")
        .bind(second_admin)
        .execute(&pool)
        .await
        .unwrap();
    let (_, ordinary) =
        porchlight_api::testing::trust::user_named(&pool, &s, "ordinary", "Ordinary").await;
    // Historical unsupported MINUTELY rule exceeds the parse-wide scan count/time
    // budget before reaching the window, so no large database workload is needed.
    const LIMITED: &str = "BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:old
DTSTART:20000101T090000Z
DTEND:20000101T100000Z
RRULE:FREQ=MINUTELY;COUNT=600000;BYSECOND=0
SUMMARY:Old
END:VEVENT
END:VCALENDAR
";
    let (url, fetcher, task, seen) = fixture(
        &s,
        vec![(200, LIMITED, ""), (500, "", ""), (200, EVENT, "")],
    )
    .await;
    let source = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
    refresh_source(&s.state.pool, s.init.household_id, source, now(), &fetcher)
        .await
        .unwrap();
    let row: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(s) from porchlight.ics_sources s where id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row["last_status"], "partial");
    assert!(
        row["last_warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "ExpansionLimited")
    );
    assert!(row["etag"].is_null() && row["last_modified"].is_null());
    refresh_source(
        &s.state.pool,
        s.init.household_id,
        source,
        now() + chrono::Duration::days(1),
        &fetcher,
    )
    .await
    .unwrap();
    let events: Vec<(Uuid,serde_json::Value)> = sqlx::query_as("select recipient_member_id,payload from porchlight.notification_events where kind='feed_error' and entity_id=$1 order by created_at").bind(source).fetch_all(&pool).await.unwrap();
    assert_eq!(events.len(), 4, "partial and next-day failure both notify");
    assert!(
        events
            .iter()
            .all(|(recipient, payload)| (*recipient == s.init.admin_member_id
                || *recipient == second_admin)
                && payload["source_id"] == source.to_string())
    );
    // Repeat failure within the day deduplicates; actor/self suppression does not
    // hide operational errors. Test raw configuration/status producer as well.
    sqlx::query(
        "update porchlight.ics_sources set last_status='partial',last_fetch_at=$2 where id=$1",
    )
    .bind(source)
    .bind(now() + chrono::Duration::days(1) + chrono::Duration::minutes(1))
    .execute(&pool)
    .await
    .unwrap();
    let n:i64=sqlx::query_scalar("select count(*) from porchlight.notification_events where kind='feed_error' and entity_id=$1").bind(source).fetch_one(&pool).await.unwrap();
    assert_eq!(n, 4);
    assert!(!events.iter().any(|(recipient, _)| *recipient == ordinary));
    let second_source = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
    sqlx::query(
        "update porchlight.ics_sources set last_status='failed',last_fetch_at=$2 where id=$1",
    )
    .bind(second_source)
    .bind(now() + chrono::Duration::days(1))
    .execute(&pool)
    .await
    .unwrap();
    let other_recipients: Vec<Uuid> = sqlx::query_scalar("select recipient_member_id from porchlight.notification_events where kind='feed_error' and entity_id=$1")
        .bind(second_source).fetch_all(&pool).await.unwrap();
    assert_eq!(
        other_recipients.len(),
        2,
        "second source on the same day independently alerts both admins"
    );
    assert!(other_recipients.contains(&s.init.admin_member_id));
    assert!(other_recipients.contains(&second_admin));
    refresh_source(
        &s.state.pool,
        s.init.household_id,
        source,
        now() + chrono::Duration::days(2),
        &fetcher,
    )
    .await
    .unwrap();
    {
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[1].get("if-none-match").is_none());
        assert!(requests[1].get("if-modified-since").is_none());
        assert!(requests[2].get("if-none-match").is_none());
    }
    let status: String =
        sqlx::query_scalar("select last_status from porchlight.ics_sources where id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "ok");
    task.abort();
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn s4b_invalid_retained_override_leaves_uid_unchanged(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    const SERIES: &str = "BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:shared
DTSTART:20261012T090000Z
DTEND:20261012T100000Z
RRULE:FREQ=DAILY;COUNT=3
SUMMARY:School
END:VEVENT
END:VCALENDAR
";
    let bad: &'static str = Box::leak(
        SERIES
            .replace(
                "END:VCALENDAR",
                &format!(
                    "BEGIN:VEVENT
UID:shared
RECURRENCE-ID:20261013T090000Z
DTSTART:20261013T120000Z
DTEND:20261013T130000Z
SUMMARY:{}
END:VEVENT
END:VCALENDAR",
                    "s".repeat(201)
                ),
            )
            .into_boxed_str(),
    );
    let (url, fetcher, task, _) = fixture(&s, vec![(200, SERIES, ""), (200, bad, "")]).await;
    let source = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
    refresh_source(&s.state.pool, s.init.household_id, source, now(), &fetcher)
        .await
        .unwrap();
    let before:serde_json::Value=sqlx::query_scalar("select jsonb_build_object('item',to_jsonb(i),'occurrences',(select jsonb_agg(to_jsonb(o) order by o.seq) from porchlight.item_occurrences o where o.item_id=i.id)) from porchlight.items i where ics_source_id=$1").bind(source).fetch_one(&pool).await.unwrap();
    refresh_source(
        &s.state.pool,
        s.init.household_id,
        source,
        now() + chrono::Duration::days(1),
        &fetcher,
    )
    .await
    .unwrap();
    let after:serde_json::Value=sqlx::query_scalar("select jsonb_build_object('item',to_jsonb(i),'occurrences',(select jsonb_agg(to_jsonb(o) order by o.seq) from porchlight.item_occurrences o where o.item_id=i.id)) from porchlight.items i where ics_source_id=$1").bind(source).fetch_one(&pool).await.unwrap();
    assert_eq!(before, after);
    let (status, warnings): (String, serde_json::Value) =
        sqlx::query_as("select last_status,last_warnings from porchlight.ics_sources where id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "partial");
    assert!(
        warnings
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "InvalidEvent")
    );
    task.abort();
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn m7fix_f1_unchanged_expanded_feed_extends_rolling_horizon(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let content: &'static str = Box::leak(
        EVENT
            .replace("DTSTART:20261012T090000Z", "DTSTART:20261005T090000Z")
            .replace(
                "DTEND:20261012T100000Z",
                "DTEND:20261005T100000Z\r\nRRULE:FREQ=MONTHLY;BYDAY=MO;BYSETPOS=1",
            )
            .into_boxed_str(),
    );
    let (url, fetcher, task, seen) = fixture(&s, vec![(0, content, ""), (0, content, "")]).await;
    let source = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
    refresh_source(&s.state.pool, s.init.household_id, source, now(), &fetcher)
        .await
        .unwrap();
    // Simulate a pre-upgrade source that retained validators for ExpandedRule.
    sqlx::query("update porchlight.ics_sources set etag=$2,last_modified=$3 where id=$1")
        .bind(source)
        .bind("\"calendar-v1\"")
        .bind("Tue, 06 Oct 2026 12:00:00 GMT")
        .execute(&pool)
        .await
        .unwrap();
    let latest: chrono::DateTime<Utc> = sqlx::query_scalar(
        "select max(starts_at) from porchlight.items where ics_source_id=$1 and deleted_at is null",
    )
    .bind(source)
    .fetch_one(&pool)
    .await
    .unwrap();
    refresh_source(
        &s.state.pool,
        s.init.household_id,
        source,
        now() + chrono::Duration::days(100),
        &fetcher,
    )
    .await
    .unwrap();
    let extended: chrono::DateTime<Utc> = sqlx::query_scalar(
        "select max(starts_at) from porchlight.items where ics_source_id=$1 and deleted_at is null",
    )
    .bind(source)
    .fetch_one(&pool)
    .await
    .unwrap();
    task.abort();
    assert!(
        extended > latest,
        "conditional 304 froze horizon at {latest}; got {extended}"
    );
    assert!(!seen.lock().unwrap()[1].contains_key("if-none-match"));
    assert!(!seen.lock().unwrap()[1].contains_key("if-modified-since"));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn m7fix_f2_override_detail_loss_is_informational_and_clears(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let replacement = "BEGIN:VEVENT\r\nUID:shared\r\nRECURRENCE-ID:20261013T090000Z\r\nDTSTART:20261013T110000Z\r\nDTEND:20261013T120000Z\r\nSUMMARY:Exam\r\nDESCRIPTION:Bring pencils\r\nLOCATION:Room B\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let content: &'static str = Box::leak(
        EVENT
            .replace(
                "SUMMARY:Busy school",
                "SUMMARY:Busy school\r\nRRULE:FREQ=DAILY;COUNT=3",
            )
            .replace("END:VCALENDAR\r\n", replacement)
            .into_boxed_str(),
    );
    let (url, fetcher, task, seen) = fixture(&s, vec![(200, content, ""), (304, "", ""), (200, EVENT, ""), (200, "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:shared\r\nDTSTART:20261012T090000Z\r\nSUMMARY:   \r\nEND:VEVENT\r\nEND:VCALENDAR\r\n", ""), (200, EVENT, "")]).await;
    let source = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
    for day in 0..2 {
        refresh_source(
            &s.state.pool,
            s.init.household_id,
            source,
            now() + chrono::Duration::days(day),
            &fetcher,
        )
        .await
        .unwrap();
        let (status, warnings): (String, serde_json::Value) = sqlx::query_as(
            "select last_status,last_warnings from porchlight.ics_sources where id=$1",
        )
        .bind(source)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, if day == 0 { "ok" } else { "not_modified" });
        assert!(
            warnings
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w["code"] == "UnsupportedOverrideDetails")
        );
    }
    assert!(
        seen.lock().unwrap()[1].contains_key("if-none-match"),
        "retained series may use validators while preserving the loss warning"
    );
    let notifications: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.notification_events where kind='feed_error'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        notifications, 0,
        "informational detail loss must not notify admins"
    );
    for (day, expected) in [(2, "ok"), (3, "partial"), (4, "ok")] {
        refresh_source(
            &s.state.pool,
            s.init.household_id,
            source,
            now() + chrono::Duration::days(day),
            &fetcher,
        )
        .await
        .unwrap();
        let (status, warnings): (String, serde_json::Value) = sqlx::query_as(
            "select last_status,last_warnings from porchlight.ics_sources where id=$1",
        )
        .bind(source)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, expected);
        if expected == "ok" {
            assert_eq!(warnings, json!([]), "warnings clear with their cause");
        }
    }
    task.abort();
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn m7fix1_feed_apply_deadline_backs_off_without_content_error(pool: PgPool) {
    use porchlight_api::testing::{CalendarApplyProbe, with_calendar_apply_probe};
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let (url, fetcher, task, _) = fixture(&s, vec![(200, EVENT, ""), (200, EVENT, "")]).await;
    let source = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
    let (result, _) = with_calendar_apply_probe(
        CalendarApplyProbe {
            duration: Some(std::time::Duration::from_secs(120)),
            expire_after_write: true,
            ..Default::default()
        },
        refresh_source(&s.state.pool, s.init.household_id, source, now(), &fetcher),
    )
    .await;
    result.unwrap();
    let (status,error,count,next):(String,String,i32,DateTime<Utc>) = sqlx::query_as("select last_status,last_error,failure_count,next_fetch_at from porchlight.ics_sources where id=$1").bind(source).fetch_one(&pool).await.unwrap();
    assert_eq!(status, "retrying");
    assert_eq!(error, "apply_retryable");
    assert_eq!(count, 1);
    assert_eq!(next, now() + chrono::Duration::minutes(5));
    let (items,errors):(i64,i64) = sqlx::query_as("select (select count(*) from porchlight.items where ics_source_id=$1),(select count(*) from porchlight.notification_events where kind='feed_error')").bind(source).fetch_one(&pool).await.unwrap();
    assert_eq!(
        (items, errors),
        (0, 0),
        "deadline rolls back domain rows and is not a content alert"
    );
    refresh_source(&s.state.pool, s.init.household_id, source, next, &fetcher)
        .await
        .unwrap();
    let (status, count): (String, i32) =
        sqlx::query_as("select last_status,failure_count from porchlight.ics_sources where id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((status.as_str(), count), ("ok", 0));
    task.abort();
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn m7fix1_feed_budget_has_specific_last_error(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let events = (0..100).map(|i| format!("BEGIN:VEVENT\r\nUID:daily-{i}\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nSUMMARY:Busy\r\nRRULE:FREQ=DAILY\r\nEND:VEVENT\r\n")).collect::<String>();
    let content: &'static str = Box::leak(
        format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{events}END:VCALENDAR\r\n").into_boxed_str(),
    );
    let (url, fetcher, task, _) = fixture(&s, vec![(200, EVENT, ""), (200, content, "")]).await;
    let source = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
    for day in 0..2 {
        refresh_source(
            &s.state.pool,
            s.init.household_id,
            source,
            now() + chrono::Duration::days(day),
            &fetcher,
        )
        .await
        .unwrap();
    }
    let error: String =
        sqlx::query_scalar("select last_error from porchlight.ics_sources where id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(error, "calendar_too_large");
    let rows: i64 =
        sqlx::query_scalar("select count(*) from porchlight.items where ics_source_id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rows, 1);
    task.abort();
    s.stop().await;
}

async fn injected_feed_timeout(s: &TestServer, id: Uuid, at: DateTime<Utc>, fetcher: &Fetcher) {
    use porchlight_api::testing::{CalendarApplyProbe, with_calendar_apply_probe};
    let (result, _) = with_calendar_apply_probe(
        CalendarApplyProbe {
            expire_after_write: true,
            ..Default::default()
        },
        refresh_source(&s.state.pool, s.init.household_id, id, at, fetcher),
    )
    .await;
    result.unwrap();
}
async fn timeout_state(pool: &PgPool, id: Uuid) -> (String, Option<String>, i32, DateTime<Utc>) {
    sqlx::query_as("select last_status,last_error,apply_timeout_count,next_fetch_at from porchlight.ics_sources where id=$1").bind(id).fetch_one(pool).await.unwrap()
}
// R80: consecutive apply-deadline failures escalate whatever bytes were fetched.
fn stamped(n: u32) -> &'static str {
    // Publishers re-stamp DTSTAMP on every download; nothing else differs.
    stamped_summary(n, "Busy school")
}
fn stamped_summary(n: u32, summary: &str) -> &'static str {
    Box::leak(format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:shared\r\nDTSTAMP:20261006T12000{n}Z\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nSUMMARY:{summary}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n").into_boxed_str())
}
async fn alerts(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "select count(*) from porchlight.notification_events where kind='feed_error'",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}
#[sqlx::test(migrations = "../db/migrations")]
async fn fix3_deadline_streak_ignores_fetched_bytes_and_stays_within_check(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let (url, fetcher, task, _) =
        fixture(&s, (1..=5).map(|n| (200, stamped(n), "")).collect()).await;
    let id = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
    let mut at = now();
    for n in 1..=3 {
        injected_feed_timeout(&s, id, at, &fetcher).await;
        let (status, error, count, next) = timeout_state(&pool, id).await;
        assert_eq!(count, n);
        assert_eq!(status, if n == 3 { "failed" } else { "retrying" });
        assert_eq!(
            error.as_deref(),
            Some(if n == 3 {
                "apply_timeout"
            } else {
                "apply_retryable"
            })
        );
        assert_eq!(alerts(&pool).await, if n == 3 { 1 } else { 0 });
        at = next;
    }
    // A 4th and 5th consecutive deadline keep count at 3 (0064 CHECK) and stay failed.
    for _ in 0..2 {
        injected_feed_timeout(&s, id, at, &fetcher).await;
        let (status, error, count, next) = timeout_state(&pool, id).await;
        assert_eq!(
            (status.as_str(), error.as_deref(), count),
            ("failed", Some("apply_timeout"), 3)
        );
        at = next;
    }
    task.abort();
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn fix3_deadline_streak_reset_rules(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let (url, fetcher, task, _) = fixture(
        &s,
        vec![
            (200, stamped(1), ""),                      // deadline        -> 1
            (200, stamped(2), ""),                      // deadline        -> 2
            (200, stamped(3), ""),                      // success         -> 0
            (200, stamped_summary(4, "Changed 4"), ""), // deadline        -> 1
            (304, "", ""),                              // 304             -> 0
            (200, stamped_summary(5, "Changed 5"), ""), // deadline        -> 1
            (200, stamped_summary(6, "Changed 6"), ""), // deadline        -> 1 (after manual refresh)
        ],
    )
    .await;
    let id = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
    injected_feed_timeout(&s, id, now(), &fetcher).await;
    let next = timeout_state(&pool, id).await.3;
    injected_feed_timeout(&s, id, next, &fetcher).await;
    let (_, _, count, next) = timeout_state(&pool, id).await;
    assert_eq!(count, 2);
    refresh_source(&s.state.pool, s.init.household_id, id, next, &fetcher)
        .await
        .unwrap();
    let (status, _, count, next) = timeout_state(&pool, id).await;
    assert_eq!(
        (status.as_str(), count),
        ("ok", 0),
        "successful apply resets"
    );
    injected_feed_timeout(&s, id, next, &fetcher).await;
    let (_, _, count, next) = timeout_state(&pool, id).await;
    assert_eq!(count, 1);
    refresh_source(&s.state.pool, s.init.household_id, id, next, &fetcher)
        .await
        .unwrap();
    let (status, _, count, next) = timeout_state(&pool, id).await;
    assert_eq!((status.as_str(), count), ("not_modified", 0), "304 resets");
    injected_feed_timeout(&s, id, next, &fetcher).await;
    let (_, _, count, _) = timeout_state(&pool, id).await;
    assert_eq!(count, 1);
    // Manual refresh resets only the streak: last_error/failure_count are untouched.
    let before: (Option<String>, i32) =
        sqlx::query_as("select last_error,failure_count from porchlight.ics_sources where id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(before.0.as_deref(), Some("apply_retryable"));
    let version: i32 = sqlx::query_scalar("select version from porchlight.ics_sources where id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut req = porchlight_api::testing::trust::req(
        "POST",
        &format!("/v1/admin/feeds/{id}/refresh"),
        json!({}),
        "timeout-manual-reset",
    );
    req.headers_mut()
        .insert("if-match", version.to_string().parse().unwrap());
    let queued =
        porchlight_api::testing::agent_request(&s.agent_socket, Some(s.bootstrap_token()), req)
            .await;
    assert_eq!(queued.status, 202);
    let after: (Option<String>, i32) =
        sqlx::query_as("select last_error,failure_count from porchlight.ics_sources where id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(before, after, "manual refresh leaves error history alone");
    assert_eq!(timeout_state(&pool, id).await.2, 0, "manual refresh resets");
    injected_feed_timeout(&s, id, now(), &fetcher).await;
    let (_, _, count, _) = timeout_state(&pool, id).await;
    assert_eq!(count, 1);
    // PUT (configuration change) resets.
    let version: i32 = sqlx::query_scalar("select version from porchlight.ics_sources where id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut req = porchlight_api::testing::trust::req(
        "PUT",
        &format!("/v1/admin/feeds/{id}"),
        json!({"name":"renamed","url":url,"target_member_id":s.init.admin_member_id}),
        "timeout-put-reset",
    );
    req.headers_mut()
        .insert("if-match", version.to_string().parse().unwrap());
    let put =
        porchlight_api::testing::agent_request(&s.agent_socket, Some(s.bootstrap_token()), req)
            .await;
    assert_eq!(put.status, 200);
    assert_eq!(timeout_state(&pool, id).await.2, 0, "PUT resets");
    task.abort();
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn fix2_contention_leaves_source_untouched_for_next_poll(pool: PgPool) {
    use porchlight_api::testing::{CalendarApplyProbe, with_calendar_apply_probe};
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let (url, fetcher, task, _) = fixture(&s, vec![(200, EVENT, ""); 2]).await;
    let id = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
    let before: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(s) from porchlight.ics_sources s where id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let (result, _) = with_calendar_apply_probe(
        CalendarApplyProbe {
            contention_after_write: true,
            ..Default::default()
        },
        refresh_source(&s.state.pool, s.init.household_id, id, now(), &fetcher),
    )
    .await;
    let error = result.unwrap_err();
    assert!(error.is_transient());
    assert!(!error.is_calendar_apply_timeout());
    let after: serde_json::Value =
        sqlx::query_scalar("select to_jsonb(s) from porchlight.ics_sources s where id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        before, after,
        "no status/version/validator/backoff change on contention"
    );
    refresh_source(&s.state.pool, s.init.household_id, id, now(), &fetcher)
        .await
        .unwrap();
    assert_eq!(timeout_state(&pool, id).await.0, "ok");
    task.abort();
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn fix3_non_deadline_failures_neither_advance_nor_reset_streak(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let (url, fetcher, task, _) = fixture(
        &s,
        vec![
            (200, stamped(1), ""),
            (500, "", ""),               // transport failure
            (200, "not a calendar", ""), // invalid calendar
            (200, stamped(2), ""),
            (500, "", ""),
            (200, stamped(3), ""),
        ],
    )
    .await;
    let id = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
    injected_feed_timeout(&s, id, now(), &fetcher).await;
    let mut next = timeout_state(&pool, id).await.3;
    for _ in 0..2 {
        refresh_source(&s.state.pool, s.init.household_id, id, next, &fetcher)
            .await
            .unwrap();
        let (status, _, count, n) = timeout_state(&pool, id).await;
        assert_eq!(
            (status.as_str(), count),
            ("failed", 1),
            "non-deadline keeps streak"
        );
        next = n;
    }
    injected_feed_timeout(&s, id, next, &fetcher).await;
    let (_, _, count, n) = timeout_state(&pool, id).await;
    assert_eq!(count, 2, "non-deadline failures did not reset");
    refresh_source(&s.state.pool, s.init.household_id, id, n, &fetcher)
        .await
        .unwrap();
    let (_, _, count, n) = timeout_state(&pool, id).await;
    assert_eq!(count, 2);
    injected_feed_timeout(&s, id, n, &fetcher).await;
    let (status, error, count, _) = timeout_state(&pool, id).await;
    assert_eq!(
        (status.as_str(), error.as_deref(), count),
        ("failed", Some("apply_timeout"), 3),
        "third deadline escalates across interleaved non-deadline failures"
    );
    task.abort();
    s.stop().await;
}

/// R86: the apply deadline lands while the write scope's SAVEPOINT is in flight. sqlx then rolls
/// the whole refresh transaction back (source-row claim and actor settings gone) and leaves it in
/// autocommit, so the deadline bookkeeping must run in a fresh attributed transaction that
/// re-claims the source row, never in the poisoned one. Hand-polled and gated on the server's
/// own state: no timing assumption.
#[sqlx::test(migrations = "../db/migrations")]
async fn r86_deadline_during_savepoint_begin_keeps_bookkeeping_transactional(pool: PgPool) {
    use porchlight_api::testing::{CalendarApplyProbe, with_calendar_apply_probe};
    use std::{
        future::Future,
        sync::atomic::{AtomicBool, Ordering},
        task::{Context, Poll, Waker},
    };
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let (url, fetcher, task, _) = fixture(&s, vec![(200, EVENT, "")]).await;
    let source = add_fixture_source(&s, &url, Some(s.init.admin_member_id)).await;
    let cancel = Arc::new(AtomicBool::new(false));
    let (result, bookkeeping) = {
        let refresh = with_calendar_apply_probe(
            CalendarApplyProbe {
                cancel: Some(cancel.clone()),
                ..Default::default()
            },
            refresh_source(&s.state.pool, s.init.household_id, source, now(), &fetcher),
        );
        let mut refresh = std::pin::pin!(refresh);
        let mut cx = Context::from_waker(Waker::noop());
        // Holds back the bookkeeping write's revision insert so its transaction can be inspected.
        let mut revisions_lock = None;
        let mut bookkeeping = None;
        let (result, _) = loop {
            if let Poll::Ready(result) = refresh.as_mut().poll(&mut cx) {
                break result;
            }
            let backends: Vec<(i32, String, String, Option<String>)> = sqlx::query_as(
                "select pid,coalesce(state,''),query,wait_event_type from pg_stat_activity
             where datname=current_database() and application_name='porchlight-api-test'",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            if !cancel.load(Ordering::Acquire) {
                // A savepoint begun inside the deadline (the domain savepoint is _1) has executed and
                // the refresh has not consumed its reply: cancel exactly there.
                if backends.iter().any(|(_, state, query, _)| {
                    state == "idle in transaction"
                        && query.starts_with("SAVEPOINT _sqlx_savepoint_")
                        && query != "SAVEPOINT _sqlx_savepoint_1"
                }) {
                    let mut lock = pool.begin().await.unwrap();
                    sqlx::query("lock table porchlight.ics_source_revisions in exclusive mode")
                        .execute(&mut *lock)
                        .await
                        .unwrap();
                    revisions_lock = Some(lock);
                    cancel.store(true, Ordering::Release);
                }
            } else if bookkeeping.is_none()
                && let Some((pid, ..)) = backends.iter().find(|(_, _, query, wait)| {
                    wait.as_deref() == Some("Lock")
                        && query
                            .trim_start()
                            .starts_with("update porchlight.ics_sources")
                })
            {
                // FOR NO KEY UPDATE takes RowShareLock on the table; an autocommit UPDATE does not.
                let claimed: bool = sqlx::query_scalar(
                    "select exists(select 1 from pg_locks where pid=$1 and granted
                 and relation='porchlight.ics_sources'::regclass and mode='RowShareLock')",
                )
                .bind(pid)
                .fetch_one(&pool)
                .await
                .unwrap();
                bookkeeping = Some(claimed);
                revisions_lock.take().unwrap().rollback().await.unwrap();
            }
        };
        (result, bookkeeping)
    };
    assert!(result.unwrap(), "the deadline is recorded");
    let claimed = bookkeeping.expect("deadline bookkeeping write observed in flight");
    assert!(
        claimed,
        "deadline bookkeeping ran outside a source-row claim (autocommit in a poisoned transaction)"
    );
    let (status, error, failures, timeouts): (String, Option<String>, i32, i32) = sqlx::query_as(
        "select last_status,last_error,failure_count,apply_timeout_count from porchlight.ics_sources where id=$1",
    )
    .bind(source)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (status.as_str(), error.as_deref(), failures, timeouts),
        ("retrying", Some("apply_retryable"), 1, 1)
    );
    // Transaction-local actor settings exist only inside the attributed transaction.
    let actor: serde_json::Value = sqlx::query_scalar(
        "select actor from porchlight.ics_source_revisions
         where source_id=$1 and snapshot->>'last_status'='retrying'",
    )
    .bind(source)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        actor.get("request_id").is_some(),
        "deadline bookkeeping lost its actor: {actor}"
    );
    let items: i64 =
        sqlx::query_scalar("select count(*) from porchlight.items where ics_source_id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(items, 0, "the cancelled apply left no domain rows");
    task.abort();
    s.stop().await;
}
