mod common;
use common::*;
use porchlight_api::testing::trust;
use porchlight_api::testing::{TestDevice, TestOptions, TestServer};
use porchlight_core::auth::{enroll_uri::EnrollUri, session::Platform, token::hash_token};
use porchlight_pki::{generate_csr, parse_cert_pem};
use serde_json::json;
use sqlx::PgPool;
use trust::*;
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn enroll_end_to_end(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let t = enrollment_token(&s, "admin").await;
    let uri = EnrollUri::parse(t["uri"].as_str().unwrap()).unwrap();
    assert_eq!(uri.ca_fingerprint, s.ca_fingerprint);
    assert_eq!(uri.host, s.state.settings.enroll_host);
    let (csr, key) = generate_csr("untrusted cn").unwrap();
    let r = complete(&s, &uri.token, &s.admin_password, &csr, "end-to-end").await;
    assert_eq!(r.status, 201);
    let j = r.json();
    let cert = j["certificate"].as_str().unwrap();
    s.state.pki.ca.verify_server_signature(cert).unwrap();
    let info = parse_cert_pem(cert).unwrap();
    assert_eq!(info.common_name.as_deref(), j["device_id"].as_str());
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT fingerprint_sha256 FROM porchlight.device_certificates")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, info.fingerprint);
    let d = TestDevice {
        device_id: Uuid::parse_str(j["device_id"].as_str().unwrap()).unwrap(),
        cert_pem: cert.to_owned(),
        key_pem: key.to_string(),
        fingerprint: info.fingerprint,
        session: Some(j["session"]["token"].as_str().unwrap().to_owned()),
    };
    assert_eq!(get(&s, &d, "/v1/me").await.status, 200);
    assert_eq!(
        get(&s, &d, "/v1/endpoints").await.json()["endpoints"],
        json!(s.state.settings.endpoints)
    );
    assert_eq!(
        get(&s, &d, "/v1/devices").await.json()["devices"][0]["current"],
        true
    );
    let used: bool = sqlx::query_scalar(
        "SELECT used_at IS NOT NULL FROM porchlight.enrollment_tokens WHERE token_hash=$1",
    )
    .bind(hash_token(&uri.token).as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(used);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn token_single_use_and_expiry(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let t = enrollment_token(&s, "admin").await;
    let token = t["token"].as_str().unwrap();
    let (csr, _) = generate_csr("device").unwrap();
    assert_eq!(
        complete(&s, token, &s.admin_password, &csr, "first")
            .await
            .status,
        201
    );
    let (new, _) = generate_csr("new").unwrap();
    problem(
        &complete(&s, token, &s.admin_password, &new, "second").await,
        410,
        "enrollment-token-used",
    );
    let t = enrollment_token(&s, "admin").await;
    let token = t["token"].as_str().unwrap();
    sqlx::query("UPDATE porchlight.enrollment_tokens SET expires_at=now()-interval '1 second' WHERE used_at IS NULL").execute(&pool).await.unwrap();
    problem(
        &complete(&s, token, &s.admin_password, &csr, "expired").await,
        410,
        "enrollment-token-expired",
    );
    problem(
        &complete(&s, "ple_unknown", &s.admin_password, &csr, "unknown").await,
        401,
        "enrollment-token-invalid",
    );
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.idempotency_keys WHERE idem_key='unknown'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 0);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn enrollment_replay(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let t = enrollment_token(&s, "admin").await;
    let token = t["token"].as_str().unwrap();
    let (csr, _) = generate_csr("device").unwrap();
    let (a, b) = tokio::join!(
        complete(&s, token, &s.admin_password, &csr, "replay"),
        complete(&s, token, &s.admin_password, &csr, "replay")
    );
    assert_eq!(a.status, 201);
    assert_eq!(a.status, b.status);
    assert_eq!(a.body, b.body);
    // Hyper generates Date independently for each transport response; it is not
    // part of the sealed replay. Keep comparing every other response header.
    let replay_headers = |mut headers: axum::http::HeaderMap| {
        headers.remove(axum::http::header::DATE);
        headers
    };
    assert_eq!(replay_headers(a.headers), replay_headers(b.headers));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.devices")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn enrollment_wrong_password_keeps_token(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let t = enrollment_token(&s, "admin").await;
    let token = t["token"].as_str().unwrap();
    let (csr, _) = generate_csr("device").unwrap();
    problem(
        &complete(&s, token, "wrong", &csr, "wrong").await,
        401,
        "invalid-credentials",
    );
    let n: bool = sqlx::query_scalar("SELECT used_at IS NULL FROM porchlight.enrollment_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(n);
    assert_eq!(
        complete(&s, token, &s.admin_password, &csr, "correct")
            .await
            .status,
        201
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn enrollment_rejects_bad_csr(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let t = enrollment_token(&s, "admin").await;
    let token = t["token"].as_str().unwrap();
    let csr = include_str!("fixtures/ed25519.csr");
    for csr in ["garbage", csr] {
        problem(
            &complete(
                &s,
                token,
                &s.admin_password,
                csr,
                &Uuid::now_v7().to_string(),
            )
            .await,
            400,
            "csr-invalid",
        );
    }
    let n: bool = sqlx::query_scalar("SELECT used_at IS NULL FROM porchlight.enrollment_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(n);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn enrollment_not_on_lan_listener(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    problem(
        &post(&s, &d, "/v1/enrollment/complete", json!({})).await,
        404,
        "not-found",
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn renewal_window(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let (csr, _) = generate_csr("renew").unwrap();
    problem(
        &post(&s, &d, "/v1/devices/self/certificate", json!({"csr":csr})).await,
        409,
        "renewal-too-early",
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn renewal_keeps_old_certificate(pool: PgPool) {
    let mut opts = TestOptions::default();
    opts.settings.device_cert_days = 20;
    let s = TestServer::start_with(&pool, opts).await;
    let d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let (csr, key) = generate_csr("renew").unwrap();
    let r = post(&s, &d, "/v1/devices/self/certificate", json!({"csr":csr})).await;
    assert_eq!(r.status, 200);
    let cert = r.json()["certificate"].as_str().unwrap().to_owned();
    let info = parse_cert_pem(&cert).unwrap();
    let new = TestDevice {
        device_id: d.device_id,
        cert_pem: cert,
        key_pem: key.to_string(),
        fingerprint: info.fingerprint,
        session: d.session.clone(),
    };
    assert_eq!(get(&s, &new, "/v1/me").await.status, 200);
    assert_eq!(get(&s, &d, "/v1/me").await.status, 200);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.device_certificates")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 2);
    // Presented certificate's expiry, not the device's newest certificate, governs renewal.
    let long = s
        .state
        .pki
        .ca
        .issue_device(
            &csr,
            &d.device_id.to_string(),
            std::time::SystemTime::now(),
            100,
        )
        .unwrap();
    sqlx::query("INSERT INTO porchlight.device_certificates(household_id,device_id,fingerprint_sha256,serial,not_before,not_after) VALUES ($1,$2,$3,$4,$5,$6)").bind(s.init.household_id).bind(d.device_id).bind(long.fingerprint.as_slice()).bind(&long.serial_hex).bind(chrono::DateTime::<chrono::Utc>::from(long.not_before)).bind(chrono::DateTime::<chrono::Utc>::from(long.not_after)).execute(&pool).await.unwrap();
    let long_device = TestDevice {
        device_id: d.device_id,
        cert_pem: long.cert_pem,
        key_pem: key.to_string(),
        fingerprint: long.fingerprint,
        session: d.session.clone(),
    };
    problem(
        &post(
            &s,
            &long_device,
            "/v1/devices/self/certificate",
            json!({"csr":csr}),
        )
        .await,
        409,
        "renewal-too-early",
    );
    assert_eq!(
        post(&s, &d, "/v1/devices/self/certificate", json!({"csr":csr}))
            .await
            .status,
        200
    );
    s.stop().await;
}
