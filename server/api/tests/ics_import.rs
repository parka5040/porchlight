use porchlight_api::testing::{TestClient, TestOptions, TestServer};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;

const CAL: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:school\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\nEXDATE:20261013T090000Z\r\nSUMMARY:School\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:school\r\nRECURRENCE-ID:20261014T090000Z\r\nDTSTART:20261014T110000Z\r\nDTEND:20261014T120000Z\r\nSUMMARY:Moved school\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

#[sqlx::test(migrations = "../db/migrations")]
async fn preview_commit_reimport_scoped_identity_and_exceptions(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            db_pool_size: 1,
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-06T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let body = json!({"content":CAL,"name":"school","member_id":s.init.admin_member_id,"scope":"school-file","preview":true});
    let preview = c
        .request(
            s.lan,
            d.request("POST", "/v1/imports/ics", body.clone(), Some("preview")),
        )
        .await
        .unwrap();
    assert_eq!(preview.status, 200);
    assert_eq!(preview.json()["created"], 1);
    let count: i64 = sqlx::query_scalar("select count(*) from porchlight.items")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let mut body = body;
    body["preview"] = false.into();
    let imported = c
        .request(
            s.lan,
            d.request("POST", "/v1/imports/ics", body.clone(), Some("commit")),
        )
        .await
        .unwrap();
    assert_eq!(imported.status, 200);
    assert_eq!(imported.json()["created"], 1);
    assert_eq!(imported.json()["skipped"], 1);
    let rows: Vec<(String, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as("select o.status,coalesce(a.starts_at,o.starts_at) from porchlight.item_occurrences o left join porchlight.member_agenda a on a.occurrence_id=o.id order by o.seq").fetch_all(&pool).await.unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1].0, "skipped");
    assert_eq!(rows[2].1.unwrap().to_rfc3339(), "2026-10-14T11:00:00+00:00");
    let imported = c
        .request(
            s.lan,
            d.request("POST", "/v1/imports/ics", body.clone(), Some("reimport")),
        )
        .await
        .unwrap();
    assert_eq!(imported.status, 200);
    assert_eq!(imported.json()["created"], 0);
    let count: i64 = sqlx::query_scalar("select count(*) from porchlight.items")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    body["scope"] = "another-file".into();
    let imported = c
        .request(
            s.lan,
            d.request("POST", "/v1/imports/ics", body, Some("other")),
        )
        .await
        .unwrap();
    assert_eq!(imported.status, 200);
    assert_eq!(imported.json()["created"], 1);
    s.stop().await;
}

#[test]
fn ssrf_denies_non_public_address_classes() {
    use porchlight_api::calendar::fetch::public_address;
    for value in [
        "127.0.0.1",
        "10.2.3.4",
        "172.16.0.1",
        "192.168.1.1",
        "169.254.1.1",
        "100.64.0.1",
        "224.0.0.1",
        "0.0.0.0",
        "::1",
        "::",
        "fc00::1",
        "fe80::1",
        "ff02::1",
        "::ffff:127.0.0.1",
        "192.0.2.1",
        "2001:db8::1",
        "2001:10::1",
        "2001:20::1",
        "2001:2::1",
        "3fff::1",
    ] {
        assert!(
            !public_address(value.parse().unwrap()),
            "non-public address admitted"
        );
    }
    for value in [
        "8.8.8.8",
        "1.1.1.1",
        "2606:4700:4700::1111",
        "172.32.0.1",
        "100.128.0.1",
        "198.20.0.1",
    ] {
        assert!(public_address(value.parse().unwrap()));
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn producer_fixtures_roundtrip_without_duplicates(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-01-01T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let fixtures = [
        (
            "google",
            include_str!("../../core/tests/fixtures/ics/google.ics"),
        ),
        (
            "outlook",
            include_str!("../../core/tests/fixtures/ics/outlook.ics"),
        ),
        (
            "apple",
            include_str!("../../core/tests/fixtures/ics/apple.ics"),
        ),
        (
            "school",
            include_str!("../../core/tests/fixtures/ics/school.ics"),
        ),
        (
            "floating",
            include_str!("../../core/tests/fixtures/ics/floating.ics"),
        ),
        (
            "unsupported",
            include_str!("../../core/tests/fixtures/ics/unsupported.ics"),
        ),
        (
            "folded",
            include_str!("../../core/tests/fixtures/ics/folded.ics"),
        ),
    ];
    for (name, content) in fixtures {
        let body =
            json!({"content":content,"scope":name,"name":name,"member_id":s.init.admin_member_id});
        let result = c
            .request(
                s.lan,
                d.request(
                    "POST",
                    "/v1/imports/ics",
                    body.clone(),
                    Some(&format!("first-{name}")),
                ),
            )
            .await
            .unwrap();
        assert_eq!(result.status, 200, "producer fixture {name}");
        let count: i64 = sqlx::query_scalar("select count(*) from porchlight.items")
            .fetch_one(&pool)
            .await
            .unwrap();
        let result = c
            .request(
                s.lan,
                d.request(
                    "POST",
                    "/v1/imports/ics",
                    body,
                    Some(&format!("second-{name}")),
                ),
            )
            .await
            .unwrap();
        assert_eq!(result.status, 200, "producer fixture reimport {name}");
        assert_eq!(result.json()["created"], 0);
        let after: i64 = sqlx::query_scalar("select count(*) from porchlight.items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, after);
    }
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn exception_removal_restores_anchor_and_cancelled_master_soft_deletes(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-06T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let body = json!({"content":CAL,"member_id":s.init.admin_member_id,"scope":"exceptions"});
    assert_eq!(
        c.request(
            s.lan,
            d.request("POST", "/v1/imports/ics", body.clone(), Some("initial"))
        )
        .await
        .unwrap()
        .status,
        200
    );
    let plain = CAL
        .split("BEGIN:VEVENT\r\nUID:school\r\nRECURRENCE-ID")
        .next()
        .unwrap()
        .replace("EXDATE:20261013T090000Z\r\n", "")
        + "END:VCALENDAR\r\n";
    let mut body = body;
    body["content"] = plain.clone().into();
    assert_eq!(
        c.request(
            s.lan,
            d.request("POST", "/v1/imports/ics", body.clone(), Some("reset"))
        )
        .await
        .unwrap()
        .status,
        200
    );
    let starts: Vec<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select starts_at from porchlight.member_agenda order by starts_at")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(starts.len(), 3);
    assert_eq!(starts[2].to_rfc3339(), "2026-10-14T09:00:00+00:00");
    body["content"] = plain
        .replace("SUMMARY:School", "STATUS:CANCELLED\r\nSUMMARY:School")
        .into();
    let deleted = c
        .request(
            s.lan,
            d.request("POST", "/v1/imports/ics", body, Some("cancel")),
        )
        .await
        .unwrap();
    assert_eq!(deleted.status, 200);
    assert_eq!(deleted.json()["deleted"], 1);
    let live: i64 =
        sqlx::query_scalar("select count(*) from porchlight.items where deleted_at is null")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(live, 0);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn future_exceptions_survive_beyond_materialization_horizon(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-06T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let content = CAL.replace("202610", "203010");
    let result = c
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/imports/ics",
                json!({"content":content,"member_id":s.init.admin_member_id,"scope":"future"}),
                Some("future"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(result.status, 200);
    assert_eq!(result.json()["skipped"], 1);
    let rows: Vec<(String, bool)> =
        sqlx::query_as("select status,is_detached from porchlight.item_occurrences order by seq")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows, vec![("skipped".into(), false), ("todo".into(), true)]);
    s.stop().await;
}
