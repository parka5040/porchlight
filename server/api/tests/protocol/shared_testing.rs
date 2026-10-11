use porchlight_api::testing::{HouseholdFixture, TestClient, TestServer};
use sqlx::PgPool;
#[sqlx::test(migrations = "../db/migrations")]
async fn fixture_has_three_enrolled_users_and_a_dependent(pool: PgPool) {
    let f = HouseholdFixture::build(&pool).await;
    let client = TestClient::new(&f.server.ca_pem, Some(&f.admin.device));
    let r = client
        .request(
            f.server.lan,
            f.admin
                .device
                .request("GET", "/v1/me", serde_json::json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    for user in &f.users {
        assert!(user.device.session.is_some());
        assert_ne!(user.member_id, f.dependent);
    }
    let members: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.household_members WHERE household_id=$1",
    )
    .bind(f.server.init.household_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(members, 4);
    f.server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn startup_purge_failure_does_not_prevent_serving(pool: PgPool) {
    sqlx::raw_sql("REVOKE DELETE ON porchlight.idempotency_keys FROM porchlight_app")
        .execute(&pool)
        .await
        .unwrap();
    let s = TestServer::start(&pool).await;
    let request = axum::http::Request::builder()
        .uri("/missing")
        .header("host", "localhost")
        .header("porchlight-client-version", porchlight_core::VERSION)
        .body(http_body_util::Full::new(hyper::body::Bytes::new()))
        .unwrap();
    let r = TestClient::pinned(s.ca_fingerprint)
        .request(s.enroll, request)
        .await
        .unwrap();
    assert_eq!(r.status, 404);
    s.stop().await;
}
