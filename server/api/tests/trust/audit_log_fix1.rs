use crate::common::*;
use porchlight_api::testing::{TestClient, TestDevice, TestOptions, TestServer};
use porchlight_core::auth::session::Platform;
use porchlight_pki::{generate_csr, parse_cert_pem};
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrations = "../db/migrations")]
async fn lost_renewal_reply_allows_bounded_replacement_of_unused_successor(pool: PgPool) {
    let mut opts = TestOptions::default();
    opts.settings.device_cert_days = 20;
    let s = TestServer::start_with(&pool, opts).await;
    let d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let (csr, key) = generate_csr("renew fixture").unwrap();
    let body = json!({"csr":csr});
    let first = post(&s, &d, "/v1/devices/self/certificate", body.clone()).await;
    assert_eq!(first.status, 200);
    // The shipped client cannot replay a reply lost after commit: its next attempt is fresh.
    let retry = send(
        &s,
        &d,
        "POST",
        "/v1/devices/self/certificate",
        body.clone(),
        Some("retry-lost"),
    )
    .await;
    assert_eq!(
        retry.status, 200,
        "unused successor must not strand a lost-reply client"
    );
    let unused = first.json()["certificate"].as_str().unwrap().to_owned();
    let abandoned = TestDevice {
        device_id: d.device_id,
        fingerprint: parse_cert_pem(&unused).unwrap().fingerprint,
        cert_pem: unused,
        key_pem: key.to_string(),
        session: d.session.clone(),
    };
    let refused = TestClient::new(&s.ca_pem, Some(&abandoned))
        .request(s.lan, abandoned.request("GET", "/v1/me", json!({}), None))
        .await;
    match refused {
        Ok(response) => assert_eq!(
            response.status, 401,
            "superseded unused leaf must be revoked"
        ),
        Err(error) => assert!(
            format!("{error:?}").contains("CertificateRevoked"),
            "{error:?}"
        ),
    }
    assert!(
        porchlight_db::devices::revoked_fingerprints(&s.state.pool)
            .await
            .unwrap()
            .contains(&abandoned.fingerprint)
    );
    let seen: bool = sqlx::query_scalar("SELECT first_seen_at IS NOT NULL FROM porchlight.device_certificates WHERE fingerprint_sha256=$1")
        .bind(abandoned.fingerprint.as_slice()).fetch_one(&pool).await.unwrap();
    assert!(
        !seen,
        "a refused superseded leaf must not be recorded as used"
    );
    for statement in [
        "UPDATE porchlight.device_certificates SET first_seen_at=NULL WHERE first_seen_at IS NOT NULL",
        "UPDATE porchlight.device_certificates SET revoked_at=NULL WHERE revoked_at IS NOT NULL",
    ] {
        let error = sqlx::query(statement)
            .execute(&s.state.pool)
            .await
            .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("PL422")
        );
    }
    let error = sqlx::query(
        "UPDATE porchlight.device_certificates SET created_at=clock_timestamp() WHERE device_id=$1",
    )
    .bind(d.device_id)
    .execute(&s.state.pool)
    .await
    .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("42501")
    );
    let replay = send(
        &s,
        &d,
        "POST",
        "/v1/devices/self/certificate",
        body.clone(),
        Some("retry-lost"),
    )
    .await;
    assert_eq!(replay.status, 200);
    assert!(retry.body == replay.body);
    assert_eq!(
        post(&s, &d, "/v1/devices/self/certificate", body)
            .await
            .status,
        429
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.device_certificates WHERE device_id=$1",
    )
    .bind(d.device_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 3,
        "revoked unused certificates still count against the daily budget"
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn presented_successor_permanently_fences_fresh_old_leaf_issuance(pool: PgPool) {
    let mut opts = TestOptions::default();
    opts.settings.device_cert_days = 20;
    let s = TestServer::start_with(&pool, opts).await;
    let d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let (csr, key) = generate_csr("renew fixture").unwrap();
    let body = json!({"csr":csr});
    let first = send(
        &s,
        &d,
        "POST",
        "/v1/devices/self/certificate",
        body.clone(),
        Some("used-reply"),
    )
    .await;
    assert_eq!(first.status, 200);
    let cert = first.json()["certificate"].as_str().unwrap().to_owned();
    let successor = TestDevice {
        device_id: d.device_id,
        fingerprint: parse_cert_pem(&cert).unwrap().fingerprint,
        cert_pem: cert,
        key_pem: key.to_string(),
        session: d.session.clone(),
    };
    assert_eq!(get(&s, &successor, "/v1/me").await.status, 200);
    assert_eq!(
        post(&s, &d, "/v1/devices/self/certificate", body.clone())
            .await
            .status,
        429
    );
    let replay = send(
        &s,
        &d,
        "POST",
        "/v1/devices/self/certificate",
        body,
        Some("used-reply"),
    )
    .await;
    assert_eq!(replay.status, 200);
    assert!(first.body == replay.body);
    let seen: bool = sqlx::query_scalar("SELECT first_seen_at IS NOT NULL FROM porchlight.device_certificates WHERE fingerprint_sha256=$1")
        .bind(successor.fingerprint.as_slice()).fetch_one(&pool).await.unwrap();
    assert!(
        seen,
        "successful mTLS authentication must be recorded per leaf"
    );
    s.stop().await;
}
