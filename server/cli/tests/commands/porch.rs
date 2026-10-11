use porchlight_api::{
    ratelimit::Class,
    testing::{TestClient, TestDevice, TestOptions, TestServer},
};
use porchlight_core::auth::{enroll_uri::EnrollUri, session::Platform};
use serde_json::{Value, json};
use sqlx::{
    PgPool, Postgres,
    testing::{TestArgs, TestSupport},
};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
use uuid::Uuid;

async fn run(s: &TestServer, args: &[&str], password: bool, token: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_porch"));
    command
        .args(args)
        .env("PORCHLIGHT_SOCKET", &s.agent_socket)
        .env(
            "PORCHLIGHT_TOKEN",
            token.unwrap_or_else(|| s.bootstrap_token()),
        )
        .env_remove("PORCHLIGHT_PASSWORD")
        .stdin(Stdio::null());
    if password {
        command.env("PORCHLIGHT_PASSWORD", &s.admin_password);
    }
    tokio::task::spawn_blocking(move || {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            if child.try_wait().unwrap().is_some() {
                return child.wait_with_output().unwrap();
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let _ = child.wait();
                panic!("CLI child exceeded 25-second timeout");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })
    .await
    .unwrap()
}
fn ok(out: &Output) {
    assert_eq!(out.status.code(), Some(0), "CLI did not succeed");
}
fn fail(out: &Output) {
    assert_eq!(out.status.code(), Some(1));
}
fn data(out: &Output) -> Value {
    ok(out);
    serde_json::from_slice(&out.stdout).expect("JSON output")
}
fn runtime_dir() -> tempfile::TempDir {
    tempfile::tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var")).unwrap()
}
fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}
fn snapshot(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut result = BTreeMap::new();
    for label in ["emulator", "desktop"] {
        for file in [
            "client.key",
            "client.crt",
            "ca.crt",
            "session.token",
            "enrollment.json",
        ] {
            result.insert(
                format!("{label}/{file}"),
                fs::read(path.join(label).join(file)).unwrap(),
            );
        }
    }
    result
}

