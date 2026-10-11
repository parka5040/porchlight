use porchlight_api::testing::{TestClient, TestOptions, TestServer, trust};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrations = "../db/migrations")]
async fn audit_limits_exception_history_is_charged_before_writes(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-11T00:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let (_, member) = trust::user_named(&pool, &s, "exceptions", "Sam Rivera").await;
    let d = s.session_device(member, Platform::Desktop).await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    // No matching occurrence at noon. Even one exception requires walking all
    // history through its identity, rather than just the materialization horizon.
    let content = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:limits\r\nDTSTART:20261011T000000Z\r\nDTEND:20261011T010000Z\r\nRRULE:FREQ=DAILY\r\nEXDATE:20650101T120000Z\r\nSUMMARY:Bounded exceptions\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let response = client
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/imports/ics",
                json!({"content":content,"member_id":member}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status, 422,
        "exception traversal must consume the apply budget"
    );
    assert!(
        response.json()["type"]
            .as_str()
            .unwrap()
            .ends_with("calendar-too-large")
    );
    let exdates = (0..100)
        .map(|day| {
            let date =
                chrono::NaiveDate::from_ymd_opt(9998, 1, 1).unwrap() + chrono::Duration::days(day);
            format!("{}T120000Z", date.format("%Y%m%d"))
        })
        .collect::<Vec<_>>()
        .join(",");
    let content = content.replace("20650101T120000Z", &exdates);
    let response = client
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/imports/ics",
                json!({"content":content,"member_id":member}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 422);
    assert!(
        response.json()["type"]
            .as_str()
            .unwrap()
            .ends_with("calendar-too-large")
    );
    let count: i64 = sqlx::query_scalar("select count(*) from porchlight.items where owner_id=$1")
        .bind(member)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "admission must precede domain writes");
}
