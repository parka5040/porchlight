use porchlight_api::testing::{TestClient, TestOptions, TestServer};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;

async fn accept_tight_fold(pool: &PgPool, busy: bool) {
    let s = TestServer::start_with(
        pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(|| "2026-11-01T04:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let member = s.init.admin_member_id;
    sqlx::query("update porchlight.household_members set tzid='America/New_York' where id=$1")
        .bind(member)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) select household_id,id,6,$2,'02:00' from porchlight.household_members where id=$1")
        .bind(member).bind(if busy { "00:00:00" } else { "00:15:30" }.parse::<chrono::NaiveTime>().unwrap())
        .execute(pool).await.unwrap();
    let device = s.session_device(member, Platform::Desktop).await;
    let client = TestClient::new(&s.ca_pem, Some(&device));
    if busy {
        let created = client.request(s.lan,device.request("POST","/v1/items",json!({
            "title":"Busy time","kind":"appointment","time_mode":"fixed","tzid":"America/New_York",
            "starts_at":"2026-11-01T04:00:00Z","ends_at":"2026-11-01T04:15:30Z"
        }),None)).await.unwrap();
        assert_eq!(created.status, 201, "{}", created.json());
    }
    let created = client.request(s.lan,device.request("POST","/v1/items",json!({
        "title":"Fold work","kind":"todo","time_mode":"floating","due_date":"2026-11-01","est_minutes":120
    }),None)).await.unwrap();
    assert_eq!(created.status, 201, "{}", created.json());
    let proposed = client
        .request(
            s.lan,
            device.request("POST", "/v1/schedule/suggest", json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(proposed.status, 200, "{}", proposed.json());
    let proposed = proposed.json();
    assert_eq!(
        proposed["suggestions"].as_array().unwrap().len(),
        1,
        "tight fold slot must be proposed: busy={busy}"
    );
    let suggestion = &proposed["suggestions"][0];
    let occurrence: uuid::Uuid = suggestion["occurrence_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let version: i32 =
        sqlx::query_scalar("select version from porchlight.item_occurrences where id=$1")
            .bind(occurrence)
            .fetch_one(pool)
            .await
            .unwrap();
    let mut request = device.request(
        "POST",
        &format!(
            "/v1/suggestions/{}/accept",
            suggestion["id"].as_str().unwrap()
        ),
        json!({}),
        None,
    );
    request
        .headers_mut()
        .insert("if-match", version.to_string().parse().unwrap());
    let accepted = client.request(s.lan, request).await.unwrap();
    assert_eq!(
        accepted.status,
        200,
        "unchanged second-precision fold suggestion must be accepted: {}",
        accepted.json()
    );
    let times: (chrono::NaiveDateTime, chrono::NaiveDateTime) = sqlx::query_as("select override_local_start,override_local_end from porchlight.item_occurrences where id=$1")
        .bind(occurrence).fetch_one(pool).await.unwrap();
    assert_eq!(times.0.to_string(), "2026-11-01 01:00:00");
    assert_eq!(times.1.to_string(), "2026-11-01 02:00:00");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn second_precision_window_suggestion_is_accepted(pool: PgPool) {
    accept_tight_fold(&pool, false).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn second_precision_busy_end_suggestion_is_accepted(pool: PgPool) {
    accept_tight_fold(&pool, true).await;
}
