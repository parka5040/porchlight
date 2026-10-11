use crate::common;
use common::{get, post};
use porchlight_api::testing::{HouseholdFixture, TestClient, TestDevice, TestOptions, TestServer};
use serde_json::{Value, json};
use sqlx::PgPool;

async fn mutate(
    s: &TestServer,
    d: &TestDevice,
    method: &str,
    path: &str,
    body: Value,
    version: &Value,
) -> Value {
    let mut req = d.request(method, path, body, None);
    req.headers_mut()
        .insert("if-match", version.to_string().parse().unwrap());
    let r = TestClient::new(&s.ca_pem, Some(d))
        .request(s.lan, req)
        .await
        .unwrap();
    assert!(r.status < 300, "mutation status {}", r.status);
    if r.status == 204 {
        Value::Null
    } else {
        r.json()
    }
}
async fn fixture(pool: &PgPool) -> HouseholdFixture {
    HouseholdFixture::build_with(
        pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await
}
async fn private_list(f: &HouseholdFixture) -> (Value, Value) {
    let s = &f.server;
    let d = &f.users[0].device;
    let item = post(
        s,
        d,
        "/v1/items",
        json!({"title":"private parent", "visibility":"private"}),
    )
    .await;
    assert_eq!(item.status, 201);
    let item = item.json();
    let list = post(
        s,
        d,
        "/v1/lists",
        json!({"name":"private sentinel name", "item_id":item["id"]}),
    )
    .await;
    assert_eq!(list.status, 201);
    let list = list.json();
    let entry = post(
        s,
        d,
        &format!("/v1/lists/{}/entries", list["id"].as_str().unwrap()),
        json!({"label":"private sentinel label", "notes":"private sentinel notes"}),
    )
    .await;
    assert_eq!(entry.status, 201);
    (item, list)
}
async fn orphan(s: &TestServer, d: &TestDevice, list: &Value) -> Value {
    let page = get(s, d, "/v1/lists?include_orphaned=true").await;
    assert_eq!(page.status, 200);
    page.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == list["id"])
        .unwrap()
        .clone()
}
fn redacted(row: &Value) {
    assert_eq!(
        row["capabilities"],
        json!(["delete"]),
        "cleanup must not grant content view"
    );
    assert_eq!(row["redacted"], true);
    assert_eq!(row["entries"], json!([]));
    assert_eq!(row["got_it"], json!([]));
    assert_eq!(row["viewers"], json!([]));
    assert!(row.get("name").is_none(), "private name must be absent");
    assert!(!row.to_string().contains("sentinel"));
    for key in row.as_object().unwrap().keys() {
        assert!(
            [
                "id",
                "version",
                "item_id",
                "mode",
                "owner_id",
                "capabilities",
                "redacted",
                "entries",
                "got_it",
                "viewers",
                "occurrence_id"
            ]
            .contains(&key.as_str()),
            "unexpected cleanup field: {key}"
        );
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn orphan_admin_cleanup_redacts_private_content_but_owner_retains_it(pool: PgPool) {
    let f = fixture(&pool).await;
    let (item, list) = private_list(&f).await;
    let s = &f.server;
    let before = get(s, &f.admin.device, "/v1/lists?include_orphaned=true").await;
    assert_eq!(before.json()["items"], json!([]));
    mutate(
        s,
        &f.users[0].device,
        "DELETE",
        &format!("/v1/items/{}", item["id"].as_str().unwrap()),
        json!({}),
        &item["version"],
    )
    .await;
    let visible = orphan(s, &f.users[0].device, &list).await;
    assert_eq!(visible["name"], "private sentinel name");
    assert_eq!(visible["entries"][0]["notes"], "private sentinel notes");
    assert_eq!(visible["capabilities"], json!(["view", "delete"]));
    let hidden = orphan(s, &f.admin.device, &list).await;
    redacted(&hidden);
    let mut recorded = hidden.clone();
    for (field, suffix) in [("id", 1), ("item_id", 2), ("owner_id", 3)] {
        recorded[field] = json!(format!("00000000-0000-4000-8000-{suffix:012}"));
    }
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../client/shared/src/jvmTest/resources/item-fixtures/list-orphan-redacted.json"
    ))
    .unwrap();
    assert_eq!(recorded, fixture, "redacted client recording drift");
    let mut stale = f.admin.device.request(
        "DELETE",
        &format!("/v1/lists/{}", list["id"].as_str().unwrap()),
        json!({}),
        None,
    );
    stale.headers_mut().insert("if-match", "0".parse().unwrap());
    let conflict = TestClient::new(&s.ca_pem, Some(&f.admin.device))
        .request(s.lan, stale)
        .await
        .unwrap();
    assert_eq!(conflict.status, 409);
    redacted(&conflict.json()["current"]);
    mutate(
        s,
        &f.admin.device,
        "DELETE",
        &format!("/v1/lists/{}", list["id"].as_str().unwrap()),
        json!({}),
        &hidden["version"],
    )
    .await;
    let after = get(s, &f.admin.device, "/v1/lists?include_orphaned=true").await;
    assert_eq!(after.json()["items"], json!([]));
    f.server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn former_list_owner_cannot_read_transferred_private_orphan(pool: PgPool) {
    let f = fixture(&pool).await;
    let (item, list) = private_list(&f).await;
    let s = &f.server;
    let path = format!("/v1/items/{}", item["id"].as_str().unwrap());
    mutate(
        s,
        &f.users[0].device,
        "PATCH",
        &path,
        json!({"owner_id":f.users[1].member_id}),
        &item["version"],
    )
    .await;
    let transferred = get(s, &f.users[1].device, &path).await.json();
    let before = get(s, &f.users[0].device, "/v1/lists?include_orphaned=true").await;
    assert_eq!(before.json()["items"], json!([]));
    mutate(
        s,
        &f.users[1].device,
        "DELETE",
        &path,
        json!({}),
        &transferred["version"],
    )
    .await;
    redacted(&orphan(s, &f.users[0].device, &list).await);
    assert_eq!(
        get(s, &f.users[1].device, "/v1/lists?include_orphaned=true")
            .await
            .json()["items"],
        json!([])
    );
    f.server.stop().await;
}

fn calendar(description: &str, rule: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:retained\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nSUMMARY:Retained\r\nDESCRIPTION:{description}\r\n{rule}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}
async fn item_state(pool: &PgPool) -> Value {
    sqlx::query_scalar("SELECT coalesce(jsonb_agg(jsonb_build_array(id,version,title,notes,deleted_at) ORDER BY id),'[]'::jsonb) FROM porchlight.items").fetch_one(pool).await.unwrap()
}
#[sqlx::test(migrations = "../db/migrations")]
async fn hostile_import_and_feed_preserve_previous_items(pool: PgPool) {
    let hostile = calendar(
        &"d".repeat(4 * 1024 * 1024),
        "RRULE:FREQ=HOURLY;COUNT=10000",
    );
    assert_hostile_import_and_feed(pool, hostile, false).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn diagnostic_limits_protect_import_and_feed_reconciliation(pool: PgPool) {
    // Less than 12 MiB of diagnostics even on the old code, not the GiB attack.
    let missing_times: String = (0..512)
        .map(|n| {
            format!(
                "BEGIN:VEVENT\r\nUID:{n:04}{}\r\nEND:VEVENT\r\n",
                "u".repeat(1020)
            )
        })
        .collect();
    let hostile = calendar(
        "unsafe replacement",
        &format!("EXDATE:{}", ",".repeat(10_001)),
    )
    .replace("END:VCALENDAR", &(missing_times + "END:VCALENDAR"));
    assert_hostile_import_and_feed(pool, hostile, true).await;
}

async fn assert_hostile_import_and_feed(pool: PgPool, hostile: String, truncated: bool) {
    use porchlight_api::calendar::{fetch::Fetcher, jobs::refresh_source};
    use porchlight_core::auth::session::Platform;
    use std::sync::{Arc, Mutex};
    let now = || "2026-10-06T12:00:00Z".parse().unwrap();
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let good = calendar("original", "");
    let response = post(
        &s,
        &d,
        "/v1/imports/ics",
        json!({"content":good,"member_id":s.init.admin_member_id}),
    )
    .await;
    assert_eq!(response.status, 200);
    let before = item_state(&pool).await;
    let upload = post(
        &s,
        &d,
        "/v1/imports/ics",
        json!({"content":hostile,"member_id":s.init.admin_member_id}),
    );
    let responsive = async {
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(2), get(&s, &d, "/v1/me"))
                .await
                .unwrap()
                .status,
            200
        );
    };
    let (response, ()) = tokio::join!(upload, responsive);
    assert_eq!(response.status, 200);
    assert!(
        response.json()["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "InvalidEvent")
    );
    assert_eq!(item_state(&pool).await, before);
    if truncated {
        assert_bounded_diagnostics(&response.json()["warnings"], true);
    }

    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(
        porchlight_api::tls::enroll_server_config(&s.state.pki).unwrap(),
    );
    let body = Arc::new(Mutex::new(good));
    let served = body.clone();
    let task = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            let body = served.clone();
            tokio::spawn(async move {
                let stream = acceptor.accept(stream).await.unwrap();
                let service = hyper::service::service_fn(move |_| {
                    let content = body.lock().unwrap().clone();
                    async move {
                        Ok::<_, std::convert::Infallible>(
                            hyper::Response::builder()
                                .header("content-type", "text/calendar")
                                .body(http_body_util::Full::new(hyper::body::Bytes::from(content)))
                                .unwrap(),
                        )
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
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
    let fetcher = Fetcher::local_test(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let source = post(
        &s,
        &d,
        "/v1/feeds",
        json!({"name":"Resource test","url":format!("https://{address}/calendar"),"refresh_seconds":60}),
    )
    .await;
    assert_eq!(source.status, 201);
    let id = serde_json::from_value(source.json()["id"].clone()).unwrap();
    refresh_source(&s.state.pool, s.init.household_id, id, now(), &fetcher)
        .await
        .unwrap();
    let before = item_state(&pool).await;
    assert_eq!(
        before.as_array().unwrap().len(),
        2,
        "feed's original event must exist before hostile refresh"
    );
    *body.lock().unwrap() = hostile;
    let (result, response) = tokio::join!(
        refresh_source(
            &s.state.pool,
            s.init.household_id,
            id,
            now() + chrono::Duration::hours(2),
            &fetcher
        ),
        get(&s, &d, "/v1/me")
    );
    assert!(result.unwrap());
    assert_eq!(response.status, 200);
    assert_eq!(item_state(&pool).await, before);
    let feeds = get(&s, &d, "/v1/feeds").await;
    assert_eq!(feeds.status, 200);
    let feeds = feeds.json();
    let feed = feeds["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == source.json()["id"])
        .unwrap();
    assert_eq!(feed["last_status"], "partial");
    assert!(
        feed["last_warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "InvalidEvent")
    );
    if truncated {
        assert_bounded_diagnostics(&feed["last_warnings"], false);
    }
    task.abort();
    s.stop().await;
}

fn assert_bounded_diagnostics(warnings: &Value, require_diagnostic_marker: bool) {
    assert!(warnings.as_array().unwrap().len() <= 128);
    assert!(warnings.to_string().len() <= 80 * 1024);
    // Feed storage additionally caps warnings at 50, retaining ExpansionLimited
    // and its own WarningsTruncated marker rather than every partial diagnostic.
    assert!(warnings.as_array().unwrap().iter().any(|w| {
        w["uid"].is_null()
            && w["code"] == "ExpansionLimited"
            && (!require_diagnostic_marker
                || w["message"]
                    .as_str()
                    .unwrap()
                    .contains("Diagnostics truncated"))
    }));
}
