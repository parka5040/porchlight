use porchlight_api::testing::{TestClient, TestOptions, TestServer, trust};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrations = "../db/migrations")]
async fn import_rechecks_authorized_item_version_after_lock_wait(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let (_, other) = trust::user_named(&pool, &s, "other", "Other").await;
    let device = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&device));
    let cal = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:race\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nSUMMARY:Original\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let body = json!({"content":cal,"member_id":s.init.admin_member_id});
    let made = client
        .request(
            s.lan,
            device.request("POST", "/v1/imports/ics", body.clone(), Some("initial")),
        )
        .await
        .unwrap();
    assert_eq!(made.status, 200);
    let id: uuid::Uuid = serde_json::from_value(made.json()["item_ids"][0].clone()).unwrap();
    let mut blocker = porchlight_db::tx::begin_as(&pool, &porchlight_db::tx::Actor::system())
        .await
        .unwrap();
    sqlx::query("update porchlight.items set owner_id=$2,visibility='private',title='Changed privately' where id=$1").bind(id).bind(other).execute(&mut *blocker).await.unwrap();
    let address = s.lan;
    let ca = s.ca_pem.clone();
    let request = device.request("POST", "/v1/imports/ics", body, Some("racing"));
    let running = tokio::spawn(async move {
        TestClient::new(&ca, Some(&device))
            .request(address, request)
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let blocked: bool = sqlx::query_scalar("select exists(select 1 from pg_stat_activity where datname=current_database() and wait_event='transactionid')").fetch_one(&pool).await.unwrap();
            if blocked { break; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.expect("import did not reach its planned item lock");
    blocker.commit().await.unwrap();
    let response = running.await.unwrap();
    assert_eq!(
        response.status, 409,
        "a changed authorization snapshot must be retried, never used"
    );
    let title: String = sqlx::query_scalar("select title from porchlight.items where id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "Changed privately");
    s.stop().await;
}
