use crate::common;
use common::*;
use porchlight_api::testing::trust;
use porchlight_api::testing::{TestOptions, TestServer};
use porchlight_core::auth::session::Platform;
use porchlight_pki::generate_csr;
use serde_json::json;
use sqlx::PgPool;
use trust::*;
#[sqlx::test(migrations = "../db/migrations")]
async fn admin_scope_before_body_and_path(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let (_, t) = seed_pat(&pool, &s, &["read"]).await;
    for path in [
        "/v1/admin/enrollment-tokens",
        "/v1/admin/users/admin/password-reset",
        "/v1/admin/devices/not-a-uuid/revoke",
        "/v1/admin/tokens",
        "/v1/admin/tokens/not-a-uuid/revoke",
    ] {
        problem(
            &agent(
                &s,
                Some(&t),
                "POST",
                path,
                json!({"unexpected":true}),
                "key",
            )
            .await,
            403,
            "forbidden",
        );
    }
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn listener_isolation_and_session_requirement(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    for path in ["/v1/devices", "/v1/endpoints"] {
        problem(&get(&s, &d, path).await, 401, "unauthenticated");
    }
    problem(
        &post(
            &s,
            &d,
            "/v1/devices/self/certificate",
            json!({"csr":"garbage"}),
        )
        .await,
        401,
        "unauthenticated",
    );
    problem(&get(&s, &d, "/v1/admin/whoami").await, 404, "not-found");
    let r = porchlight_api::testing::TestClient::pinned(s.ca_fingerprint)
        .request(s.enroll, req("GET", "/v1/admin/whoami", json!({}), "key"))
        .await
        .unwrap();
    problem(&r, 404, "not-found");
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn enrollment_lockout_and_dead_member(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let t = enrollment_token(&s, "admin").await;
    let token = t["token"].as_str().unwrap();
    let (csr, _) = generate_csr("device").unwrap();
    for i in 0..5 {
        problem(
            &complete(&s, token, "wrong", &csr, &format!("wrong-{i}")).await,
            401,
            "invalid-credentials",
        );
    }
    let r = complete(&s, token, &s.admin_password, &csr, "locked").await;
    problem(&r, 429, "account-locked");
    assert!(r.headers.contains_key("retry-after"));
    let unused: bool =
        sqlx::query_scalar("SELECT used_at IS NULL FROM porchlight.enrollment_tokens")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(unused);
    let member = user(&pool, &s).await;
    let t = enrollment_token(&s, "alice").await;
    sqlx::query("UPDATE porchlight.household_members SET deleted_at=now() WHERE id=$1")
        .bind(member)
        .execute(&pool)
        .await
        .unwrap();
    problem(
        &complete(
            &s,
            t["token"].as_str().unwrap(),
            &s.admin_password,
            &csr,
            "dead",
        )
        .await,
        401,
        "unauthenticated",
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn enrollment_address_rate_limit_but_agent_unlimited(pool: PgPool) {
    let mut opts = TestOptions::default();
    opts.limits.auth_device = 2;
    opts.limits.auth_addr = 3;
    let now = std::time::Instant::now();
    opts.rate_clock = std::sync::Arc::new(move || now);
    let s = TestServer::start_with(&pool, opts).await;
    let (csr, _) = generate_csr("device").unwrap();
    for i in 0..3 {
        problem(
            &complete(
                &s,
                "unknown",
                &s.admin_password,
                &csr,
                &format!("unknown-{i}"),
            )
            .await,
            401,
            "enrollment-token-invalid",
        );
    }
    problem(
        &complete(&s, "unknown", &s.admin_password, &csr, "limit").await,
        429,
        "rate-limited",
    );
    let (_, a) = seed_pat(&pool, &s, &["admin"]).await;
    let (_, b) = seed_pat(&pool, &s, &["admin"]).await;
    let body = json!({"name":"Admin","scopes":["admin"],"password":s.admin_password});
    assert_eq!(
        agent(&s, Some(&a), "POST", "/v1/admin/tokens", body.clone(), "a")
            .await
            .status,
        201
    );
    assert_eq!(
        agent(&s, Some(&b), "POST", "/v1/admin/tokens", body.clone(), "b")
            .await
            .status,
        201
    );
    assert_eq!(
        agent(&s, Some(&a), "POST", "/v1/admin/tokens", body, "c")
            .await
            .status,
        201
    );
    assert_eq!(
        admin(
            &s,
            "POST",
            "/v1/admin/tokens",
            json!({"name":"Read","scopes":["read"]})
        )
        .await
        .status,
        201
    );
    s.stop().await;
}
