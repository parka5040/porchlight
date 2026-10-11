use crate::common;
use chrono::{Duration, Utc};
use common::*;
use porchlight_api::testing::{TestClient, TestDevice, TestServer};
use porchlight_core::auth::{session::Platform, token::hash_token};
use serde_json::json;
use sqlx::PgPool;

async fn device(s: &TestServer) -> TestDevice {
    s.issue_device(s.init.admin_member_id, Platform::Desktop)
        .await
}
async fn failures(pool: &PgPool) -> i32 {
    sqlx::query_scalar("SELECT failed_attempts FROM porchlight.user_credentials LIMIT 1")
        .fetch_one(pool)
        .await
        .unwrap()
}
#[sqlx::test(migrations = "../db/migrations")]
async fn login_returns_session_and_me_works(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    for (platform, days) in [(Platform::Desktop, 90), (Platform::Android, 180)] {
        let mut d = s.issue_device(s.init.admin_member_id, platform).await;
        let r = login(&s, &mut d).await;
        assert!(d.session.as_ref().unwrap().starts_with("pls_"));
        let expiry = chrono::DateTime::parse_from_rfc3339(
            r.json()["session"]["expires_at"].as_str().unwrap(),
        )
        .unwrap();
        assert!(
            (expiry.with_timezone(&Utc) - Utc::now() - Duration::days(days))
                .num_seconds()
                .abs()
                < 5
        );
        let me = get(&s, &d, "/v1/me").await;
        assert_eq!(me.status, 200);
        let j = me.json();
        assert_eq!(j["username"], "admin");
        assert_eq!(j["display_name"], "Admin");
        assert_eq!(j["role"], "super_admin");
        assert_eq!(j["connection"]["transport"], "lan");
        assert_eq!(j["device"]["id"], d.device_id.to_string());
    }
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn login_wrong_password_401_and_counts(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = device(&s).await;
    problem(
        &post(&s, &d, "/v1/auth/login", json!({"password":"wrong"})).await,
        401,
        "invalid-credentials",
    );
    assert_eq!(failures(&pool).await, 1);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn lockout_after_five(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let mut d = device(&s).await;
    for _ in 0..5 {
        problem(
            &post(&s, &d, "/v1/auth/login", json!({"password":"wrong"})).await,
            401,
            "invalid-credentials",
        );
    }
    let r = post(
        &s,
        &d,
        "/v1/auth/login",
        json!({"password":s.admin_password}),
    )
    .await;
    problem(&r, 429, "account-locked");
    assert!(
        r.headers["retry-after"]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= 1
    );
    assert_eq!(failures(&pool).await, 5);
    sqlx::query("UPDATE porchlight.user_credentials SET locked_until=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    login(&s, &mut d).await;
    assert_eq!(failures(&pool).await, 0);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn session_bound_to_device(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let mut a = device(&s).await;
    let mut b = device(&s).await;
    login(&s, &mut a).await;
    b.session = a.session.clone();
    problem(&get(&s, &b, "/v1/me").await, 401, "session-device-mismatch");
    assert_eq!(get(&s, &a, "/v1/me").await.status, 200);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn session_invalid_cases(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let mut d = device(&s).await;
    problem(&get(&s, &d, "/v1/me").await, 401, "unauthenticated");
    d.session = Some("garbage".into());
    problem(&get(&s, &d, "/v1/me").await, 401, "session-invalid");
    login(&s, &mut d).await;
    assert_eq!(post(&s, &d, "/v1/auth/logout", json!({})).await.status, 204);
    problem(&get(&s, &d, "/v1/me").await, 401, "session-invalid");
    login(&s, &mut d).await;
    sqlx::query("UPDATE porchlight.sessions SET expires_at=now()-interval '1 second' WHERE revoked_at IS NULL").execute(&pool).await.unwrap();
    problem(&get(&s, &d, "/v1/me").await, 401, "session-invalid");
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn sliding_refresh(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let mut d = device(&s).await;
    login(&s, &mut d).await;
    sqlx::query("UPDATE porchlight.sessions SET last_used_at=now()-interval '2 hours',expires_at=now()+interval '1 day'").execute(&pool).await.unwrap();
    let first = get(&s, &d, "/v1/me").await.json()["session_expires_at"].clone();
    let expiry = chrono::DateTime::parse_from_rfc3339(first.as_str().unwrap()).unwrap();
    assert!(
        (expiry.with_timezone(&Utc) - Utc::now() - Duration::days(90))
            .num_seconds()
            .abs()
            < 5
    );
    assert_eq!(
        get(&s, &d, "/v1/me").await.json()["session_expires_at"],
        first
    );
    let seen: bool =
        sqlx::query_scalar("SELECT last_seen_at IS NOT NULL FROM porchlight.devices WHERE id=$1")
            .bind(d.device_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(seen);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn login_revokes_previous_device_sessions(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let mut d = device(&s).await;
    login(&s, &mut d).await;
    let first = d.session.clone();
    login(&s, &mut d).await;
    login(&s, &mut d).await; // includes already revoked rows (set-once controller note)
    d.session = first;
    problem(&get(&s, &d, "/v1/me").await, 401, "session-invalid");
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn password_change(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let mut a = device(&s).await;
    let mut b = device(&s).await;
    login(&s, &mut a).await;
    login(&s, &mut b).await;
    problem(
        &post(
            &s,
            &a,
            "/v1/auth/password",
            json!({"current_password":"wrong","new_password":"a unique new password"}),
        )
        .await,
        401,
        "invalid-credentials",
    );
    for (new, detail) in [
        ("password1234".to_owned(), "breached"),
        ("a".repeat(11), "too_short"),
        ("a".repeat(1025), "too_long"),
    ] {
        let r = post(
            &s,
            &a,
            "/v1/auth/password",
            json!({"current_password":s.admin_password,"new_password":new}),
        )
        .await;
        problem(&r, 422, "password-policy");
        assert_eq!(r.json()["detail"], detail);
    }
    assert_eq!(
        post(
            &s,
            &a,
            "/v1/auth/password",
            json!({"current_password":s.admin_password,"new_password":"a unique new password"})
        )
        .await
        .status,
        204
    );
    problem(&get(&s, &b, "/v1/me").await, 401, "session-invalid");
    assert_eq!(get(&s, &a, "/v1/me").await.status, 200);
    assert_eq!(
        post(
            &s,
            &b,
            "/v1/auth/login",
            json!({"password":"a unique new password"})
        )
        .await
        .status,
        200
    );
    // Revoking all user sessions must skip ones already revoked.
    assert_eq!(post(&s,&a,"/v1/auth/password",json!({"current_password":"a unique new password","new_password":"a second unique password"})).await.status,204);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn reauth_step_up(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let mut d = device(&s).await;
    login(&s, &mut d).await;
    let r = post(
        &s,
        &d,
        "/v1/auth/reauth",
        json!({"password":s.admin_password,"action":"device.revoke"}),
    )
    .await;
    assert_eq!(r.status, 200);
    assert!(
        r.json()["step_up_token"]
            .as_str()
            .unwrap()
            .starts_with("plu_")
    );
    let expiry =
        chrono::DateTime::parse_from_rfc3339(r.json()["expires_at"].as_str().unwrap()).unwrap();
    assert!(
        (expiry.with_timezone(&Utc) - Utc::now() - Duration::minutes(5))
            .num_seconds()
            .abs()
            < 5
    );
    problem(
        &post(
            &s,
            &d,
            "/v1/auth/reauth",
            json!({"password":s.admin_password,"action":"unknown"}),
        )
        .await,
        400,
        "invalid-request",
    );
    problem(
        &post(
            &s,
            &d,
            "/v1/auth/reauth",
            json!({"password":"wrong","action":"device.revoke"}),
        )
        .await,
        401,
        "invalid-credentials",
    );
    assert_eq!(failures(&pool).await, 1);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn revoked_or_unknown_device(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let mut d = device(&s).await;
    login(&s, &mut d).await;
    sqlx::query("UPDATE porchlight.devices SET revoked_at=now() WHERE id=$1")
        .bind(d.device_id)
        .execute(&pool)
        .await
        .unwrap();
    // The revocation listener may refresh the TLS verifier's revoked set before this request: then the handshake
    // itself fails (spec: revoked devices fail at the handshake); otherwise the per-request check answers 401.
    match TestClient::new(&s.ca_pem, Some(&d))
        .request(s.lan, request(&d, "GET", "/v1/me", json!({}), None))
        .await
    {
        Ok(r) => problem(&r, 401, "device-revoked"),
        Err(e) => assert!(format!("{e:?}").contains("CertificateRevoked"), "{e:?}"),
    }
    let (csr, key) = porchlight_pki::generate_csr("unknown").unwrap();
    let id = uuid::Uuid::now_v7();
    let cert = s
        .state
        .pki
        .ca
        .issue_device(&csr, &id.to_string(), std::time::SystemTime::now(), 365)
        .unwrap();
    let unknown = TestDevice {
        device_id: id,
        cert_pem: cert.cert_pem,
        key_pem: key.to_string(),
        fingerprint: cert.fingerprint,
        session: d.session,
    };
    problem(&get(&s, &unknown, "/v1/me").await, 401, "device-unknown");
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn problem_json_shape(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = device(&s).await;
    let r = get(&s, &d, "/v1/me").await;
    problem(&r, 401, "unauthenticated");
    assert!(r.json().get("detail").is_some());
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn connection_requires_session(pool: PgPool) {
    use http_body_util::BodyExt;
    let s = TestServer::start(&pool).await;
    let mut a = device(&s).await;
    let mut b = device(&s).await;
    problem(&get(&s, &a, "/v1/connection").await, 401, "unauthenticated");
    login(&s, &mut a).await;
    b.session = a.session.clone();
    problem(
        &get(&s, &b, "/v1/connection").await,
        401,
        "session-device-mismatch",
    );
    assert_eq!(post(&s, &a, "/v1/auth/logout", json!({})).await.status, 204);
    problem(&get(&s, &a, "/v1/connection").await, 401, "session-invalid");
    login(&s, &mut a).await;
    let tls = TestClient::new(&s.ca_pem, Some(&a))
        .connect(s.lan)
        .await
        .unwrap();
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(tls))
            .await
            .unwrap();
    let task = tokio::spawn(conn);
    let r = sender
        .send_request(request(&a, "GET", "/v1/connection", json!({}), None))
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    r.into_body().collect().await.unwrap();
    sqlx::query("UPDATE porchlight.devices SET revoked_at=now() WHERE id=$1")
        .bind(a.device_id)
        .execute(&pool)
        .await
        .unwrap();
    let r = sender
        .send_request(request(&a, "GET", "/v1/connection", json!({}), None))
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let j: serde_json::Value =
        serde_json::from_slice(&r.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(j["type"], "urn:porchlight:problem:device-revoked");
    drop(sender);
    task.abort();
    let _ = task.await;
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn logout_retry_is_terminal(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let mut d = device(&s).await;
    login(&s, &mut d).await;
    assert_eq!(
        send(
            &s,
            &d,
            "POST",
            "/v1/auth/logout",
            json!({}),
            Some("logout-key")
        )
        .await
        .status,
        204
    );
    problem(
        &send(
            &s,
            &d,
            "POST",
            "/v1/auth/logout",
            json!({}),
            Some("logout-key"),
        )
        .await,
        401,
        "session-invalid",
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn rate_limited(pool: PgPool) {
    use porchlight_api::{ratelimit::RateLimits, testing::TestOptions};
    let clock = std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let read_clock = clock.clone();
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            limits: RateLimits::production(),
            rate_clock: std::sync::Arc::new(move || *read_clock.lock().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let d = device(&s).await;
    for _ in 0..5 {
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
    }
    let r = post(
        &s,
        &d,
        "/v1/auth/login",
        json!({"password":s.admin_password}),
    )
    .await;
    problem(&r, 429, "rate-limited");
    assert!(
        r.headers["retry-after"]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= 1
    );
    // A device's rejection does not charge the address bucket. Three more devices fill the remaining 15 slots.
    for _ in 0..3 {
        let other = device(&s).await;
        for _ in 0..5 {
            assert_eq!(
                post(
                    &s,
                    &other,
                    "/v1/auth/login",
                    json!({"password":s.admin_password})
                )
                .await
                .status,
                200
            );
        }
    }
    let other = device(&s).await;
    problem(
        &post(
            &s,
            &other,
            "/v1/auth/login",
            json!({"password":s.admin_password}),
        )
        .await,
        429,
        "rate-limited",
    );
    // No wall time can refill the bucket above; advancing the injected clock does.
    *clock.lock().unwrap() += std::time::Duration::from_secs(3);
    assert_eq!(
        post(
            &s,
            &other,
            "/v1/auth/login",
            json!({"password":s.admin_password})
        )
        .await
        .status,
        200
    );
    s.stop().await;
}
// The database uses the token hash, never the opaque bearer.
#[test]
fn session_hash_is_fixed_size() {
    assert_eq!(hash_token("pls_example").len(), 32);
}
