use crate::common;
use common::*;
use porchlight_api::testing::{TestClient, TestServer};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrations = "../db/migrations")]
async fn mutation_rejects_bad_json_and_oversized_body(pool: PgPool) {
    use axum::http::Request;
    use http_body_util::Full;
    use hyper::body::Bytes;
    let s = TestServer::start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    for bytes in [b"not json".to_vec(), vec![b' '; 65537]] {
        let req = Request::builder()
            .header("porchlight-client-version", porchlight_core::VERSION)
            .method("POST")
            .uri("/v1/auth/login")
            .header("host", "localhost")
            .header("idempotency-key", "body-limit")
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(bytes)))
            .unwrap();
        let r = TestClient::new(&s.ca_pem, Some(&d))
            .request(s.lan, req)
            .await
            .unwrap();
        problem(&r, 400, "invalid-request");
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.idempotency_keys")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn protocol_case_and_method_problems(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let mut d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let mut req = request(
        &d,
        "POST",
        "/v1/auth/login",
        json!({"password":s.admin_password}),
        Some("case"),
    );
    req.headers_mut().insert(
        "content-type",
        "Application/JSON; charset=utf-8".parse().unwrap(),
    );
    let r = TestClient::new(&s.ca_pem, Some(&d))
        .request(s.lan, req)
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    d.session = Some(r.json()["session"]["token"].as_str().unwrap().to_owned());
    let mut req = request(&d, "GET", "/v1/me", json!({}), None);
    req.headers_mut().insert(
        "authorization",
        format!("bEaReR {}", d.session.as_ref().unwrap())
            .parse()
            .unwrap(),
    );
    assert_eq!(
        TestClient::new(&s.ca_pem, Some(&d))
            .request(s.lan, req)
            .await
            .unwrap()
            .status,
        200
    );
    for (method, path, key) in [
        ("GET", "/v1/auth/login", None),
        ("OPTIONS", "/v1/me", None),
        ("PUT", "/v1/me", Some("method")),
    ] {
        problem(
            &send(&s, &d, method, path, json!({}), key).await,
            405,
            "method-not-allowed",
        );
    }
    let head = send(&s, &d, "HEAD", "/v1/me", json!({}), None).await;
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    for addr in [s.lan, s.enroll] {
        for method in ["HEAD", "OPTIONS"] {
            let r = TestClient::new(&s.ca_pem, Some(&d))
                .request(addr, request(&d, method, "/absent", json!({}), None))
                .await
                .unwrap();
            assert_eq!(r.status, 404);
            assert_eq!(r.headers["content-type"], "application/problem+json");
        }
    }
    for method in ["HEAD", "OPTIONS"] {
        let r = porchlight_api::testing::agent_request(
            &s.agent_socket,
            None,
            request(&d, method, "/absent", json!({}), None),
        )
        .await;
        assert_eq!(r.status, 404);
        assert_eq!(r.headers["content-type"], "application/problem+json");
    }
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn sliding_touch_skips_contended_device(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    sqlx::query("UPDATE porchlight.sessions SET last_used_at=now()-interval '2 hours'")
        .execute(&pool)
        .await
        .unwrap();
    let before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT last_used_at FROM porchlight.sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM porchlight.devices WHERE id=$1 FOR UPDATE")
        .bind(d.device_id)
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    let r = tokio::time::timeout(std::time::Duration::from_secs(2), get(&s, &d, "/v1/me"))
        .await
        .expect("contended touch must not wait or fail");
    assert_eq!(r.status, 200);
    // The GET must not have locked the session before attempting the device lock.
    sqlx::query("SET LOCAL lock_timeout='500ms'")
        .execute(&mut *holder)
        .await
        .unwrap();
    sqlx::query("SELECT id FROM porchlight.sessions FOR UPDATE")
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    holder.commit().await.unwrap();
    let after: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT last_used_at FROM porchlight.sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(before, after);
    assert_eq!(get(&s, &d, "/v1/me").await.status, 200);
    let refreshed: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT last_used_at FROM porchlight.sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(refreshed > before);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn issuance_rechecks_revocation_under_device_lock(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let user = sqlx::query_scalar("SELECT user_id FROM porchlight.household_members WHERE id=$1")
        .bind(s.init.admin_member_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let code = s.reset_code(user).await;
    for (path, body) in [
        ("/v1/auth/login", json!({"password":s.admin_password})),
        (
            "/v1/auth/reset-password",
            json!({"code":code,"new_password":"a newly reset password"}),
        ),
    ] {
        let d = if path.ends_with("login") {
            &d
        } else {
            &s.issue_device(s.init.admin_member_id, Platform::Desktop)
                .await
        };
        let mut revoke = pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM porchlight.devices WHERE id=$1 FOR UPDATE")
            .bind(d.device_id)
            .fetch_one(&mut *revoke)
            .await
            .unwrap();
        let request = post(&s, d, path, body);
        tokio::pin!(request);
        // Poll the request concurrently until PostgreSQL confirms it is waiting on our device row lock.
        let wait = async {
            tokio::time::timeout(std::time::Duration::from_secs(2),async {
                loop {
                    let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND application_name='porchlight-api-test' AND wait_event_type='Lock' AND query LIKE '%porchlight.devices%FOR NO KEY UPDATE%')").fetch_one(&pool).await.unwrap();
                    if waiting { break; }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }).await.expect("request reaches row lock");
        };
        tokio::select! { _=wait=>{}, r=&mut request=>panic!("request did not wait: {r:?}") }
        sqlx::query(
            "UPDATE porchlight.devices SET revoked_at=now() WHERE id=$1 AND revoked_at IS NULL",
        )
        .bind(d.device_id)
        .execute(&mut *revoke)
        .await
        .unwrap();
        revoke.commit().await.unwrap();
        problem(&request.await, 401, "device-revoked");
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let used: bool =
        sqlx::query_scalar("SELECT used_at IS NOT NULL FROM porchlight.password_resets")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!used);
    s.stop().await;
}