#[sqlx::test(migrations = "../db/migrations")]
async fn whoami_json(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let j = data(&run(&s, &["whoami", "--json"], false, None).await);
    let mut scopes: Vec<_> = j["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap())
        .collect();
    scopes.sort_unstable();
    assert_eq!(scopes, ["admin", "read", "write"]);
    assert_eq!(j["username"], "admin");
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn enroll_prints_uri_and_qr(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let out = run(
        &s,
        &["enroll", "--user", "admin", "--device", "phone"],
        false,
        None,
    )
    .await;
    ok(&out);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("porchlight://enroll?") && text.contains("expires at "));
    assert!(text.chars().any(|c| matches!(c, '█' | '▀' | '▄')));
    let j = data(
        &run(
            &s,
            &["--json", "enroll", "--user", "admin", "--device", "other"],
            false,
            None,
        )
        .await,
    );
    let uri = EnrollUri::parse(j["uri"].as_str().unwrap()).unwrap();
    assert_eq!(uri.ca_fingerprint, s.ca_fingerprint);
    assert!(uri.token == j["token"].as_str().unwrap());
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reset_and_redeem(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let j = data(
        &run(
            &s,
            &["user", "reset-password", "admin", "--json"],
            false,
            None,
        )
        .await,
    );
    assert!(j["expires_at"].is_string());
    let r = TestClient::new(&s.ca_pem, Some(&d))
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/auth/reset-password",
                json!({"code":j["code"], "new_password":"a new secure password"}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn device_list_and_revoke(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let j = data(&run(&s, &["device", "list", "--json"], false, None).await);
    assert_eq!(j["devices"][0]["id"], d.device_id.to_string());
    let out = run(&s, &["device", "list"], false, None).await;
    ok(&out);
    let text = String::from_utf8(out.stdout).unwrap();
    for field in ["ID", "USER", "LABEL", "PLATFORM", "REVOKED", "LAST SEEN"] {
        assert!(text.contains(field));
    }
    ok(&run(
        &s,
        &["device", "revoke", &d.device_id.to_string()],
        false,
        None,
    )
    .await);
    let revoked: bool =
        sqlx::query_scalar("SELECT revoked_at IS NOT NULL FROM porchlight.devices WHERE id=$1")
            .bind(d.device_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(revoked);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn token_create_admin_needs_password(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let missing = run(
        &s,
        &["token", "create", "--name", "agent", "--scope", "admin"],
        false,
        None,
    )
    .await;
    fail(&missing);
    assert!(String::from_utf8_lossy(&missing.stderr).contains("No terminal for password prompt"));
    let text = run(
        &s,
        &[
            "token",
            "create",
            "--name",
            "agent-text",
            "--scope",
            "admin",
        ],
        true,
        None,
    )
    .await;
    ok(&text);
    assert!(
        String::from_utf8(text.stdout)
            .unwrap()
            .trim()
            .starts_with("plp_")
    );
    let j = data(
        &run(
            &s,
            &[
                "token",
                "create",
                "--name",
                "agent",
                "--scope",
                "read,write,admin",
                "--days",
                "7",
                "--json",
            ],
            true,
            None,
        )
        .await,
    );
    let token = j["token"].as_str().unwrap();
    assert!(token.starts_with("plp_"));
    ok(&run(&s, &["whoami"], false, Some(token)).await);
    ok(&run(
        &s,
        &["token", "revoke", j["id"].as_str().unwrap()],
        false,
        None,
    )
    .await);
    let out = run(&s, &["whoami"], false, Some(token)).await;
    fail(&out);
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .to_lowercase()
            .contains("unauthenticated")
    );
    let default = data(
        &run(
            &s,
            &["token", "create", "--name", "default", "--json"],
            false,
            None,
        )
        .await,
    );
    assert_eq!(default["scopes"], json!(["read", "write"]));
    // Read-only creation must not request a password, even without a terminal.
    data(
        &run(
            &s,
            &[
                "token", "create", "--name", "reader", "--scope", "read", "--json",
            ],
            false,
            None,
        )
        .await,
    );
    s.stop().await;
}

async fn check_device_files(s: &TestServer, out: &Path) {
    for (label, platform) in [("emulator", "android"), ("desktop", "desktop")] {
        let dir = out.join(label);
        assert_eq!(mode(&dir), 0o700);
        for file in ["client.key", "session.token"] {
            assert_eq!(mode(&dir.join(file)), 0o600);
        }
        for file in ["client.crt", "ca.crt"] {
            assert_eq!(mode(&dir.join(file)), 0o644);
        }
        let j: Value =
            serde_json::from_slice(&fs::read(dir.join("enrollment.json")).unwrap()).unwrap();
        assert!(j["expires"].is_string() && j["endpoints"].is_array());
        let cert_pem = fs::read_to_string(dir.join("client.crt")).unwrap();
        let fingerprint = porchlight_pki::parse_cert_pem(&cert_pem)
            .unwrap()
            .fingerprint;
        let d = TestDevice {
            device_id: Uuid::parse_str(j["device_id"].as_str().unwrap()).unwrap(),
            cert_pem,
            key_pem: fs::read_to_string(dir.join("client.key")).unwrap(),
            fingerprint,
            session: Some(
                fs::read_to_string(dir.join("session.token"))
                    .unwrap()
                    .trim()
                    .to_owned(),
            ),
        };
        let ca = fs::read_to_string(dir.join("ca.crt")).unwrap();
        let r = TestClient::new(&ca, Some(&d))
            .request(s.lan, d.request("GET", "/v1/me", json!(null), None))
            .await
            .unwrap();
        assert_eq!(r.status, 200);
        let stored: String =
            sqlx::query_scalar("SELECT platform FROM porchlight.devices WHERE id=$1")
                .bind(d.device_id)
                .fetch_one(&s.state.pool)
                .await
                .unwrap();
        assert_eq!(stored, platform);
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn dev_init_enrolls_two_devices(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let dir = runtime_dir();
    let path = dir.path().to_str().unwrap();
    let addr = s.enroll.to_string();
    let args = ["dev-init", "--out", path, "--enroll-addr", &addr];
    ok(&run(&s, &args, true, None).await);
    check_device_files(&s, dir.path()).await;
    let before = snapshot(dir.path());
    let refusal = run(&s, &args, true, None).await;
    fail(&refusal);
    assert!(String::from_utf8_lossy(&refusal.stderr).contains("emulator"));
    assert!(
        before == snapshot(dir.path()),
        "refusal changed credential files"
    );
    // Preflight checks every label: no enrollment if a later directory exists.
    let another = runtime_dir();
    fs::create_dir(another.path().join("desktop")).unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.enrollment_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    fail(
        &run(
            &s,
            &["dev-init", "--out", another.path().to_str().unwrap()],
            true,
            None,
        )
        .await,
    );
    assert!(!another.path().join("emulator").exists());
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.enrollment_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, after);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn dev_init_refuses_impostor(pool: PgPool) {
    // Separate SQLx-managed database: both listeners have independent PKI, credentials and buckets.
    let ctx = Postgres::test_context(&TestArgs::new("porch::impostor_server"))
        .await
        .unwrap();
    let other_pool = ctx.pool_opts.connect_with(ctx.connect_opts).await.unwrap();
    sqlx::migrate!("../db/migrations")
        .run(&other_pool)
        .await
        .unwrap();
    let now = Instant::now();
    let options = || {
        let mut o = TestOptions::default();
        o.limits.auth_addr = 1;
        o.rate_clock = Arc::new(move || now);
        o
    };
    let s = TestServer::start_with(&pool, options()).await;
    let other = TestServer::start_with(&other_pool, options()).await;
    let dir = runtime_dir();
    let out = run(
        &s,
        &[
            "dev-init",
            "--out",
            dir.path().to_str().unwrap(),
            "--enroll-addr",
            &other.enroll.to_string(),
        ],
        true,
        None,
    )
    .await;
    fail(&out);
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("invalid peer certificate")
    );
    for (server, db) in [(&s, &pool), (&other, &other_pool)] {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM porchlight.idempotency_keys WHERE scope_kind='enrollment'",
        )
        .fetch_one(db)
        .await
        .unwrap();
        assert_eq!(n, 0);
        let n: i64 = sqlx::query_scalar(
            "SELECT sum(failed_attempts)::bigint FROM porchlight.user_credentials",
        )
        .fetch_one(db)
        .await
        .unwrap();
        assert_eq!(n, 0);
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.devices")
            .fetch_one(db)
            .await
            .unwrap();
        assert_eq!(n, 0);
        // Capacity one, frozen clock: even one enrollment handler call would have exhausted this bucket.
        assert!(
            server
                .state
                .limits
                .check(Class::Auth, None, Some("127.0.0.1".parse().unwrap()))
                .is_ok()
        );
    }
    assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    s.stop().await;
    other.stop().await;
    other_pool.close().await;
    Postgres::cleanup_test(&ctx.db_name).await.unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn dev_init_accepts_ip_literal_enroll_host_in_server_san(pool: PgPool) {
    let mut opts = TestOptions::default();
    opts.settings.enroll_host = "127.0.0.1:8444".into();
    let s = TestServer::start_with(&pool, opts).await;
    let dir = runtime_dir();
    // Override only the random fixture listener port; the issued URI's IP remains
    // the TLS identity, verified against the fixture certificate's IP SAN.
    ok(&run(
        &s,
        &[
            "dev-init",
            "--out",
            dir.path().to_str().unwrap(),
            "--enroll-addr",
            &s.enroll.to_string(),
        ],
        true,
        None,
    )
    .await);
    check_device_files(&s, dir.path()).await;
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn dev_init_keeps_uri_tls_name_with_override(pool: PgPool) {
    let mut opts = TestOptions::default();
    opts.settings.enroll_host = "not-in-server-san.invalid".into();
    let s = TestServer::start_with(&pool, opts).await;
    let dir = runtime_dir();
    let out = run(
        &s,
        &[
            "dev-init",
            "--out",
            dir.path().to_str().unwrap(),
            "--enroll-addr",
            &s.enroll.to_string(),
        ],
        true,
        None,
    )
    .await;
    fail(&out);
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("invalid peer certificate")
    );
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.idempotency_keys WHERE scope_kind='enrollment'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 0);
    assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn dev_init_uses_uri_port(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    assert!(s.state.settings.enroll_host.starts_with("localhost:"));
    let dir = runtime_dir();
    ok(&run(
        &s,
        &["dev-init", "--out", dir.path().to_str().unwrap(), "--json"],
        true,
        None,
    )
    .await);
    check_device_files(&s, dir.path()).await;
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn errors_exit_nonzero(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let out = run(&s, &["whoami"], false, Some("bad-token")).await;
    fail(&out);
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .to_lowercase()
            .contains("unauthenticated")
    );
    let out = run(&s, &["whoami", "--json"], false, Some("bad-token")).await;
    fail(&out);
    let j: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(j["type"], "urn:porchlight:problem:unauthenticated");
    assert_eq!(j["status"], 401);
    assert!(out.stderr.is_empty());
    let text = run(&s, &["whoami"], false, Some("bad-token")).await;
    let stderr = String::from_utf8(text.stderr).unwrap();
    assert!(stderr.contains(j["title"].as_str().unwrap()));
    assert!(stderr.contains(j["detail"].as_str().unwrap_or("Request failed")));
    let missing_socket = s.agent_socket.with_extension("missing");
    let out = run(
        &s,
        &[
            "whoami",
            "--socket",
            missing_socket.to_str().unwrap(),
            "--json",
        ],
        false,
        None,
    )
    .await;
    fail(&out);
    let j: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(j["type"], "urn:porchlight:problem:client-error");
    assert!(j["status"].is_null());
    assert!(!String::from_utf8_lossy(&out.stdout).contains(s.bootstrap_token()));
    assert!(!String::from_utf8_lossy(&out.stderr).contains(s.bootstrap_token()));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn dev_init_custom_devices_and_safe_paths(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let dir = runtime_dir();
    let path = dir.path().to_str().unwrap();
    let out = run(
        &s,
        &["dev-init", "--out", path, "--devices", "../escape:android"],
        true,
        None,
    )
    .await;
    assert_eq!(out.status.code(), Some(2));
    fail(
        &run(
            &s,
            &[
                "dev-init",
                "--out",
                path,
                "--devices",
                "phone:android,phone:desktop",
            ],
            true,
            None,
        )
        .await,
    );
    let missing = run(&s, &["dev-init", "--out", path], false, None).await;
    fail(&missing);
    assert!(String::from_utf8_lossy(&missing.stderr).contains("No terminal for password prompt"));
    assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    let j = data(
        &run(
            &s,
            &[
                "dev-init",
                "--out",
                path,
                "--devices",
                "phone:android",
                "--json",
            ],
            true,
            None,
        )
        .await,
    );
    assert_eq!(j["devices"][0]["label"], "phone");
    assert_eq!(j["devices"].as_array().unwrap().len(), 1);
    assert_eq!(mode(&dir.path().join("phone/client.key")), 0o600);
    let link = runtime_dir();
    std::os::unix::fs::symlink(dir.path().join("phone"), link.path().join("emulator")).unwrap();
    fail(
        &run(
            &s,
            &["dev-init", "--out", link.path().to_str().unwrap()],
            true,
            None,
        )
        .await,
    );
    assert!(!link.path().join("desktop").exists());
    s.stop().await;
}

#[test]
fn usage_and_help_do_not_expose_token() {
    let missing = Command::new(env!("CARGO_BIN_EXE_porch"))
        .arg("whoami")
        .env_remove("PORCHLIGHT_TOKEN")
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
    let token = "plp_not_for_display";
    for args in [
        vec!["--help"],
        vec!["token", "create", "--scope", "invalid"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_porch"))
            .args(&args)
            .env("PORCHLIGHT_TOKEN", token)
            .output()
            .unwrap();
        assert!(!String::from_utf8_lossy(&out.stdout).contains(token));
        assert!(!String::from_utf8_lossy(&out.stderr).contains(token));
        assert_eq!(
            out.status.code(),
            Some(if args[0] == "--help" { 0 } else { 2 })
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn dev_init_partial_failure_reports_saved_and_remaining(pool: PgPool) {
    let now = Instant::now();
    let mut opts = TestOptions::default();
    opts.limits.auth_addr = 1;
    opts.rate_clock = Arc::new(move || now);
    let s = TestServer::start_with(&pool, opts).await;
    let dir = runtime_dir();
    let out = run(
        &s,
        &["dev-init", "--out", dir.path().to_str().unwrap(), "--json"],
        true,
        None,
    )
    .await;
    fail(&out);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Devices enrolled before failure: emulator"));
    assert!(stderr.contains("Credentials retained for: emulator"));
    assert!(stderr.contains("Rerun with --devices desktop:desktop"));
    assert!(dir.path().join("emulator/session.token").exists());
    assert!(!dir.path().join("desktop").exists());
    let problem: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(problem["status"], 429);
    s.stop().await;
}

#[test]
fn token_scope_help_documents_least_privilege_and_password() {
    let out = Command::new(env!("CARGO_BIN_EXE_porch"))
        .args(["token", "create", "--help"])
        .env_remove("PORCHLIGHT_TOKEN")
        .output()
        .unwrap();
    ok(&out);
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("least-privilege default: read,write"));
    assert!(help.contains("admin implies write implies read"));
    assert!(help.contains("every live token can read"));
    assert!(help.contains("admin requires password re-entry"));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn private_token_file_overrides_ambient_token_and_rejects_public_file(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let dir = runtime_dir();
    let path = dir.path().join("caller.pat");
    fs::write(&path, format!("{}\n", s.bootstrap_token())).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let args = ["--token-file", path.to_str().unwrap(), "--json", "whoami"];
    let out = run(&s, &args, false, Some("invalid-ambient-token")).await;
    assert_eq!(data(&out)["member_id"], s.init.admin_member_id.to_string());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let out = run(&s, &args, false, None).await;
    fail(&out);
    let problem: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        problem["detail"]
            .as_str()
            .unwrap()
            .contains("no group/other permissions")
    );
    let human = run(
        &s,
        &["--token-file", path.to_str().unwrap(), "whoami"],
        false,
        None,
    )
    .await;
    fail(&human);
    assert!(String::from_utf8_lossy(&human.stderr).contains("no group/other permissions"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains(s.bootstrap_token()));
    assert!(!String::from_utf8_lossy(&out.stdout).contains(s.bootstrap_token()));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn notification_commands_and_versioned_prefs(pool: PgPool) {
    let t = chrono::DateTime::parse_from_rfc3339("2026-10-05T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(move || t),
            ..Default::default()
        },
    )
    .await;
    let prefs = data(&run(&s, &["notifications", "prefs", "--json"], false, None).await);
    assert_eq!(prefs["version"], 1);
    let dir = runtime_dir();
    let input = dir.path().join("prefs.json");
    let mut p = prefs["prefs"].clone();
    p["assignments"] = json!(false);
    fs::write(&input, serde_json::to_vec(&p).unwrap()).unwrap();
    let changed = data(
        &run(
            &s,
            &[
                "notifications",
                "set-prefs",
                "--file",
                input.to_str().unwrap(),
                "--version",
                "1",
                "--json",
            ],
            false,
            None,
        )
        .await,
    );
    assert_eq!(changed["version"], 2);
    sqlx::query("insert into porchlight.notification_events(household_id,recipient_member_id,kind,entity_type,entity_id,created_at) values($1,$2,'completed','member',$2,$3)").bind(s.init.household_id).bind(s.init.admin_member_id).bind(t).execute(&pool).await.unwrap();
    porchlight_api::notifier::drain(&pool, t).await.unwrap();
    let inbox = data(
        &run(
            &s,
            &["notifications", "list", "--unread", "--json"],
            false,
            None,
        )
        .await,
    );
    assert_eq!(inbox["items"].as_array().unwrap().len(), 1);
    let inspected = data(
        &run(
            &s,
            &[
                "notifications",
                "inspect",
                "--member",
                &s.init.admin_member_id.to_string(),
                "--json",
            ],
            false,
            None,
        )
        .await,
    );
    assert_eq!(inspected, inbox);
    let id = inbox["items"][0]["id"].as_str().unwrap();
    let read = data(&run(&s, &["notifications", "read", id, "--json"], false, None).await);
    assert_eq!(read["id"], id);
    assert_eq!(read["read_at"], "2026-10-05T12:00:00Z");
    let all = data(&run(&s, &["notifications", "read-all", "--json"], false, None).await);
    assert_eq!(all["count"], 0);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn collaboration_commands_json_golden(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let item = client
        .request(
            s.lan,
            d.request("POST", "/v1/items", json!({"title":"CLI collab"}), None),
        )
        .await
        .unwrap()
        .json();
    let id = item["id"].as_str().unwrap();
    let added = data(
        &run(
            &s,
            &["comments", "add", id, "Golden comment", "--json"],
            false,
            None,
        )
        .await,
    );
    assert_eq!(added["body"], "Golden comment");
    assert_eq!(added["author_id"], Value::Null);
    assert!(added["author_token_id"].is_string());
    let listed = data(&run(&s, &["comments", "list", id, "--json"], false, None).await);
    assert_eq!(listed, json!({"items":[added],"next_cursor":null}));
    let dir = runtime_dir();
    let file = dir.path().join("notes.txt");
    fs::write(&file, b"CLI attachment").unwrap();
    let attached = data(
        &run(
            &s,
            &["attach", id, file.to_str().unwrap(), "--json"],
            false,
            None,
        )
        .await,
    );
    assert_eq!(attached["original_name"], "notes.txt");
    assert_eq!(attached["size"], 14);
    assert_eq!(attached["content_type"], "text/plain");
    let bad = run(
        &s,
        &["attach", id, dir.path().to_str().unwrap(), "--json"],
        false,
        None,
    )
    .await;
    fail(&bad);
    let error: Value = serde_json::from_slice(&bad.stdout).unwrap();
    assert_eq!(error["type"], "urn:porchlight:problem:client-error");
    s.stop().await;
}

// S8a: real socket golden wire shape and mutation version headers.
#[sqlx::test(migrations = "../db/migrations")]
async fn import_and_feed_command_families(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(|| "2026-10-06T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let dir = runtime_dir();
    let file = dir.path().join("school.ics");
    fs::write(
        &file,
        include_str!("../../../core/tests/fixtures/ics/school.ics"),
    )
    .unwrap();
    let target = "admin".to_string();
    let preview = data(
        &run(
            &s,
            &[
                "--json",
                "import",
                "ics",
                file.to_str().unwrap(),
                "--member",
                &target,
                "--preview",
            ],
            false,
            None,
        )
        .await,
    );
    assert_eq!(preview["preview"], true);
    assert_eq!(preview["created"], 19);
    let imported = data(
        &run(
            &s,
            &[
                "--json",
                "import",
                "ics",
                file.to_str().unwrap(),
                "--member",
                &target,
            ],
            false,
            None,
        )
        .await,
    );
    assert_eq!(imported["created"], 19);
    assert_eq!(imported["preview"], false);
    let reimported = data(
        &run(
            &s,
            &[
                "--json",
                "import",
                "ics",
                file.to_str().unwrap(),
                "--member",
                &target,
            ],
            false,
            None,
        )
        .await,
    );
    assert_eq!(reimported["created"], 0);
    let source = data(
        &run(
            &s,
            &[
                "--json",
                "feed",
                "add",
                "--name",
                "School",
                "--url",
                "https://example.org/calendar",
                "--member",
                &target,
            ],
            false,
            None,
        )
        .await,
    );
    let id = source["id"].as_str().unwrap();
    assert_eq!(source["refresh_seconds"], 86400);
    let list = data(&run(&s, &["--json", "feed", "list"], false, None).await);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert!(list["next_cursor"].is_null());
    let version = source["version"].to_string();
    let queued = data(
        &run(
            &s,
            &["--json", "feed", "refresh", id, "--if-match", &version],
            false,
            None,
        )
        .await,
    );
    assert_eq!(queued["last_status"], "queued");
    let stale = run(
        &s,
        &["--json", "feed", "rm", id, "--if-match", &version],
        false,
        None,
    )
    .await;
    fail(&stale);
    assert_eq!(
        serde_json::from_slice::<Value>(&stale.stdout).unwrap()["type"],
        "urn:porchlight:problem:version-conflict"
    );
    let version = queued["version"].to_string();
    let removed = data(
        &run(
            &s,
            &["--json", "feed", "rm", id, "--if-match", &version],
            false,
            None,
        )
        .await,
    );
    assert!(!removed["deleted_at"].is_null());
    s.stop().await;
}

// s8b: golden planning JSON and version header over the actual agent socket.
#[sqlx::test(migrations = "../db/migrations")]
async fn schedule_availability_and_suggest_golden(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let member = s.init.admin_member_id.to_string();
    let empty = data(
        &run(
            &s,
            &["schedule", "availability", &member, "--json"],
            false,
            None,
        )
        .await,
    );
    assert_eq!(
        empty,
        json!({"member_id":member,"version":0,"windows":[],"conflicts":[]})
    );
    let dir = runtime_dir();
    let file = dir.path().join("windows.json");
    fs::write(
        &file,
        r#"{"windows":[{"weekday":0,"start":"09:00:00","end":"17:00:00"}]}"#,
    )
    .unwrap();
    let changed = data(
        &run(
            &s,
            &[
                "schedule",
                "set-availability",
                &member,
                "--file",
                file.to_str().unwrap(),
                "--version",
                "0",
                "--json",
            ],
            false,
            None,
        )
        .await,
    );
    assert_eq!(
        changed["windows"],
        json!([{"weekday":0,"start":"09:00:00","end":"17:00:00"}])
    );
    let suggest = data(
        &run(
            &s,
            &["schedule", "suggest", "--member", &member, "--json"],
            false,
            None,
        )
        .await,
    );
    assert_eq!(suggest["suggestions"], json!([]));
    assert_eq!(suggest["unplaced"], json!([]));
    assert!(Uuid::parse_str(suggest["run_id"].as_str().unwrap()).is_ok());
    assert_eq!(
        data(
            &run(
                &s,
                &["schedule", "list", "--member", &member, "--json"],
                false,
                None
            )
            .await
        ),
        json!({"items":[],"next_cursor":null})
    );
    let device = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&device));
    for title in ["CLI first", "CLI second"] {
        let made = client
            .request(
                s.lan,
                device.request(
                    "POST",
                    "/v1/items",
                    json!({"title":title,"due_date":"2026-10-12","est_minutes":30}),
                    None,
                ),
            )
            .await
            .unwrap();
        assert_eq!(made.status, 201);
    }
    let proposed = data(
        &run(
            &s,
            &["schedule", "suggest", "--member", &member, "--json"],
            false,
            None,
        )
        .await,
    );
    assert_eq!(proposed["suggestions"].as_array().unwrap().len(), 2);
    let first = &proposed["suggestions"][0];
    let accepted = data(
        &run(
            &s,
            &[
                "schedule",
                "accept",
                first["id"].as_str().unwrap(),
                "--version",
                &first["occurrence_version"].to_string(),
                "--json",
            ],
            false,
            None,
        )
        .await,
    );
    assert_eq!(accepted["status"], "accepted");
    assert_eq!(accepted["version"], 2);
    assert_eq!(accepted["occurrence_version"], 2);
    let second = &proposed["suggestions"][1];
    let dismissed = data(
        &run(
            &s,
            &[
                "schedule",
                "dismiss",
                second["id"].as_str().unwrap(),
                "--version",
                &second["version"].to_string(),
                "--json",
            ],
            false,
            None,
        )
        .await,
    );
    assert_eq!(dismissed["status"], "dismissed");
    assert_eq!(dismissed["version"], 2);
    let page = data(
        &run(
            &s,
            &["schedule", "list", "--limit", "1", "--json"],
            false,
            None,
        )
        .await,
    );
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert!(page["next_cursor"].is_string());
    let next = data(
        &run(
            &s,
            &[
                "schedule",
                "list",
                "--limit",
                "1",
                "--cursor",
                page["next_cursor"].as_str().unwrap(),
                "--json",
            ],
            false,
            None,
        )
        .await,
    );
    assert_eq!(next["items"].as_array().unwrap().len(), 1);
    assert!(next["next_cursor"].is_null());
    assert_ne!(page["items"][0]["id"], next["items"][0]["id"]);
    let dependent:Uuid=sqlx::query_scalar("insert into porchlight.household_members(household_id,role,display_name,color,tzid) values($1,'dependent','CLI child','#AABBCC','America/New_York') returning id").bind(s.init.household_id).fetch_one(&pool).await.unwrap();
    let visit=client.request(s.lan,device.request("POST","/v1/items",json!({"title":"CLI visit","kind":"appointment","time_mode":"fixed","tzid":"America/New_York","starts_at":"2026-10-12T14:00:00Z","ends_at":"2026-10-12T15:00:00Z","subject_member_id":dependent}),None)).await.unwrap();
    assert_eq!(visit.status, 201);
    let candidates = data(
        &run(
            &s,
            &[
                "schedule",
                "candidates",
                visit.json()["id"].as_str().unwrap(),
                "--json",
            ],
            false,
            None,
        )
        .await,
    );
    assert_eq!(
        candidates["candidates"],
        json!([{"member_id":member,"reason":"free_inside_availability"}])
    );
    assert_eq!(candidates["pending_derivation"], false);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn exports_stream_stdout_files_and_refuse_overwrite(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let file = s.state.settings.data_dir.join("member-export.json");
    let path = file.to_str().unwrap();
    let out = run(
        &s,
        &["--json", "export", "--member", "admin", "--out", path],
        false,
        None,
    )
    .await;
    ok(&out);
    let summary: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(summary["bytes"].as_u64().unwrap() > 0);
    let data: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(data["format_version"], 1);
    assert!(
        data["appointments_ics"]
            .as_str()
            .unwrap()
            .contains("BEGIN:VCALENDAR")
    );
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let again = run(
        &s,
        &["export", "--member", "admin", "--out", path],
        false,
        None,
    )
    .await;
    fail(&again);
    let out = run(&s, &["export", "--member", "admin"], false, None).await;
    ok(&out);
    let _: Value = serde_json::from_slice(&out.stdout).unwrap();
    let out = run(&s, &["export", "--household"], false, None).await;
    ok(&out);
    assert_eq!(&out.stdout[..2], &[0x1f, 0x8b]);
    let out = run(
        &s,
        &["export", "--household", "--member", "admin"],
        false,
        None,
    )
    .await;
    assert_eq!(
        out.status.code(),
        Some(2),
        "mutually exclusive selector is a clap usage error"
    );
    let out = run(&s, &["--json", "backups"], false, None).await;
    ok(&out);
    assert!(serde_json::from_slice::<Value>(&out.stdout).unwrap()["latest"].is_null());
    s.stop().await;
}
