use crate::common;
use common::*;
use porchlight_api::testing::TestServer;
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrations = "../db/migrations")]
async fn missing_key_400(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let mut d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    login(&s, &mut d).await;
    for path in ["login", "logout", "password", "reauth", "reset-password"] {
        problem(
            &send(&s, &d, "POST", &format!("/v1/auth/{path}"), json!({}), None).await,
            400,
            "idempotency-key-required",
        );
    }
    for key in ["", "contains space", &"x".repeat(256)] {
        problem(
            &send(
                &s,
                &d,
                "POST",
                "/v1/auth/login",
                json!({"password":s.admin_password}),
                Some(key),
            )
            .await,
            400,
            "idempotency-key-required",
        );
    }
    for values in [
        vec![
            axum::http::HeaderValue::from_static("one"),
            axum::http::HeaderValue::from_static("two"),
        ],
        vec![axum::http::HeaderValue::from_static("contains\ttab")],
        vec![axum::http::HeaderValue::from_bytes(&[0x80]).unwrap()],
    ] {
        let mut req = request(
            &d,
            "POST",
            "/v1/auth/login",
            json!({"password":s.admin_password}),
            None,
        );
        for value in values {
            req.headers_mut().append("idempotency-key", value);
        }
        let r = porchlight_api::testing::TestClient::new(&s.ca_pem, Some(&d))
            .request(s.lan, req)
            .await
            .unwrap();
        problem(&r, 400, "idempotency-key-required");
    }
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn replay_returns_identical_response(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let body = json!({"password":s.admin_password});
    let a = send(&s, &d, "POST", "/v1/auth/login", body.clone(), Some("key")).await;
    let b = send(&s, &d, "POST", "/v1/auth/login", body, Some("key")).await;
    assert_eq!(a.status, 200);
    assert_eq!(a.status, b.status);
    assert_eq!(a.body, b.body);
    assert_eq!(a.headers["content-type"], b.headers["content-type"]);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn same_key_different_body_422(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    assert_eq!(
        send(
            &s,
            &d,
            "POST",
            "/v1/auth/login",
            json!({"password":s.admin_password}),
            Some("key")
        )
        .await
        .status,
        200
    );
    problem(
        &send(
            &s,
            &d,
            "POST",
            "/v1/auth/login",
            json!({"password":"different"}),
            Some("key"),
        )
        .await,
        422,
        "idempotency-key-reused",
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn concurrent_duplicates_one_effect(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let results = futures_util::future::join_all((0..5).map(|_| {
        send(
            &s,
            &d,
            "POST",
            "/v1/auth/login",
            json!({"password":s.admin_password}),
            Some("key"),
        )
    }))
    .await;
    for r in &results {
        assert_eq!(r.status, 200);
        assert_eq!(r.body, results[0].body);
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn stored_rows_leak_nothing(pool: PgPool) {
    use sha2::{Digest, Sha256};
    let s = TestServer::start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let body = json!({"password":s.admin_password});
    let r = send(&s, &d, "POST", "/v1/auth/login", body.clone(), Some("key")).await;
    assert_eq!(r.status, 200);
    let (hash, sealed): (Vec<u8>, Vec<u8>) =
        sqlx::query_as("SELECT request_hash,response_sealed FROM porchlight.idempotency_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
    let token = r.json()["session"]["token"].as_str().unwrap().to_owned();
    assert!(!sealed.windows(token.len()).any(|w| w == token.as_bytes()));
    let mut plain = b"POST\n/v1/auth/login\n".to_vec();
    plain.extend(serde_json::to_vec(&body).unwrap());
    assert_ne!(hash, Sha256::digest(&plain).to_vec());
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn replay_keeps_allowlisted_headers(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    sqlx::query("UPDATE porchlight.user_credentials SET failed_attempts=5,locked_until=now()+interval '60 seconds'").execute(&pool).await.unwrap();
    let body = json!({"password":s.admin_password});
    let a = send(&s, &d, "POST", "/v1/auth/login", body.clone(), Some("key")).await;
    sqlx::query("UPDATE porchlight.user_credentials SET locked_until=NULL")
        .execute(&pool)
        .await
        .unwrap();
    let b = send(&s, &d, "POST", "/v1/auth/login", body, Some("key")).await;
    problem(&a, 429, "account-locked");
    problem(&b, 429, "account-locked");
    // A fresh key proves reprocessing would now succeed, not return the stored lockout.
    assert_eq!(
        post(
            &s,
            &d,
            "/v1/auth/login",
            json!({"password":s.admin_password})
        )
        .await
        .status,
        200
    );
    assert_eq!(a.body, b.body);
    assert_eq!(a.headers["retry-after"], b.headers["retry-after"]);
    assert_eq!(a.headers["content-type"], b.headers["content-type"]);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn error_responses_are_replayed(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    for _ in 0..2 {
        problem(
            &send(
                &s,
                &d,
                "POST",
                "/v1/auth/login",
                json!({"password":"wrong"}),
                Some("key"),
            )
            .await,
            401,
            "invalid-credentials",
        );
    }
    let count: i32 = sqlx::query_scalar("SELECT failed_attempts FROM porchlight.user_credentials")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn expired_keys_reusable(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    problem(
        &send(
            &s,
            &d,
            "POST",
            "/v1/auth/login",
            json!({"password":"wrong"}),
            Some("key"),
        )
        .await,
        401,
        "invalid-credentials",
    );
    sqlx::query("UPDATE porchlight.idempotency_keys SET expires_at=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        send(
            &s,
            &d,
            "POST",
            "/v1/auth/login",
            json!({"password":s.admin_password}),
            Some("key")
        )
        .await
        .status,
        200
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn keys_scoped_per_device(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    for _ in 0..2 {
        let d = s
            .issue_device(s.init.admin_member_id, Platform::Desktop)
            .await;
        assert_eq!(
            send(
                &s,
                &d,
                "POST",
                "/v1/auth/login",
                json!({"password":s.admin_password}),
                Some("key")
            )
            .await
            .status,
            200
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
    s.stop().await;
}
