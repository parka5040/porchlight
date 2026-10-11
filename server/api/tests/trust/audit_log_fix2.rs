use axum::body::to_bytes;
use chrono::{Duration, Utc};
use porchlight_api::{
    config::Settings,
    pki_store::ServerPki,
    ratelimit::{RateLimiter, RateLimits},
    routes,
    server::{ConnInfo, Transport},
    state::AppState,
    testing::TestDevice,
};
use porchlight_core::auth::{
    password::PasswordHasher,
    token::{Token, TokenKind},
};
use porchlight_db::{
    auth, devices,
    testing::{Role, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use porchlight_pki::{HouseholdCa, generate_csr, parse_cert_pem};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{sync::Arc, time::SystemTime};
use tower::ServiceExt;
use uuid::Uuid;

// Build genuine pre-0071 issuance/session rows, then run the actual upgrade. No metadata
// columns exist while the old app authenticates: only device/session activity was recorded.
async fn upgrade(pool: &PgPool, successor_used: bool) -> (AppState, TestDevice, TestDevice) {
    let legacy = sqlx::migrate::Migrator::with_migrations(
        porchlight_db::MIGRATOR
            .iter()
            .filter(|m| m.version < 71)
            .cloned()
            .collect(),
    );
    let mut migration_conn = pool.acquire().await.unwrap();
    legacy.run(&mut *migration_conn).await.unwrap();
    let h = seed_household(pool).await.unwrap();
    let app = pool_as(pool, Role::App, None).await.unwrap();
    let ca = HouseholdCa::generate("Upgrade fixture", SystemTime::now()).unwrap();
    let id = Uuid::now_v7();
    let token = Token::generate(TokenKind::Session);
    let actor = Actor {
        member_id: Some(h.super_admin),
        device_id: None,
        request_id: Uuid::now_v7(),
        agent_label: None,
        token_id: None,
    };
    let mut tx = begin_as(&app, &actor).await.unwrap();
    devices::create(
        &mut tx,
        id,
        h.id,
        h.super_admin,
        "Upgrade fixture",
        "desktop",
    )
    .await
    .unwrap();
    let mut leaves = Vec::new();
    for age in [48_i64, 1] {
        let (csr, key) = generate_csr("Upgrade fixture").unwrap();
        let cert = ca
            .issue_device(&csr, &id.to_string(), SystemTime::now(), 20)
            .unwrap();
        sqlx::query("INSERT INTO porchlight.device_certificates(household_id,device_id,fingerprint_sha256,serial,not_before,not_after,created_at) VALUES ($1,$2,$3,$4,$5,$6,$7)")
            .bind(h.id).bind(id).bind(cert.fingerprint.as_slice()).bind(&cert.serial_hex)
            .bind(chrono::DateTime::<Utc>::from(cert.not_before))
            .bind(chrono::DateTime::<Utc>::from(cert.not_after))
            .bind(Utc::now() - Duration::hours(age))
            .execute(&mut *tx).await.unwrap();
        leaves.push(TestDevice {
            device_id: id,
            fingerprint: cert.fingerprint,
            cert_pem: cert.cert_pem,
            key_pem: key.to_string(),
            session: Some(token.expose().to_owned()),
        });
    }
    auth::create_session(
        &mut tx,
        h.id,
        id,
        &token.hash(),
        Utc::now() + Duration::days(1),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let successor = leaves.pop().unwrap();
    let older = leaves.pop().unwrap();
    if successor_used {
        // Pre-upgrade authentication resolves B's fingerprint and the shared live session.
        // Its persisted state is indistinguishable from a lost renewal reply at upgrade.
        let authenticated: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM porchlight.device_certificates c JOIN porchlight.devices d ON d.id=c.device_id JOIN porchlight.sessions s ON s.device_id=d.id WHERE c.fingerprint_sha256=$1 AND c.not_after>clock_timestamp() AND d.revoked_at IS NULL AND s.token_hash=$2 AND s.revoked_at IS NULL AND s.expires_at>clock_timestamp())")
            .bind(successor.fingerprint.as_slice()).bind(token.hash().as_slice())
            .fetch_one(&app).await.unwrap();
        assert!(authenticated);
        sqlx::query("UPDATE porchlight.devices SET last_seen_at=clock_timestamp() WHERE id=$1")
            .bind(id)
            .execute(&app)
            .await
            .unwrap();
    }
    porchlight_db::MIGRATOR
        .run(&mut *migration_conn)
        .await
        .unwrap();
    drop(migration_conn);
    let mut settings = Settings::for_tests();
    settings.device_cert_days = 20;
    let state = AppState::new(
        app,
        Arc::new(ServerPki {
            ca,
            // HTTP router tests inject the authenticated connection fingerprint, not a TLS listener.
            server_cert_pem: String::new(),
            server_key_pem: String::new().into(),
        }),
        settings,
        &[1; 32],
        Arc::new(PasswordHasher::with_params(8, 1, 1)),
        RateLimiter::new(RateLimits::generous()),
    )
    .await
    .unwrap();
    (state, older, successor)
}

async fn call(
    state: &AppState,
    d: &TestDevice,
    method: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let response = routes::mtls_router(state.clone())
        .layer(axum::Extension(ConnInfo {
            transport: Transport::Lan,
            client_addr: None,
            client_cert_fingerprint: Some(d.fingerprint),
        }))
        .oneshot(d.request(method, path, body, None))
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn renew(state: &AppState, older: &TestDevice) -> TestDevice {
    let (csr, key) = generate_csr("Replacement fixture").unwrap();
    let (status, body) = call(
        state,
        older,
        "POST",
        "/v1/devices/self/certificate",
        json!({"csr":csr}),
    )
    .await;
    assert_eq!(
        status, 200,
        "legacy unknown usage must permit bounded recovery"
    );
    let cert = body["certificate"].as_str().unwrap().to_owned();
    let fingerprint = parse_cert_pem(&cert).unwrap().fingerprint;
    let unused: bool = sqlx::query_scalar("SELECT usage_tracked AND first_seen_at IS NULL FROM porchlight.device_certificates WHERE fingerprint_sha256=$1")
        .bind(fingerprint.as_slice()).fetch_one(&state.pool).await.unwrap();
    assert!(unused, "post-upgrade issuance must be confirmed unused");
    TestDevice {
        device_id: older.device_id,
        fingerprint,
        cert_pem: cert,
        key_pem: key.to_string(),
        session: older.session.clone(),
    }
}

async fn assert_legacy_survives(pool: &PgPool, legacy: &TestDevice) {
    let (tracked, revoked): (bool, bool) = sqlx::query_as("SELECT usage_tracked,revoked_at IS NOT NULL FROM porchlight.device_certificates WHERE fingerprint_sha256=$1")
        .bind(legacy.fingerprint.as_slice()).fetch_one(pool).await.unwrap();
    assert!(!tracked, "legacy issuance must retain its untracked origin");
    assert!(
        !revoked,
        "legacy-unknown successor must not be revoked by older-leaf renewal"
    );
    assert!(
        !devices::revoked_fingerprints(pool)
            .await
            .unwrap()
            .contains(&legacy.fingerprint)
    );
}

#[sqlx::test(migrations = false)]
async fn already_used_legacy_successor_survives_older_leaf_renewal(pool: PgPool) {
    let (state, older, legacy) = upgrade(&pool, true).await;
    let replacement = renew(&state, &older).await;
    assert_legacy_survives(&pool, &legacy).await;
    assert_eq!(
        call(&state, &legacy, "GET", "/v1/me", json!({})).await.0,
        200
    );
    let seen: bool = sqlx::query_scalar("SELECT first_seen_at IS NOT NULL FROM porchlight.device_certificates WHERE fingerprint_sha256=$1")
        .bind(legacy.fingerprint.as_slice()).fetch_one(&pool).await.unwrap();
    assert!(
        seen,
        "legacy presentation must transition unknown usage to used"
    );
    let (csr, _) = generate_csr("Refused fixture").unwrap();
    assert_eq!(
        call(
            &state,
            &older,
            "POST",
            "/v1/devices/self/certificate",
            json!({"csr":csr})
        )
        .await
        .0,
        429,
        "used legacy successor fences old-leaf issuance even with budget remaining"
    );
    assert_eq!(
        call(&state, &replacement, "GET", "/v1/me", json!({}))
            .await
            .0,
        200
    );
    assert_legacy_survives(&pool, &legacy).await;
    for (connection, expected) in [(&state.pool, "42501"), (&pool, "PL422")] {
        let error = sqlx::query("UPDATE porchlight.device_certificates SET usage_tracked=true WHERE fingerprint_sha256=$1")
            .bind(legacy.fingerprint.as_slice()).execute(connection).await.unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some(expected),
            "tracking origin must be immutable through both grants and the history trigger"
        );
    }
}

#[sqlx::test(migrations = false)]
async fn legacy_lost_reply_recovers_without_revoking_unknown_successor(pool: PgPool) {
    let (state, older, legacy) = upgrade(&pool, false).await;
    let lost_replacement = renew(&state, &older).await;
    assert_legacy_survives(&pool, &legacy).await;
    // Another lost reply now has both unknown and confirmed-unused successors.
    let recovered = renew(&state, &older).await;
    assert_legacy_survives(&pool, &legacy).await;
    assert!(
        devices::revoked_fingerprints(&pool)
            .await
            .unwrap()
            .contains(&lost_replacement.fingerprint)
    );
    assert_eq!(
        call(&state, &lost_replacement, "GET", "/v1/me", json!({}))
            .await
            .0,
        401
    );
    let (csr, _) = generate_csr("Budget fixture").unwrap();
    assert_eq!(
        call(
            &state,
            &older,
            "POST",
            "/v1/devices/self/certificate",
            json!({"csr":csr})
        )
        .await
        .0,
        429
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.device_certificates WHERE device_id=$1 AND created_at>clock_timestamp()-interval '24 hours'")
        .bind(older.device_id).fetch_one(&pool).await.unwrap();
    assert_eq!(
        count, 3,
        "legacy and revoked issuances still count toward the daily budget"
    );
    assert_eq!(
        call(&state, &recovered, "GET", "/v1/me", json!({})).await.0,
        200
    );
    assert_eq!(
        call(&state, &legacy, "GET", "/v1/me", json!({})).await.0,
        200
    );
}
