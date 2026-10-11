use crate::common;
use porchlight_api::testing::{TestClient, TestOptions, TestServer};
use porchlight_core::auth::session::Platform;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::Duration;

#[sqlx::test(migrations = "../db/migrations")]
async fn maintenance_defers_household_export_step_up_and_blob_leases(pool: PgPool) {
    let server = TestServer::start(&pool).await;
    let device = server
        .session_device(server.init.admin_member_id, Platform::Desktop)
        .await;
    let step = common::post(
        &server,
        &device,
        "/v1/auth/reauth",
        json!({"password": server.admin_password, "action": "household.export"}),
    )
    .await;
    assert_eq!(step.status, 200);
    std::fs::write(server.state.settings.maintenance_file.clone(), b"").unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut request = device.request("GET", "/v1/admin/export/household", json!({}), None);
    request.headers_mut().insert(
        "porchlight-step-up",
        step.json()["step_up_token"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
    );
    let client = TestClient::new(&server.ca_pem, Some(&device));
    let response = client.request(server.lan, request).await.unwrap();
    common::problem(&response, 503, "maintenance");
    let consumed: i64 =
        sqlx::query_scalar("select count(*) from porchlight.step_ups where used_at is not null")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(consumed, 0);
    let leases: i64 = sqlx::query_scalar("select count(*) from porchlight.blob_leases")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(leases, 0);
    server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn init_resume_acknowledges_original_staged_delivery(pool: PgPool) {
    init_resume(pool, false).await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn init_resume_finishes_cleanup_after_staged_output_removal(pool: PgPool) {
    init_resume(pool, true).await;
}

async fn init_resume(pool: PgPool, removed: bool) {
    use porchlight_api::{
        init::{InitParams, initialize},
        pki_store::{DataDir, load, verify_install},
    };
    use porchlight_core::auth::password::PasswordHasher;
    use porchlight_pki::ServerSans;
    use sqlx::ConnectOptions;
    use std::{
        io::Write,
        process::{Command, Stdio},
        time::SystemTime,
    };
    std::fs::create_dir_all("../var/tests").unwrap();
    let tmp = tempfile::tempdir_in("../var/tests").unwrap();
    let dir = DataDir::new(tmp.path());
    let recipient = format!("age1{}", "q".repeat(58));
    let params = InitParams {
        household_name: "Home".into(),
        tzid: "America/New_York".into(),
        admin_username: "admin".into(),
        admin_display_name: "Sam Rivera".into(),
        admin_password: "fixture orchard river".to_owned().into(),
        sans: ServerSans {
            dns: vec!["fixture.invalid".into()],
            ips: vec![],
        },
        age_recipient: recipient.clone(),
    };
    let original = initialize(
        &pool,
        &dir,
        params,
        &PasswordHasher::with_params(8, 1, 1),
        SystemTime::now(),
    )
    .await
    .unwrap();
    let stage = dir.root.join(".init-staging");
    if removed {
        std::fs::remove_file(stage.join("init-output.json")).unwrap();
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_porchlight"))
        .args([
            "init",
            "--household",
            "Home",
            "--admin-username",
            "admin",
            "--admin-name",
            "Sam Rivera",
            "--age-recipient",
            &recipient,
            "--json",
        ])
        .arg("--data-dir")
        .arg(&dir.root)
        .env(
            "PORCHLIGHT_DATABASE_URL",
            pool.connect_options().to_url_lossy().as_str(),
        )
        .env_remove("PORCHLIGHT_MIGRATOR_URL")
        .env_remove("PORCHLIGHT_INIT_ADMIN_PASSWORD")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"fixture orchard river\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(if removed { 2 } else { 0 }));
    assert!(
        !stage.exists(),
        "server resume left its bootstrap delivery staged"
    );
    if removed {
        assert!(
            output.stdout.is_empty(),
            "AlreadyInitialized must not replace saved delivery"
        );
    } else {
        let recovered: porchlight_api::init::InitOutput =
            serde_json::from_slice(&output.stdout).unwrap();
        assert!(
            recovered.bootstrap_token == original.bootstrap_token,
            "resume changed original bootstrap token"
        );
    }
    verify_install(&pool, &load(&dir).unwrap()).await.unwrap();
}

async fn zone_state(pool: &PgPool) -> Value {
    sqlx::query_scalar(
        "select jsonb_build_object(
            'members', (select jsonb_agg(to_jsonb(m) order by id) from porchlight.household_members m),
            'outbox', (select count(*) from porchlight.outbox),
            'load_jobs', (select jsonb_agg(to_jsonb(j) order by member_id) from porchlight.member_load_jobs j),
            'load_signals', (select jsonb_agg(to_jsonb(s) order by id) from porchlight.member_load_signals s),
            'derivation', (select jsonb_agg(to_jsonb(d) order by item_id) from porchlight.item_derivation d))",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn maintenance_defers_get_zone_claims_and_authority_updates(pool: PgPool) {
    let server = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let device = server
        .session_device(server.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&server.ca_pem, Some(&device));
    sqlx::query("insert into porchlight.items(household_id,owner_id,kind,title) values($1,$2,'todo','Zone fixture')")
        .bind(server.init.household_id).bind(server.init.admin_member_id).execute(&pool).await.unwrap();
    let fence = server.state.settings.maintenance_file.clone();
    for zone in ["Europe/London", "Asia/Tokyo"] {
        let before = zone_state(&pool).await;
        assert!(
            before["derivation"].is_array(),
            "fixture needs an affected item"
        );
        std::fs::write(&fence, b"").unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let request = || {
            let mut req = device.request("GET", "/v1/members", json!({}), None);
            req.headers_mut()
                .insert("porchlight-tz", zone.parse().unwrap());
            req
        };
        let response = client.request(server.lan, request()).await.unwrap();
        assert_eq!(response.status, 200, "reads remain available while fenced");
        assert_eq!(
            zone_state(&pool).await,
            before,
            "fenced GET committed zone/authority or derivation changes"
        );
        std::fs::remove_file(&fence).unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let response = client.request(server.lan, request()).await.unwrap();
        assert_eq!(response.status, 200);
        let member = porchlight_db::members::get(
            &mut pool.acquire().await.unwrap(),
            server.init.admin_member_id,
        )
        .await
        .unwrap();
        assert_eq!(member.tzid, zone);
        assert_eq!(member.tz_authority_device_id, Some(device.device_id));
        assert_ne!(
            zone_state(&pool).await,
            before,
            "report must resume after unfencing"
        );
    }
    let read = common::get(&server, &device, "/v1/me").await;
    assert_eq!(read.status, 200);
    server.stop().await;
}
