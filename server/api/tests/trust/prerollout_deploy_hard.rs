use chrono::{Duration, Utc};
use porchlight_api::{
    pki_store::{DataDir, write_secret_file},
    scheduler::{CallbackClock, Scheduler},
    testing::{TestClient, TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use sqlx::PgPool;
use std::{sync::Arc, time::SystemTime};

#[sqlx::test(migrations = "../db/migrations")]
async fn durable_fence_survives_loss_of_run_directory(pool: PgPool) {
    let opts = TestOptions {
        background_derivation: false,
        ..Default::default()
    };
    let durable = opts.settings.maintenance_file.clone();
    let run = durable.with_file_name("run");
    std::fs::create_dir(&run).unwrap();
    std::fs::write(run.join("maintenance"), b"legacy").unwrap();
    write_secret_file(&durable, b"upgrade pending", true).unwrap();
    // Simulated reboot: only the runtime directory disappears before the new API starts.
    std::fs::remove_dir_all(&run).unwrap();
    let server = TestServer::start_with(&pool, opts).await;
    let response = TestClient::new(&server.ca_pem, None)
        .request(
            server.enroll,
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/enroll")
                .header("host", "localhost")
                .header("porchlight-client-version", porchlight_core::VERSION)
                .body(http_body_util::Full::new(hyper::body::Bytes::from_static(
                    b"{}",
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 503);
    assert_eq!(
        response.json()["type"],
        "urn:porchlight:problem:maintenance"
    );
    assert!(durable.exists());
    server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn injected_daily_renewal_changes_running_tls_and_dedupes_alert(pool: PgPool) {
    let server = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let now = Utc::now();
    let pki = &server.state.pki;
    let sans = porchlight_pki::parse_cert_pem(&pki.server_cert_pem)
        .unwrap()
        .sans;
    let old = pki
        .ca
        .reissue_server(
            &pki.server_key_pem,
            &sans,
            SystemTime::from(now - Duration::days(671)),
            730,
        )
        .unwrap();
    let dir = DataDir::new(server.state.settings.data_dir.clone());
    write_secret_file(&dir.server_cert(), old.cert_pem.as_bytes(), false).unwrap();
    let device = server
        .session_device(server.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&server.ca_pem, Some(&device));
    let before = client.connect(server.lan).await.unwrap();
    let before_der = before.get_ref().1.peer_certificates().unwrap()[0].clone();
    assert_eq!(before_der.as_ref(), old.cert_der);
    drop(before);
    let scheduler = Scheduler::from_state(
        &server.state,
        Arc::new(CallbackClock(Arc::new(move || now))),
    )
    .await
    .unwrap();
    scheduler.tick().await.unwrap();
    let after = client.connect(server.lan).await.unwrap();
    let after_der = after.get_ref().1.peer_certificates().unwrap()[0].clone();
    assert_ne!(
        after_der, before_der,
        "new connections must serve the renewed leaf without restart"
    );
    drop(after);
    let disk = porchlight_api::pki_store::load(&dir).unwrap();
    assert_eq!(disk.ca.cert().fingerprint, pki.ca.cert().fingerprint);
    assert!(disk.server_key_pem == pki.server_key_pem);
    let info = porchlight_pki::parse_cert_pem(&disk.server_cert_pem).unwrap();
    assert!(
        info.not_after
            .duration_since(SystemTime::from(now))
            .unwrap()
            .as_secs()
            > 729 * 86400
    );
    let count: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_events where kind like 'server_certificate_expiring_%' and (payload->>'threshold_days')::integer=60").fetch_one(&pool).await.unwrap();
    assert_eq!(count, 1);
    // Registering a new scheduler must not rerun the persisted daily job.
    Scheduler::from_state(
        &server.state,
        Arc::new(CallbackClock(Arc::new(move || now))),
    )
    .await
    .unwrap()
    .tick()
    .await
    .unwrap();
    let count: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_events where kind like 'server_certificate_expiring_%'").fetch_one(&pool).await.unwrap();
    assert_eq!(count, 1);
    server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn certificate_bands_and_measured_disk_need_reach_admin_inbox(pool: PgPool) {
    let server = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let root = &server.state.settings.data_dir;
    let backups = &server.state.settings.backup_dir;
    std::fs::create_dir_all(backups).unwrap();
    write_secret_file(
        &backups.join(".working-space-need"),
        u64::MAX.to_string().as_bytes(),
        true,
    )
    .unwrap();
    let sans = porchlight_pki::parse_cert_pem(&server.state.pki.server_cert_pem)
        .unwrap()
        .sans;
    let initial = Utc::now();
    for (day, band) in [90, 60, 30, 7].into_iter().enumerate() {
        let now = initial + Duration::days(day as i64 * 2);
        let old = server
            .state
            .pki
            .ca
            .reissue_server(
                &server.state.pki.server_key_pem,
                &sans,
                SystemTime::from(now - Duration::days(730 - band)),
                730,
            )
            .unwrap();
        write_secret_file(
            &DataDir::new(root).server_cert(),
            old.cert_pem.as_bytes(),
            false,
        )
        .unwrap();
        let scheduler = Scheduler::from_state(
            &server.state,
            Arc::new(CallbackClock(Arc::new(move || now))),
        )
        .await
        .unwrap();
        scheduler.tick().await.unwrap();
        let count: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_events where kind like 'server_certificate_expiring_%' and (payload->>'threshold_days')::integer=$1 and recipient_member_id=$2").bind(band as i32).bind(server.init.admin_member_id).fetch_one(&pool).await.unwrap();
        assert_eq!(count, 1, "missing certificate band {band}");
        porchlight_api::notifier::drain(&server.state.pool, now + Duration::minutes(1))
            .await
            .unwrap();
    }
    let count: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_sources s join porchlight.notification_events e on e.id=s.event_id where e.kind like 'server_certificate_expiring_%' or e.kind='disk_backup_headroom_low'").fetch_one(&pool).await.unwrap();
    assert!(
        count >= 6,
        "operational alerts must use the existing inbox delivery path"
    );
    let recipients: Vec<uuid::Uuid> = sqlx::query_scalar("select distinct recipient_member_id from porchlight.notification_events where kind like 'server_certificate_expiring_%' or kind='disk_backup_headroom_low'").fetch_all(&pool).await.unwrap();
    assert_eq!(recipients, vec![server.init.admin_member_id]);
    server.stop().await;
}
