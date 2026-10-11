use crate::common;
use common::*;
use porchlight_api::{
    revocation,
    testing::{TestOptions, TestServer},
    tls::RevokedSet,
};
use porchlight_core::auth::session::Platform;
use porchlight_pki::generate_csr;
use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;
use tokio::sync::{broadcast, oneshot};

#[sqlx::test(migrations = "../db/migrations")]
async fn renewal_rechecks_after_waiting_for_device_lock(pool: PgPool) {
    let mut opts = TestOptions::default();
    opts.settings.device_cert_days = 20;
    opts.watch_revocations = false;
    let s = TestServer::start_with(&pool, opts).await;
    let d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let mut holder = pool.begin().await.unwrap();
    sqlx::query(
        "UPDATE porchlight.devices SET revoked_at=now() WHERE id=$1 AND revoked_at IS NULL",
    )
    .bind(d.device_id)
    .execute(&mut *holder)
    .await
    .unwrap();
    let (csr, _) = generate_csr("renew").unwrap();
    let (r, ()) = tokio::join!(
        post(&s, &d, "/v1/devices/self/certificate", json!({"csr":csr})),
        async {
            tokio::time::timeout(Duration::from_secs(2),async{loop{
            let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE 'SELECT revoked_at IS NULL%FOR NO KEY UPDATE%')").fetch_one(&pool).await.unwrap();
            if waiting{break;}tokio::time::sleep(Duration::from_millis(5)).await;
        }}).await.expect("renewal must acquire the device row lock");
            holder.commit().await.unwrap();
        }
    );
    problem(&r, 401, "device-revoked");
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.device_certificates WHERE device_id=$1",
    )
    .bind(d.device_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn watcher_reconnects_and_stops(pool: PgPool) {
    let opts = TestOptions {
        watch_revocations: false,
        ..Default::default()
    };
    let s = TestServer::start_with(&pool, opts).await;
    let d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
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
    let pid=tokio::time::timeout(Duration::from_secs(2),async{loop{
        let pid:Option<i32>=sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND state='idle' AND query LIKE 'LISTEN%porchlight_revocation%' LIMIT 1").fetch_optional(&pool).await.unwrap();
        if let Some(pid)=pid{break pid;}tokio::time::sleep(Duration::from_millis(5)).await;
    }}).await.unwrap();
    let killed: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
        .bind(pid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(killed);
    // Revocation while the listener is disconnected must be caught by reconnect reload.
    sqlx::query(
        "UPDATE porchlight.devices SET revoked_at=now() WHERE id=$1 AND revoked_at IS NULL",
    )
    .bind(d.device_id)
    .execute(&pool)
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async { loop {
        let new_pid: Option<i32> = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND application_name='porchlight-revocation-listener' AND state='idle' AND query LIKE 'LISTEN%' AND pid<>$1").bind(pid).fetch_optional(&pool).await.unwrap();
        if new_pid.is_some() { break; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }}).await.expect("watcher did not reconnect with a new LISTEN pid");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap(),
        d.device_id
    );
    assert!(revoked.contains(&d.fingerprint));
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    s.stop().await;
}
