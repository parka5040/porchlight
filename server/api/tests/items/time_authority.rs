use porchlight_api::{
    routes,
    server::{ConnInfo, Transport},
    testing::{TestClient, TestServer},
};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;
#[sqlx::test(migrations = "../db/migrations")]
async fn authenticated_timezone_header_claims_authority_and_failure_is_best_effort(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let _client = TestClient::new(&s.ca_pem, Some(&d));
    let router = routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(d.fingerprint),
    }));
    let mut req = d.request("GET", "/v1/me", json!({}), None);
    req.headers_mut()
        .insert("porchlight-tz", "Europe/London".parse().unwrap());
    assert_eq!(router.clone().oneshot(req).await.unwrap().status(), 200);
    let state: (String, Option<uuid::Uuid>) = sqlx::query_as(
        "select tzid,tz_authority_device_id from porchlight.household_members where id=$1",
    )
    .bind(s.init.admin_member_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, ("Europe/London".into(), Some(d.device_id)));
    sqlx::raw_sql("create function porchlight.test_fail_zone() returns trigger language plpgsql as $$ begin raise exception 'test injected zone failure'; end $$; create trigger zz_test_fail_zone before update of tzid on porchlight.household_members for each row execute function porchlight.test_fail_zone();").execute(&pool).await.unwrap();
    let mut req = d.request("GET", "/v1/me", json!({}), None);
    req.headers_mut()
        .insert("porchlight-tz", "Asia/Tokyo".parse().unwrap());
    assert_eq!(router.oneshot(req).await.unwrap().status(), 200);
    let zone: String =
        sqlx::query_scalar("select tzid from porchlight.household_members where id=$1")
            .bind(s.init.admin_member_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(zone, "Europe/London");
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unchanged_timezone_get_returns_while_same_device_mutation_is_in_flight(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let router = routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(d.fingerprint),
    }));
    let mut req = d.request("GET", "/v1/me", json!({}), None);
    req.headers_mut()
        .insert("porchlight-tz", "Europe/London".parse().unwrap());
    assert_eq!(router.clone().oneshot(req).await.unwrap().status(), 200);
    let mut mutation =
        porchlight_db::tx::begin_as(&s.state.pool, &porchlight_db::tx::Actor::system())
            .await
            .unwrap();
    assert_eq!(
        porchlight_db::auth::share_device(&mut mutation, d.device_id)
            .await
            .unwrap(),
        Some(true)
    );
    let mut req = d.request("GET", "/v1/me", json!({}), None);
    req.headers_mut()
        .insert("porchlight-tz", "Europe/London".parse().unwrap());
    let response = tokio::time::timeout(std::time::Duration::from_millis(500), router.oneshot(req))
        .await
        .expect("unchanged GET waited on the in-flight mutation device lock")
        .unwrap();
    assert_eq!(response.status(), 200);
    mutation.rollback().await.unwrap();
    s.stop().await;
}
