mod common;
use common::*;
use porchlight_api::testing::trust;
use porchlight_api::{
    revocation,
    testing::{TestClient, TestDevice, TestOptions, TestServer},
    tls::RevokedSet,
};
use porchlight_core::auth::session::Platform;
use porchlight_pki::{generate_csr, parse_cert_pem};
use serde_json::json;
use sqlx::PgPool;
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::sync::{Barrier, broadcast, oneshot};
use trust::*;
#[sqlx::test(migrations = "../db/migrations")]
async fn self_revoke_needs_step_up(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let path = format!("/v1/devices/{}/revoke", d.device_id);
    problem(
        &post(&s, &d, &path, json!({})).await,
        403,
        "step-up-required",
    );
    let r = post(
        &s,
        &d,
        "/v1/auth/reauth",
        json!({"password":s.admin_password,"action":"device.revoke"}),
    )
    .await;
    assert_eq!(r.status, 200);
    let j = r.json();
    let t = j["step_up_token"].as_str().unwrap();
    let other = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let mut req = other.request("POST", &path, json!({}), None);
    req.headers_mut()
        .insert("porchlight-step-up", t.parse().unwrap());
    problem(
        &TestClient::new(&s.ca_pem, Some(&other))
            .request(s.lan, req)
            .await
            .unwrap(),
        403,
        "step-up-required",
    );
    sqlx::query("UPDATE porchlight.step_ups SET action='device.other' WHERE used_at IS NULL")
        .execute(&pool)
        .await
        .unwrap();
    let mut req = d.request("POST", &path, json!({}), None);
    req.headers_mut()
        .insert("porchlight-step-up", t.parse().unwrap());
    problem(
        &TestClient::new(&s.ca_pem, Some(&d))
            .request(s.lan, req)
            .await
            .unwrap(),
        403,
        "step-up-required",
    );
    sqlx::query("UPDATE porchlight.step_ups SET action='device.revoke' WHERE used_at IS NULL")
        .execute(&pool)
        .await
        .unwrap();
    let mut req = d.request("POST", &path, json!({}), Some("terminal"));
    req.headers_mut()
        .insert("porchlight-step-up", t.parse().unwrap());
    assert_eq!(
        TestClient::new(&s.ca_pem, Some(&d))
            .request(s.lan, req)
            .await
            .unwrap()
            .status,
        200
    );
    assert!(
        TestClient::new(&s.ca_pem, Some(&d))
            .request(s.lan, d.request("GET", "/v1/me", json!({}), None))
            .await
            .is_err()
    );
    let used: bool = sqlx::query_scalar("SELECT used_at IS NOT NULL FROM porchlight.step_ups")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(used);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn step_up_single_use_and_already_revoked_sessions(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let target = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    // Leave a previously revoked session in the matching set.
    sqlx::query("INSERT INTO porchlight.sessions(household_id,device_id,token_hash,expires_at,revoked_at) VALUES ($1,$2,$3,now()+interval '1 day',now())").bind(s.init.household_id).bind(target.device_id).bind([7u8;32].as_slice()).execute(&pool).await.unwrap();
    let r = post(
        &s,
        &d,
        "/v1/auth/reauth",
        json!({"password":s.admin_password,"action":"device.revoke"}),
    )
    .await;
    let j = r.json();
    let t = j["step_up_token"].as_str().unwrap();
    let path = format!("/v1/devices/{}/revoke", target.device_id);
    for status in [200, 403] {
        let mut req = d.request("POST", &path, json!({}), None);
        req.headers_mut()
            .insert("porchlight-step-up", t.parse().unwrap());
        assert_eq!(
            TestClient::new(&s.ca_pem, Some(&d))
                .request(s.lan, req)
                .await
                .unwrap()
                .status,
            status
        );
    }
    let a = admin(
        &s,
        "POST",
        &format!("/v1/admin/devices/{}/revoke", target.device_id),
        json!({}),
    )
    .await;
    let b = admin(
        &s,
        "POST",
        &format!("/v1/admin/devices/{}/revoke", target.device_id),
        json!({}),
    )
    .await;
    assert_eq!(a.status, 200);
    assert_eq!(a.json(), b.json());
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn revocation_bites_everywhere(pool: PgPool) {
    use http_body_util::BodyExt;
    use hyper_util::rt::TokioIo;
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
    let fp = parse_cert_pem(&cert).unwrap().fingerprint;
    let new = TestDevice {
        device_id: d.device_id,
        cert_pem: cert,
        key_pem: key.to_string(),
        fingerprint: fp,
        session: d.session.clone(),
    };
    let io = TestClient::new(&s.ca_pem, Some(&d))
        .connect(s.lan)
        .await
        .unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(io))
        .await
        .unwrap();
    let task = tokio::spawn(conn);
    let first = sender
        .send_request(d.request("GET", "/v1/me", json!({}), None))
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    first.into_body().collect().await.unwrap();
    assert_eq!(
        admin(
            &s,
            "POST",
            &format!("/v1/admin/devices/{}/revoke", d.device_id),
            json!({})
        )
        .await
        .status,
        200
    );
    let r = sender
        .send_request(d.request("GET", "/v1/me", json!({}), None))
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let j: serde_json::Value =
        serde_json::from_slice(&r.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(j["type"], "urn:porchlight:problem:device-revoked");
    for d in [&d, &new] {
        assert!(
            TestClient::new(&s.ca_pem, Some(d))
                .request(s.lan, d.request("GET", "/v1/me", json!({}), None))
                .await
                .is_err()
        );
    }
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.sessions WHERE device_id=$1 AND revoked_at IS NULL",
    )
    .bind(d.device_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live, 0);
    drop(sender);
    task.abort();
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn revocation_propagates_between_processes(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let revoked = RevokedSet::default();
    let (events, mut rx) = broadcast::channel(16);
    let (stop, shutdown) = oneshot::channel();
    let task = tokio::spawn(revocation::run(
        s.state.pool.clone(),
        revoked.clone(),
        events.clone(),
        async {
            let _ = shutdown.await;
        },
    ));
    // Wait until LISTEN is established, not a scheduler-dependent sleep.
    tokio::time::timeout(Duration::from_secs(2),async {loop{let n:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND query LIKE 'LISTEN%porchlight_revocation%' AND state='idle'").fetch_one(&pool).await.unwrap();if n>=2{break;}tokio::time::sleep(Duration::from_millis(10)).await;}}).await.unwrap();
    // LISTEN is sufficient: startup catch-up must broadcast any concurrent revoke too.
    // The deadline is far below the thirty-second poll period.
    sqlx::query(
        "UPDATE porchlight.devices SET revoked_at=now() WHERE id=$1 AND revoked_at IS NULL",
    )
    .bind(d.device_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap(),
        d.device_id
    );
    assert!(revoked.contains(&d.fingerprint));
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
    let second = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    sqlx::query(
        "UPDATE porchlight.devices SET revoked_at=now() WHERE id=$1 AND revoked_at IS NULL",
    )
    .bind(second.device_id)
    .execute(&pool)
    .await
    .unwrap();
    let mut known = HashSet::from([d.device_id]);
    revocation::reload(&s.state.pool, &revoked, &mut known, &events)
        .await
        .unwrap();
    assert!(revoked.contains(&second.fingerprint));
    assert_eq!(rx.recv().await.unwrap(), second.device_id);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn remote_revoke_between_seed_and_listen_is_broadcast(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let revoked = RevokedSet::default();
    let (events, mut rx) = broadcast::channel(16);
    let (stop, shutdown) = oneshot::channel();
    let request_pool = s.state.pool.clone();
    let id = d.device_id;
    let task = tokio::spawn(revocation::run_after_seed(
        request_pool,
        revoked.clone(),
        events,
        async {
            let _ = shutdown.await;
        },
        async move {
            // Commit in the startup gap, without relying on scheduler delays or local publication.
            sqlx::query("UPDATE porchlight.devices SET revoked_at=now() WHERE id=$1")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        },
    ));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap(),
        id
    );
    assert!(revoked.contains(&d.fingerprint));
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn non_admin_cannot_revoke_others(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    user(&pool, &s).await;
    let admin_device = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let d = s
        .enroll("alice", &s.admin_password, Platform::Desktop)
        .await;
    let r = post(
        &s,
        &d,
        "/v1/auth/reauth",
        json!({"password":s.admin_password,"action":"device.revoke"}),
    )
    .await;
    let j = r.json();
    let mut req = d.request(
        "POST",
        &format!("/v1/devices/{}/revoke", admin_device.device_id),
        json!({}),
        None,
    );
    req.headers_mut().insert(
        "porchlight-step-up",
        j["step_up_token"].as_str().unwrap().parse().unwrap(),
    );
    problem(
        &TestClient::new(&s.ca_pem, Some(&d))
            .request(s.lan, req)
            .await
            .unwrap(),
        404,
        "not-found",
    );
    let list = get(&s, &d, "/v1/devices").await;
    assert_eq!(list.json()["devices"].as_array().unwrap().len(), 1);
    assert_eq!(
        get(&s, &admin_device, "/v1/devices").await.json()["devices"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn renewal_vs_revoke_race(pool: PgPool) {
    let mut opts = TestOptions::default();
    opts.settings.device_cert_days = 20;
    opts.watch_revocations = false;
    let s = TestServer::start_with(&pool, opts).await;
    for _ in 0..20 {
        let d = s
            .enroll("admin", &s.admin_password, Platform::Desktop)
            .await;
        let (csr, _) = generate_csr("renew").unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let b = barrier.clone();
        // Pre-established TLS ensures losing renewal is an HTTP 401, not just handshake failure.
        let io = TestClient::new(&s.ca_pem, Some(&d))
            .connect(s.lan)
            .await
            .unwrap();
        let path = format!("/v1/admin/devices/{}/revoke", d.device_id);
        let (a, r) = tokio::join!(
            async {
                b.wait().await;
                TestClient::send(
                    io,
                    d.request(
                        "POST",
                        "/v1/devices/self/certificate",
                        json!({"csr":csr}),
                        None,
                    ),
                )
                .await
                .unwrap()
            },
            async {
                barrier.wait().await;
                admin(&s, "POST", &path, json!({})).await
            }
        );
        assert_eq!(r.status, 200);
        assert!([200, 401].contains(&a.status));
        if a.status == 401 {
            problem(&a, 401, "device-revoked");
        }
        let fingerprints: Vec<Vec<u8>> = sqlx::query_scalar(
            "SELECT fingerprint_sha256 FROM porchlight.device_certificates WHERE device_id=$1",
        )
        .bind(d.device_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        for fp in fingerprints {
            assert!(s.state.revoked.contains(&fp.try_into().unwrap()));
        }
    }
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn stale_reload_never_shrinks(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let revoked = RevokedSet::default();
    revoked.insert([9; 32]);
    let (events, _) = broadcast::channel(8);
    revocation::reload(&s.state.pool, &revoked, &mut HashSet::new(), &events)
        .await
        .unwrap();
    assert!(revoked.contains(&[9; 32]));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn startup_seeds_history_without_broadcast(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    assert_eq!(
        admin(
            &s,
            "POST",
            &format!("/v1/admin/devices/{}/revoke", d.device_id),
            json!({})
        )
        .await
        .status,
        200
    );
    let revoked = RevokedSet::default();
    let (events, mut rx) = broadcast::channel(16);
    let (stop, shutdown) = oneshot::channel();
    let task = tokio::spawn(revocation::run(
        s.state.pool.clone(),
        revoked.clone(),
        events,
        async {
            let _ = shutdown.await;
        },
    ));
    tokio::time::timeout(Duration::from_secs(2), async {
        while !revoked.contains(&d.fingerprint) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), rx.recv())
            .await
            .is_err()
    );
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn locally_published_revocation_not_broadcast_twice(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let mut rx = s.state.revocations.subscribe();
    assert_eq!(
        admin(
            &s,
            "POST",
            &format!("/v1/admin/devices/{}/revoke", d.device_id),
            json!({})
        )
        .await
        .status,
        200
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap(),
        d.device_id
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .is_err()
    );
    s.stop().await;
}
