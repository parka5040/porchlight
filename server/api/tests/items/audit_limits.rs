use porchlight_api::testing::{TestClient, TestOptions, TestServer, trust};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrations = "../db/migrations")]
async fn audit_limits_create_and_update_reject_excess_history(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-11T00:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let (_, member) = trust::user_named(&pool, &s, "limits", "Sam Rivera").await;
    let d = s.session_device(member, Platform::Desktop).await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let fields = json!({"title":"History", "due_date":"1990-01-01", "recurrence_mode":"calendar", "rrule":"FREQ=DAILY", "missed_policy":"stack"});
    let response = client
        .request(s.lan, d.request("POST", "/v1/items", fields.clone(), None))
        .await
        .unwrap();
    assert_eq!(response.status, 422, "over-budget create must be rejected");
    assert_eq!(response.json()["errors"][0]["field"], "rrule");
    let created = client
        .request(
            s.lan,
            d.request("POST", "/v1/items", json!({"title":"History"}), None),
        )
        .await
        .unwrap();
    assert_eq!(created.status, 201);
    let item = created.json();
    let path = format!("/v1/items/{}", item["id"].as_str().unwrap());
    let mut request = d.request("PATCH", &path, fields, None);
    request
        .headers_mut()
        .insert("if-match", item["version"].to_string().parse().unwrap());
    let response = client.request(s.lan, request).await.unwrap();
    assert_eq!(response.status, 422, "over-budget update must be rejected");
    let count: i64 = sqlx::query_scalar("select count(*) from porchlight.items where owner_id=$1")
        .bind(member)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "rejected create must not write a master");
}
