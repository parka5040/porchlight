use axum::{Router, body::to_bytes};
use chrono::{DateTime, Utc};
use porchlight_api::{
    routes,
    server::{ConnInfo, Transport},
    testing::{HouseholdFixture, TestOptions, TestServer, trust},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;
fn now() -> DateTime<Utc> {
    "2026-10-06T12:00:00Z".parse().unwrap()
}
fn router(s: &TestServer, fp: [u8; 32]) -> Router {
    routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(fp),
    }))
}
async fn read(r: &Router, d: &porchlight_api::testing::TestDevice, path: &str) -> Value {
    let response = r
        .clone()
        .oneshot(d.request("GET", path, json!({}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{path}");
    serde_json::from_slice(
        &to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}
#[sqlx::test(migrations = "../db/migrations")]
async fn canonical_ics_zones_scope_bounds_and_changed_notifications(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    for (alias, canonical, instant) in [
        ("US/Eastern", "America/New_York", "2026-10-12T13:00:00Z"),
        ("Asia/Calcutta", "Asia/Kolkata", "2026-10-12T03:30:00Z"),
        ("CET", "Europe/Brussels", "2026-10-12T07:00:00Z"),
        ("Etc/UTC", "UTC", "2026-10-12T09:00:00Z"),
    ] {
        let content = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:zone\r\nDTSTART;TZID={alias}:20261012T090000\r\nDTEND;TZID={alias}:20261012T100000\r\nSUMMARY:Zone\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let body = json!({"content":content,"scope":format!("{}{}",alias,"s".repeat(200-alias.len())),"name":"n".repeat(196),"member_id":f.users[0].member_id});
        let before: i64 = sqlx::query_scalar(
            "select count(*) from porchlight.notification_events where kind='calendar_changed'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let mut preview = body.clone();
        preview["preview"] = json!(true);
        let preview = trust::admin(&f.server, "POST", "/v1/imports/ics", preview).await;
        assert_eq!(preview.status, 200);
        assert_eq!(preview.json()["created"], 1);
        let after: i64 = sqlx::query_scalar(
            "select count(*) from porchlight.notification_events where kind='calendar_changed'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            before, after,
            "preview rolls back producer events with item changes"
        );
        let result = trust::admin(&f.server, "POST", "/v1/imports/ics", body.clone()).await;
        assert_eq!(result.status, 200, "{alias}: {}", result.json());
        let id: Uuid = serde_json::from_value(result.json()["item_ids"][0].clone()).unwrap();
        let (zone, start): (String, DateTime<Utc>) =
            sqlx::query_as("select tzid,starts_at from porchlight.items where id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(zone, canonical);
        assert_eq!(start, instant.parse::<DateTime<Utc>>().unwrap());
        let count:i64=sqlx::query_scalar("select count(*) from porchlight.notification_events where entity_id=$1 and kind='calendar_changed'").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(count, 1, "target receives imported change");
        let replay = trust::admin(&f.server, "POST", "/v1/imports/ics", body.clone()).await;
        assert_eq!(replay.status, 200);
        let unchanged:i64=sqlx::query_scalar("select count(*) from porchlight.notification_events where entity_id=$1 and kind='calendar_changed'").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(unchanged, count, "no-op import never emits");
        let mut invalid = body.clone();
        invalid["scope"] = json!("s".repeat(201));
        assert_eq!(
            trust::admin(&f.server, "POST", "/v1/imports/ics", invalid)
                .await
                .status,
            422
        );
        let mut invalid = body;
        invalid["name"] = json!("n".repeat(197));
        assert_eq!(
            trust::admin(&f.server, "POST", "/v1/imports/ics", invalid)
                .await
                .status,
            422
        );
    }
    let own = trust::admin(&f.server,"POST","/v1/imports/ics",json!({"content":"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:own\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nSUMMARY:Own\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n","member_id":f.admin.member_id})).await;
    assert_eq!(own.status, 200);
    let own_id: Uuid = serde_json::from_value(own.json()["item_ids"][0].clone()).unwrap();
    let own_count: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.notification_events where entity_id=$1",
    )
    .bind(own_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        own_count, 0,
        "agent owner self event suppressed by shared ABI"
    );
    f.server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn shared_item_visibility_snapshot_sources_search_and_feed_capabilities(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let content = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:acl\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nSUMMARY:SecretCalendar\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let imported = trust::admin(
        &f.server,
        "POST",
        "/v1/imports/ics",
        json!({"content":content,"member_id":f.users[0].member_id}),
    )
    .await;
    assert_eq!(imported.status, 200);
    let id: Uuid = serde_json::from_value(imported.json()["item_ids"][0].clone()).unwrap();
    let source=trust::admin(&f.server,"POST","/v1/admin/feeds",json!({"name":"private-config","url":"https://example.invalid/calendar","target_member_id":f.users[0].member_id})).await;
    assert_eq!(source.status, 201);
    let source_id: Uuid = serde_json::from_value(source.json()["id"].clone()).unwrap();
    sqlx::query("update porchlight.ics_sources set etag='private-validator',last_modified='private-date',last_status='failed',last_error='private-detail',last_warnings='[{\"code\":\"InvalidEvent\",\"uid\":\"private-uid\",\"message\":\"private-message\"}]' where id=$1")
        .bind(source_id).execute(&pool).await.unwrap();
    // The authoritative marker, not the badge, controls write capability.
    sqlx::query("update porchlight.items set ics_source_id=$2 where id=$1")
        .bind(id)
        .bind(source_id)
        .execute(&pool)
        .await
        .unwrap();
    for (user, visible, admin) in [
        (&f.admin, false, true),
        (&f.users[0], true, false),
        (&f.users[1], false, false),
    ] {
        let r = router(&f.server, user.device.fingerprint);
        let sql: bool = sqlx::query_scalar("select porchlight.pl_item_can_read($1,$2)")
            .bind(id)
            .bind(user.member_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(sql, visible);
        let detail = r
            .clone()
            .oneshot(
                user.device
                    .request("GET", &format!("/v1/items/{id}"), json!({}), None),
            )
            .await
            .unwrap();
        assert_eq!(
            detail.status(),
            if visible { 200 } else { 404 },
            "item detail agrees with SQL ACL"
        );
        if visible {
            let detail: Value = serde_json::from_slice(
                &to_bytes(detail.into_body(), 8 * 1024 * 1024).await.unwrap(),
            )
            .unwrap();
            assert_eq!(detail["capabilities"], json!(["view", "comment"]));
        }
        for path in [
            "/v1/items",
            "/v1/search?q=SecretCalendar",
            "/v1/board?scope=household",
            "/v1/agenda?from=2026-10-01T00:00:00Z&to=2026-11-01T00:00:00Z",
        ] {
            let response = read(&r, &user.device, path).await;
            let items = response["items"].as_array().unwrap();
            assert_eq!(
                items.iter().any(|v| v["id"] == id.to_string()),
                visible,
                "{path}"
            );
            if visible {
                assert!(
                    items.iter().all(|v| !v["capabilities"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|c| c == "edit")),
                    "{path}"
                );
            }
        }
        let snapshot = read(&r, &user.device, "/v1/snapshot").await;
        assert_eq!(
            snapshot["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["id"] == id.to_string()),
            visible
        );
        let sources = snapshot["ics_sources"]
            .as_array()
            .expect("snapshot source extension");
        assert_eq!(sources.len(), usize::from(admin));
        if admin {
            assert_eq!(sources[0]["id"], source_id.to_string());
            assert_eq!(sources[0]["host"], "example.invalid");
            assert_eq!(sources[0]["last_status"], "failed");
            for private in [
                "url",
                "etag",
                "last_modified",
                "last_error",
                "last_warnings",
                "import_key",
            ] {
                assert!(
                    sources[0].get(private).is_none(),
                    "snapshot must not cache {private}"
                );
            }
        }
    }
    let snapshot = trust::admin(&f.server, "GET", "/v1/snapshot", json!({})).await;
    assert_eq!(snapshot.status, 200);
    assert_eq!(snapshot.json()["ics_sources"].as_array().unwrap().len(), 1);
    assert!(
        snapshot.json()["items"][0]["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c == "edit")
    );
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn source_snapshot_shares_rr_view_and_published_cursor(pool: PgPool) {
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
        .session_device(
            s.init.admin_member_id,
            porchlight_core::auth::session::Platform::Desktop,
        )
        .await;
    let source = trust::admin(
        &s,
        "POST",
        "/v1/admin/feeds",
        json!({"name":"before","url":"https://example.invalid/calendar"}),
    )
    .await;
    assert_eq!(source.status, 201);
    let id: Uuid = serde_json::from_value(source.json()["id"].clone()).unwrap();
    let list = trust::admin(
        &s,
        "POST",
        "/v1/lists",
        json!({"name":"before","visibility":"private"}),
    )
    .await;
    assert_eq!(list.status, 201);
    let list_id: Uuid = serde_json::from_value(list.json()["id"].clone()).unwrap();
    let window: Uuid = sqlx::query_scalar("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,0,'09:00','10:00') returning id")
        .bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    porchlight_db::outbox::assign_stream_seqs(&s.state.pool, 1000)
        .await
        .unwrap();
    let mut writer = pool.begin().await.unwrap();
    sqlx::query("lock table porchlight.items in access exclusive mode")
        .execute(&mut *writer)
        .await
        .unwrap();
    let r = router(&s, d.fingerprint);
    let request = d.request("GET", "/v1/snapshot", json!({}), None);
    let task = tokio::spawn(async move { r.oneshot(request).await.unwrap() });
    tokio::time::timeout(std::time::Duration::from_secs(3),async {loop {
        let waiting:bool=sqlx::query_scalar("select exists(select 1 from pg_locks where relation='porchlight.items'::regclass and not granted and mode='AccessShareLock')").fetch_one(&pool).await.unwrap();
        if waiting {break} tokio::task::yield_now().await;
    }}).await.expect("snapshot never reached item read");
    sqlx::query("update porchlight.ics_sources set name='after' where id=$1")
        .bind(id)
        .execute(&mut *writer)
        .await
        .unwrap();
    sqlx::query("update porchlight.lists set name='after' where id=$1")
        .bind(list_id)
        .execute(&mut *writer)
        .await
        .unwrap();
    sqlx::query("update porchlight.availability_windows set starts_local='08:00' where id=$1")
        .bind(window)
        .execute(&mut *writer)
        .await
        .unwrap();
    writer.commit().await.unwrap();
    let response = task.await.unwrap();
    assert_eq!(response.status(), 200);
    let snapshot: Value = serde_json::from_slice(
        &to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(snapshot["ics_sources"][0]["name"], "before");
    assert_eq!(snapshot["lists"][0]["id"], list_id.to_string());
    assert_eq!(snapshot["lists"][0]["name"], "before");
    assert_eq!(
        snapshot["availability"][0]["windows"][0]["start"],
        "09:00:00"
    );
    assert_eq!(snapshot["availability"][0]["version"], 1);
    assert_eq!(snapshot["suggestions"], json!([]));
    let cursor = snapshot["cursor"].as_i64().unwrap();
    porchlight_db::outbox::assign_stream_seqs(&s.state.pool, 1000)
        .await
        .unwrap();
    let changed:i64=sqlx::query_scalar("select count(*) from porchlight.outbox where entity_id=$1 and kind='ics_source.changed' and stream_seq>$2").bind(id).bind(cursor).fetch_one(&pool).await.unwrap();
    assert_eq!(
        changed, 1,
        "excluded source update publishes after snapshot cursor"
    );
    s.stop().await;
}
