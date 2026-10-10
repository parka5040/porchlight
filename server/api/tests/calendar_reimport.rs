use porchlight_api::testing::{TestClient, TestOptions, TestServer};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrations = "../db/migrations")]
async fn oneoff_representation_changes_replace_series_without_deleting_ordinary_absent_events(
    pool: PgPool,
) {
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
    let retained = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:series\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nRRULE:FREQ=DAILY;COUNT=3\r\nSUMMARY:Master\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let expanded = retained.replace("FREQ=DAILY;COUNT=3", "FREQ=DAILY;COUNT=3;BYHOUR=9");
    for (index, content) in [
        retained,
        expanded.as_str(),
        retained,
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n",
    ]
    .iter()
    .enumerate()
    {
        let response = c
            .request(
                s.lan,
                d.request(
                    "POST",
                    "/v1/imports/ics",
                    json!({"content":content,"member_id":s.init.admin_member_id}),
                    Some(&format!("step{index}")),
                ),
            )
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        let busy: i64 = sqlx::query_scalar("select count(*) from porchlight.member_agenda a join porchlight.items i on i.id=a.item_id where i.deleted_at is null and a.busy is not null").fetch_one(&pool).await.unwrap();
        assert_eq!(busy, 3, "step {index}");
    }
    s.stop().await;
}
