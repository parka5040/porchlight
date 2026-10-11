use crate::common;
use common::*;
use porchlight_api::{
    idempotency::{self, Begin, IdemScope},
    testing::{TestClient, TestServer, agent_request},
};
use porchlight_core::auth::session::Platform;
use porchlight_db::{idempotency::ScopeKind, tx::Actor};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn concurrent_claim_timeout_is_409(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let scope_id = Uuid::now_v7();
    let scope = || IdemScope {
        kind: ScopeKind::Enrollment,
        scope_id,
        household_id: s.init.household_id,
    };
    let actor = Actor {
        member_id: Some(s.init.admin_member_id),
        device_id: None,
        request_id: Uuid::now_v7(),
        agent_label: None,
        token_id: None,
    };
    let Begin::Fresh(first) = idempotency::begin(
        &s.state,
        &actor,
        &porchlight_api::extract::MutationAuth::Enrollment,
        scope(),
        "wait-key",
        &[0; 32],
    )
    .await
    .unwrap_or_else(|_| panic!("first claim")) else {
        panic!("fresh claim")
    };
    let before = std::time::Instant::now();
    let error = idempotency::begin(
        &s.state,
        &actor,
        &porchlight_api::extract::MutationAuth::Enrollment,
        scope(),
        "wait-key",
        &[0; 32],
    )
    .await
    .err()
    .expect("lock timeout");
    assert_eq!(error.status(), 409);
    assert_eq!(error.slug(), "idempotency-in-progress");
    assert_eq!(error.retry_after(), Some(1));
    assert!(before.elapsed() >= std::time::Duration::from_secs(4));
    first.tx.rollback().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.idempotency_keys")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn non_get_keys_required_on_all_surfaces(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let req = || request(&d, "POST", "/unknown", json!({}), None);
    problem(
        &TestClient::new(&s.ca_pem, Some(&d))
            .request(s.lan, req())
            .await
            .unwrap(),
        400,
        "idempotency-key-required",
    );
    problem(
        &TestClient::new(&s.ca_pem, None)
            .request(s.enroll, req())
            .await
            .unwrap(),
        400,
        "idempotency-key-required",
    );
    problem(
        &agent_request(&s.agent_socket, None, req()).await,
        400,
        "idempotency-key-required",
    );
    s.stop().await;
}
