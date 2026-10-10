mod common;
use common::*;
use porchlight_api::testing::trust;
use porchlight_api::testing::{TestOptions, TestServer};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use trust::*;
#[sqlx::test(migrations = "../db/migrations")]
async fn pat_auth(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    for token in [None, Some("pls_bad"), Some("plp_unknown")] {
        problem(
            &agent(&s, token, "GET", "/v1/admin/whoami", json!({}), "key").await,
            401,
            "unauthenticated",
        );
    }
    for state in ["revoked", "expired", "non-admin"] {
        let (id, t) = seed_pat(&pool, &s, &["admin"]).await;
        match state {
            "revoked" => {
                sqlx::query("UPDATE porchlight.api_tokens SET revoked_at=now() WHERE id=$1 AND revoked_at IS NULL").bind(id).execute(&pool).await.unwrap();
            }
            "expired" => {
                sqlx::query("UPDATE porchlight.api_tokens SET expires_at=now()-interval '1 second' WHERE id=$1").bind(id).execute(&pool).await.unwrap();
            }
            _ => {
                let member = user(&pool, &s).await;
                sqlx::query("UPDATE porchlight.api_tokens SET owner_member_id=$1 WHERE id=$2")
                    .bind(member)
                    .bind(id)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        }
        problem(
            &agent(&s, Some(&t), "GET", "/v1/admin/whoami", json!({}), "key").await,
            401,
            "unauthenticated",
        );
    }
    let (id, t) = seed_pat(&pool, &s, &["read"]).await;
    let r = agent(&s, Some(&t), "GET", "/v1/admin/whoami", json!({}), "key").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["token_id"], id.to_string());
    problem(
        &agent(&s, Some(&t), "GET", "/v1/admin/devices", json!({}), "key").await,
        403,
        "forbidden",
    );
    let before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT last_used_at FROM porchlight.api_tokens WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        agent(&s, Some(&t), "GET", "/v1/admin/whoami", json!({}), "key")
            .await
            .status,
        200
    );
    let after: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT last_used_at FROM porchlight.api_tokens WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(before, after);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn admin_token_requires_password(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    for body in [
        json!({"name":"admin token","scopes":["admin"]}),
        json!({"name":"admin token","scopes":["admin"],"password":"wrong"}),
    ] {
        problem(
            &admin(&s, "POST", "/v1/admin/tokens", body).await,
            401,
            "invalid-credentials",
        );
    }
    for body in [
        json!({"name":"admin token","scopes":["admin"],"password":s.admin_password}),
        json!({"name":"read token","scopes":["read"]}),
    ] {
        let r = admin(&s, "POST", "/v1/admin/tokens", body).await;
        assert_eq!(r.status, 201);
        let j = r.json();
        let t = j["token"].as_str().unwrap();
        assert_eq!(
            agent(&s, Some(t), "GET", "/v1/admin/whoami", json!({}), "key")
                .await
                .status,
            200
        );
    }
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn legacy_admin_token_lock_timeout_is_unstored_transient_conflict(pool: PgPool) {
    use porchlight_api::{
        routes,
        server::{ConnInfo, Transport},
    };
    use tower::ServiceExt;
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            db_pool_size: 1,
            ..Default::default()
        },
    )
    .await;
    // Exercise the production credentials-lock SQLSTATE on the sole app connection.
    sqlx::query("SET lock_timeout='100ms'")
        .execute(&s.state.pool)
        .await
        .unwrap();
    let user_id: uuid::Uuid =
        sqlx::query_scalar("SELECT user_id FROM porchlight.household_members WHERE id=$1")
            .bind(s.init.admin_member_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let before: (String, i32) = sqlx::query_as(
        "SELECT password_hash, failed_attempts FROM porchlight.user_credentials WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("SELECT user_id FROM porchlight.user_credentials WHERE user_id=$1 FOR UPDATE")
        .bind(user_id)
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    let router = routes::agent_router(s.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Agent,
        client_addr: None,
        client_cert_fingerprint: None,
    }));
    let request = || {
        let mut request = trust::req(
            "POST",
            "/v1/admin/tokens",
            json!({"name":"lock-probe-admin","scopes":["admin"],"password":s.admin_password}),
            "legacy-admin-credentials-lock",
        );
        request.headers_mut().insert(
            "authorization",
            format!("Bearer {}", s.bootstrap_token()).parse().unwrap(),
        );
        request.map(axum::body::Body::new)
    };
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        router.clone().oneshot(request()),
    )
    .await
    .expect("credential lock wait must be bounded")
    .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(response.headers()["retry-after"], "1");
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["type"],
        "urn:porchlight:problem:conflict"
    );
    let claims: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.idempotency_keys WHERE idem_key='legacy-admin-credentials-lock'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(claims, 0);
    let tokens: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.api_tokens WHERE name='lock-probe-admin'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(tokens, 0);
    let after: (String, i32) = sqlx::query_as(
        "SELECT password_hash, failed_attempts FROM porchlight.user_credentials WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, after);
    lock.rollback().await.unwrap();
    assert_eq!(router.oneshot(request()).await.unwrap().status(), 201);
    let claims: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.idempotency_keys WHERE idem_key='legacy-admin-credentials-lock'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(claims, 1);
    let tokens: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.api_tokens WHERE name='lock-probe-admin'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(tokens, 1);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn admin_password_lockout_and_validation(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    for _ in 0..5 {
        problem(
            &admin(
                &s,
                "POST",
                "/v1/admin/tokens",
                json!({"name":"admin","scopes":["admin"],"password":"wrong"}),
            )
            .await,
            401,
            "invalid-credentials",
        );
    }
    let r = admin(
        &s,
        "POST",
        "/v1/admin/tokens",
        json!({"name":"admin","scopes":["admin"],"password":s.admin_password}),
    )
    .await;
    problem(&r, 429, "account-locked");
    assert!(r.headers.contains_key("retry-after"));
    for body in [
        json!({"name":"","scopes":["read"]}),
        json!({"name":"t","scopes":[]}),
        json!({"name":"t","scopes":["bogus"]}),
        json!({"name":"t","scopes":["read"],"expires_days":0}),
    ] {
        problem(
            &admin(&s, "POST", "/v1/admin/tokens", body).await,
            400,
            "invalid-request",
        );
    }
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn token_revoke(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let (id, t) = seed_pat(&pool, &s, &["admin"]).await;
    let path = format!("/v1/admin/tokens/{id}/revoke");
    let a = admin(&s, "POST", &path, json!({})).await;
    assert_eq!(a.status, 200);
    let b = admin(&s, "POST", &path, json!({})).await;
    assert_eq!(a.json(), b.json());
    problem(
        &agent(&s, Some(&t), "GET", "/v1/admin/whoami", json!({}), "key").await,
        401,
        "unauthenticated",
    );
    let (id, t) = seed_pat(&pool, &s, &["admin"]).await;
    let path = format!("/v1/admin/tokens/{id}/revoke");
    assert_eq!(
        agent(&s, Some(&t), "POST", &path, json!({}), "terminal")
            .await
            .status,
        200
    );
    problem(
        &agent(&s, Some(&t), "POST", &path, json!({}), "terminal").await,
        401,
        "unauthenticated",
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn reset_code_via_socket(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    user(&pool, &s).await;
    let d = s
        .enroll("alice", &s.admin_password, Platform::Desktop)
        .await;
    let a = admin(
        &s,
        "POST",
        "/v1/admin/users/alice/password-reset",
        json!({}),
    )
    .await;
    assert_eq!(a.status, 201);
    let b = admin(
        &s,
        "POST",
        "/v1/admin/users/alice/password-reset",
        json!({}),
    )
    .await;
    assert_eq!(b.status, 201);
    problem(
        &post(
            &s,
            &d,
            "/v1/auth/reset-password",
            json!({"code":a.json()["code"],"new_password":"a new secure password"}),
        )
        .await,
        401,
        "reset-code-invalid",
    );
    assert_eq!(
        post(
            &s,
            &d,
            "/v1/auth/reset-password",
            json!({"code":b.json()["code"],"new_password":"a new secure password"})
        )
        .await
        .status,
        200
    );
    // Issuing after earlier consumed codes must not violate set-once triggers.
    assert_eq!(
        admin(
            &s,
            "POST",
            "/v1/admin/users/alice/password-reset",
            json!({})
        )
        .await
        .status,
        201
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn admin_idempotency_scoped_to_pat(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let (_, a) = seed_pat(&pool, &s, &["admin"]).await;
    let (_, b) = seed_pat(&pool, &s, &["admin"]).await;
    let body = json!({"username":"admin","device_label":"Desktop"});
    let x = agent(
        &s,
        Some(&a),
        "POST",
        "/v1/admin/enrollment-tokens",
        body.clone(),
        "same",
    )
    .await;
    let y = agent(
        &s,
        Some(&b),
        "POST",
        "/v1/admin/enrollment-tokens",
        body.clone(),
        "same",
    )
    .await;
    assert_eq!(x.status, 201);
    assert_eq!(y.status, 201);
    assert_ne!(x.body, y.body);
    assert_eq!(
        x.body,
        agent(
            &s,
            Some(&a),
            "POST",
            "/v1/admin/enrollment-tokens",
            body,
            "same"
        )
        .await
        .body
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.enrollment_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 2);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn agent_admin_tokens_unlimited_including_replay(pool: PgPool) {
    let opts = TestOptions {
        limits: porchlight_api::ratelimit::RateLimits::production(),
        ..Default::default()
    };
    let s = TestServer::start_with(&pool, opts).await;
    let body = json!({"name":"admin token","scopes":["admin"],"password":s.admin_password});
    let first = agent(
        &s,
        Some(s.bootstrap_token()),
        "POST",
        "/v1/admin/tokens",
        body.clone(),
        "first",
    )
    .await;
    assert_eq!(first.status, 201);
    for _ in 0..7 {
        assert_eq!(
            admin(&s, "POST", "/v1/admin/tokens", body.clone())
                .await
                .status,
            201
        );
    }
    let replay = agent(
        &s,
        Some(s.bootstrap_token()),
        "POST",
        "/v1/admin/tokens",
        body,
        "first",
    )
    .await;
    assert_eq!(replay.status, 201);
    assert_eq!(first.body, replay.body);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn absent_admin_password_does_not_hash_or_count(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    // Holding all hash slots proves the absent/empty field never starts verification.
    let slots = s
        .state
        .hash_slots
        .clone()
        .acquire_many_owned(2)
        .await
        .unwrap();
    for (i, body) in [
        json!({"name":"t","scopes":["admin"]}),
        json!({"name":"t","scopes":["admin"],"password":""}),
    ]
    .into_iter()
    .enumerate()
    {
        let key = format!("missing-{i}");
        let r = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            agent(
                &s,
                Some(s.bootstrap_token()),
                "POST",
                "/v1/admin/tokens",
                body.clone(),
                &key,
            ),
        )
        .await
        .expect("absent password used Argon2 slots");
        problem(&r, 401, "invalid-credentials");
        let replay = agent(
            &s,
            Some(s.bootstrap_token()),
            "POST",
            "/v1/admin/tokens",
            body,
            &key,
        )
        .await;
        assert_eq!(r.body, replay.body);
    }
    drop(slots);
    let failures: i32 = sqlx::query_scalar("SELECT failed_attempts FROM porchlight.user_credentials WHERE user_id=(SELECT user_id FROM porchlight.household_members WHERE id=$1)").bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    assert_eq!(failures, 0);
    s.stop().await;
}
