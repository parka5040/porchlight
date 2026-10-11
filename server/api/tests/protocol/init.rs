use porchlight_api::{config::ServeArgs, init::*, pki_store::*};
use porchlight_core::auth::{password::PasswordHasher, token::hash_token};
use porchlight_pki::{
    HouseholdCa, ServerSans, format_fingerprint, key_matches_cert, parse_cert_pem,
};
use sqlx::PgPool;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    time::{Duration, SystemTime},
};

fn params() -> InitParams {
    InitParams {
        household_name: "Home".into(),
        tzid: "America/New_York".into(),
        admin_username: "admin".into(),
        admin_display_name: "Sam".into(),
        admin_password: "unique horse battery staple".to_owned().into(),
        sans: ServerSans {
            dns: vec![
                "LOCALHOST".into(),
                "localhost".into(),
                "porchlight.lan".into(),
            ],
            ips: vec!["127.0.0.1".parse().unwrap(); 2],
        },
        age_recipient: format!("age1{}", "q".repeat(58)),
    }
}
fn hasher() -> PasswordHasher {
    PasswordHasher::with_params(8, 1, 1)
}
fn modes(path: &std::path::Path) {
    let meta = fs::metadata(path).unwrap();
    assert_eq!(
        meta.permissions().mode() & 0o777,
        if meta.is_dir() { 0o700 } else { 0o600 },
        "{}",
        path.display()
    );
    if meta.is_dir() {
        for child in fs::read_dir(path).unwrap() {
            modes(&child.unwrap().path());
        }
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn init_creates_everything(pool: PgPool) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    let out = initialize(&pool, &dir, params(), &hasher(), now)
        .await
        .unwrap();
    assert_eq!(
        out.ca_fingerprint,
        format_fingerprint(
            &parse_cert_pem(&fs::read_to_string(dir.ca_cert()).unwrap())
                .unwrap()
                .fingerprint
        )
    );
    let row: (String, String, String, String, String) = sqlx::query_as("SELECT h.name,u.username,m.role,m.color,c.password_hash FROM porchlight.households h JOIN porchlight.users u ON u.household_id=h.id JOIN porchlight.household_members m ON m.user_id=u.id JOIN porchlight.user_credentials c ON c.user_id=u.id WHERE m.id=$1").bind(out.admin_member_id).fetch_one(&pool).await.unwrap();
    assert_eq!(
        (&*row.0, &*row.1, &*row.2, &*row.3),
        ("Home", "admin", "super_admin", "#FABD2F")
    );
    assert!(hasher().verify("unique horse battery staple", &row.4));
    let token: (Vec<u8>, Vec<String>, chrono::DateTime<chrono::Utc>) = sqlx::query_as(
        "SELECT token_hash,scopes,expires_at FROM porchlight.api_tokens WHERE name='bootstrap'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(token.0, hash_token(&out.bootstrap_token));
    assert_eq!(token.1, ["read", "write", "admin"]);
    assert_eq!(
        token.2.timestamp(),
        chrono::DateTime::<chrono::Utc>::from(now + Duration::from_secs(365 * 86400)).timestamp()
    );
    let cert = parse_cert_pem(&fs::read_to_string(dir.server_cert()).unwrap()).unwrap();
    assert_eq!(cert.sans.dns, ["localhost", "porchlight.lan"]);
    assert_eq!(cert.sans.ips.len(), 1);
    assert_eq!(
        fs::read_to_string(dir.backup_recipient()).unwrap(),
        format!("{}\n", params().age_recipient)
    );
    let key = fs::read_to_string(dir.idempotency_key()).unwrap();
    assert_eq!(key.len(), 65);
    assert!(key.ends_with('\n'));
    assert_eq!(hex::decode(key.trim()).unwrap().len(), 32);
    load_idempotency_key(&dir).unwrap();
    modes(tmp.path());
    let (pki, renewed) = load_or_renew(&dir, now).unwrap();
    assert!(!renewed);
    verify_install(&pool, &pki).await.unwrap();
}
#[sqlx::test(migrations = "../db/migrations")]
async fn init_refuses_second_run(pool: PgPool) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    initialize(&pool, &dir, params(), &hasher(), now)
        .await
        .unwrap();
    finish_init(&dir).unwrap();
    assert!(!dir.root.join(".init-staging").exists());
    let paths = [
        dir.ca_cert(),
        dir.ca_key(),
        dir.server_cert(),
        dir.server_key(),
        dir.idempotency_key(),
        dir.backup_recipient(),
    ];
    let before: Vec<_> = paths.iter().map(|p| fs::read(p).unwrap()).collect();
    let err = initialize(&pool, &dir, params(), &hasher(), now)
        .await
        .err()
        .unwrap();
    assert!(err.is::<AlreadyInitialized>());
    let after: Vec<_> = paths.iter().map(|p| fs::read(p).unwrap()).collect();
    assert_eq!(before, after);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM porchlight.households")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}
#[sqlx::test(migrations = "../db/migrations")]
async fn concurrent_inits_one_wins(pool: PgPool) {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let da = DataDir::new(a.path());
    let db = DataDir::new(b.path());
    let h = hasher();
    let now = SystemTime::now();
    let (a, b) = tokio::join!(
        initialize(&pool, &da, params(), &h, now),
        initialize(&pool, &db, params(), &h, now)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let (loser, error) = if let Err(error) = a {
        (da, error)
    } else {
        (db, b.err().unwrap())
    };
    assert!(error.is::<AlreadyInitialized>(), "{error:#}");
    assert!(!loser.root.join("pki").exists());
    assert!(!loser.root.join(".init-staging").exists());
    for table in ["households", "install_state"] {
        let q = format!("SELECT count(*) FROM porchlight.{table}");
        assert_eq!(
            sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(q))
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn recovers_after_crash_post_commit(pool: PgPool) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    assert!(
        initialize_crashing_after_commit(&pool, &dir, params(), &hasher(), now)
            .await
            .is_err()
    );
    assert!(!dir.ca_cert().exists());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM porchlight.install_state")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    let saved: serde_json::Value = serde_json::from_slice(
        &fs::read(tmp.path().join(".init-staging/init-output.json")).unwrap(),
    )
    .unwrap();
    let mut invalid = params();
    invalid.admin_username = "!".into();
    let out = initialize(&pool, &dir, invalid, &hasher(), now)
        .await
        .unwrap();
    assert_eq!(out.bootstrap_token, saved["bootstrap_token"]);
    assert_eq!(out.ca_fingerprint, saved["ca_fingerprint"]);
    assert!(tmp.path().join(".init-staging/init-output.json").is_file());
    finish_init(&dir).unwrap();
    assert!(!tmp.path().join(".init-staging").exists());
    modes(tmp.path());
    verify_install(&pool, &load_or_renew(&dir, now).unwrap().0)
        .await
        .unwrap();
}
#[sqlx::test(migrations = "../db/migrations")]
async fn stale_staging_is_discarded(pool: PgPool) {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join(".init-staging")).unwrap();
    fs::write(tmp.path().join(".init-staging/junk"), "junk").unwrap();
    initialize(
        &pool,
        &DataDir::new(tmp.path()),
        params(),
        &hasher(),
        SystemTime::now(),
    )
    .await
    .unwrap();
    assert!(!tmp.path().join(".init-staging/junk").exists());
    assert!(tmp.path().join(".init-staging/init-output.json").is_file());
}
#[sqlx::test(migrations = "../db/migrations")]
async fn verify_install_rejects_foreign_data_dir(pool: PgPool) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    initialize(&pool, &dir, params(), &hasher(), now)
        .await
        .unwrap();
    let ca = HouseholdCa::generate("Other", now).unwrap();
    let leaf = ca.issue_server(&params().sans, now, 730).unwrap();
    let pki = ServerPki {
        ca,
        server_cert_pem: leaf.cert.cert_pem,
        server_key_pem: leaf.key_pem,
    };
    assert!(verify_install(&pool, &pki).await.is_err());
}
#[sqlx::test(migrations = "../db/migrations")]
async fn init_refuses_when_db_has_household(pool: PgPool) {
    porchlight_db::testing::seed_household(&pool).await.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let error = initialize(
        &pool,
        &DataDir::new(tmp.path()),
        params(),
        &hasher(),
        SystemTime::now(),
    )
    .await
    .err()
    .unwrap();
    assert!(error.is::<AlreadyInitialized>(), "{error:#}");
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
}
#[sqlx::test(migrations = "../db/migrations")]
async fn init_validates_params(pool: PgPool) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = DataDir::new(tmp.path());
    for i in 0..6 {
        let mut p = params();
        match i {
            0 => p.admin_password = "password1234".to_owned().into(),
            1 => p.age_recipient = "bad".into(),
            2 => p.admin_username = "!".into(),
            3 => p.sans.dns.push("foo bar".into()),
            4 => p.admin_password = "short".to_owned().into(),
            _ => p.admin_password = "x".repeat(1025).into(),
        };
        let error = initialize(&pool, &dir, p, &hasher(), SystemTime::now())
            .await
            .err()
            .unwrap();
        if let Some(reason) = match i {
            0 => Some("breached"),
            4 => Some("too short"),
            5 => Some("too long"),
            _ => None,
        } {
            assert_eq!(
                format!("{error:#}"),
                format!("admin password rejected: {reason}")
            );
        }
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM porchlight.households")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}
#[sqlx::test(migrations = "../db/migrations")]
async fn renews_leaf_under_60_days(pool: PgPool) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    initialize(&pool, &dir, params(), &hasher(), now)
        .await
        .unwrap();
    let before = fs::read(dir.server_cert()).unwrap();
    let key = fs::read(dir.server_key()).unwrap();
    assert!(
        !load_or_renew(&dir, now + Duration::from_secs(600 * 86400))
            .unwrap()
            .1
    );
    assert_eq!(before, fs::read(dir.server_cert()).unwrap());
    let (pki, renewed) = load_or_renew(&dir, now + Duration::from_secs(680 * 86400)).unwrap();
    assert!(renewed);
    assert_ne!(before, fs::read(dir.server_cert()).unwrap());
    assert_eq!(key, fs::read(dir.server_key()).unwrap());
    assert!(key_matches_cert(&pki.server_key_pem, &pki.server_cert_pem).unwrap());
    let cert = parse_cert_pem(&pki.server_cert_pem).unwrap();
    assert_eq!(
        cert.sans,
        parse_cert_pem(std::str::from_utf8(&before).unwrap())
            .unwrap()
            .sans
    );
    assert_eq!(
        chrono::DateTime::<chrono::Utc>::from(cert.not_after).timestamp(),
        chrono::DateTime::<chrono::Utc>::from(now + Duration::from_secs(1410 * 86400)).timestamp()
    );
    use rustls::{
        RootCertStore,
        client::{WebPkiServerVerifier, danger::ServerCertVerifier},
    };
    use rustls_pki_types::{CertificateDer, ServerName, UnixTime, pem::PemObject};
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_file(dir.ca_cert()).unwrap())
        .unwrap();
    let verifier = WebPkiServerVerifier::builder_with_provider(
        std::sync::Arc::new(roots),
        std::sync::Arc::new(rustls::crypto::ring::default_provider()),
    )
    .build()
    .unwrap();
    verifier
        .verify_server_cert(
            &CertificateDer::from_pem_file(dir.server_cert()).unwrap(),
            &[],
            &ServerName::try_from("porchlight.lan").unwrap(),
            &[],
            UnixTime::since_unix_epoch(
                (now + Duration::from_secs(680 * 86400))
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap(),
            ),
        )
        .unwrap();
    modes(tmp.path());
}
#[sqlx::test(migrations = "../db/migrations")]
async fn mismatched_key_refused(pool: PgPool) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    initialize(&pool, &dir, params(), &hasher(), now)
        .await
        .unwrap();
    let ca = HouseholdCa::generate("Other", now).unwrap();
    fs::write(dir.server_key(), ca.key_pem().as_bytes()).unwrap();
    assert!(load_or_renew(&dir, now).is_err());
}
#[sqlx::test(migrations = "../db/migrations")]
async fn published_output_is_retained_for_reprint(pool: PgPool) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    let first = initialize(&pool, &dir, params(), &hasher(), now)
        .await
        .unwrap();
    assert!(dir.root.join(".init-staging/init-output.json").is_file());
    let second = initialize(&pool, &dir, params(), &hasher(), now)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&second).unwrap()
    );
    finish_init(&dir).unwrap();
    assert!(!dir.root.join(".init-staging").exists());
    finish_init(&dir).unwrap(); // acknowledgement is idempotent
    let err = initialize(&pool, &dir, params(), &hasher(), now)
        .await
        .err()
        .unwrap();
    assert!(err.is::<AlreadyInitialized>());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn foreign_server_certificate_refused(pool: PgPool) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    initialize(&pool, &dir, params(), &hasher(), now)
        .await
        .unwrap();
    // Same issuer DN, but a different signing key: comparing names alone is not enough.
    let foreign = HouseholdCa::generate("Home", now).unwrap();
    let leaf = foreign
        .reissue_server(
            &fs::read_to_string(dir.server_key()).unwrap(),
            &params().sans,
            now,
            730,
        )
        .unwrap();
    write_secret_file(&dir.server_cert(), leaf.cert_pem.as_bytes(), false).unwrap();
    let before = fs::read(dir.server_cert()).unwrap();
    assert!(load_or_renew(&dir, now + Duration::from_secs(680 * 86400)).is_err());
    assert_eq!(before, fs::read(dir.server_cert()).unwrap());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn serve_refuses_foreign_install_before_renewal(pool: PgPool) {
    use sqlx::ConnectOptions;
    let tmp = tempfile::tempdir().unwrap();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    initialize(&pool, &dir, params(), &hasher(), now)
        .await
        .unwrap();
    finish_init(&dir).unwrap();
    let foreign = HouseholdCa::generate("Other", now).unwrap();
    // This valid, foreign leaf is due for renewal right now.
    let leaf = foreign.issue_server(&params().sans, now, 1).unwrap();
    write_secret_file(&dir.ca_cert(), foreign.cert().cert_pem.as_bytes(), false).unwrap();
    write_secret_file(&dir.ca_key(), foreign.key_pem().as_bytes(), false).unwrap();
    write_secret_file(&dir.server_cert(), leaf.cert.cert_pem.as_bytes(), false).unwrap();
    write_secret_file(&dir.server_key(), leaf.key_pem.as_bytes(), false).unwrap();
    let before = fs::read(dir.server_cert()).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_porchlight"))
        .args([
            "serve",
            "--enroll-host",
            "localhost",
            "--enroll-cidrs",
            "127.0.0.0/8",
        ])
        .arg("--data-dir")
        .arg(tmp.path())
        .env(
            "PORCHLIGHT_DATABASE_URL",
            pool.connect_options().to_url_lossy().as_str(),
        )
        .env_remove("PORCHLIGHT_MIGRATOR_URL")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not the same installation"));
    assert_eq!(before, fs::read(dir.server_cert()).unwrap());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn database_errors_show_metadata_not_values(pool: PgPool) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = DataDir::new(tmp.path());
    let first = initialize(&pool, &dir, params(), &hasher(), SystemTime::now())
        .await
        .unwrap();
    let error =
        sqlx::query("INSERT INTO porchlight.households (id, name, tzid) VALUES ($1, $2, 'UTC')")
            .bind(first.household_id)
            .bind("secret-value-marker")
            .execute(&pool)
            .await
            .err()
            .unwrap();
    let error = porchlight_api::db::database_error(error);
    let text = format!("{error:#}");
    assert!(text.contains("SQLSTATE 23505"));
    assert!(text.contains("constraint households_pkey"));
    assert!(!text.contains("secret-value-marker"));
    assert!(!text.contains(&first.household_id.to_string()));
}

#[test]
fn missing_pki_error_names_file_and_init_hint() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = DataDir::new(tmp.path());
    let error = load_or_renew(&dir, SystemTime::now()).err().unwrap();
    let text = format!("{error:#}");
    assert!(text.contains(&dir.ca_cert().display().to_string()));
    assert!(text.contains("run `porchlight init`"));
    assert!(error.chain().count() >= 2);
    let error = load_idempotency_key(&dir).err().unwrap();
    assert!(format!("{error:#}").contains(&dir.idempotency_key().display().to_string()));
}

#[test]
fn serve_args_debug_redacts() {
    #[derive(clap::Parser)]
    struct Cli {
        #[command(flatten)]
        args: ServeArgs,
    }
    use clap::Parser;
    let cli = Cli::try_parse_from([
        "test",
        "--database-url",
        "postgres://user:s3cret-marker@localhost/db",
        "--migrator-url",
        "postgres://user:s3cret-marker@localhost/db",
        "--enroll-host",
        "localhost",
        "--enroll-cidrs",
        "127.0.0.0/8",
    ])
    .unwrap();
    assert!(!format!("{:?}", cli.args).contains("s3cret-marker"));
    cli.args.validate().unwrap();
}
#[sqlx::test(migrations = "../db/migrations")]
async fn initializes_with_app_role(pool: PgPool) {
    let app = porchlight_db::testing::pool_as(&pool, porchlight_db::testing::Role::App, None)
        .await
        .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let dir = DataDir::new(tmp.path());
    initialize(&app, &dir, params(), &hasher(), SystemTime::now())
        .await
        .unwrap();
    verify_install(&app, &load_or_renew(&dir, SystemTime::now()).unwrap().0)
        .await
        .unwrap();
}
#[sqlx::test(migrations = "../db/migrations")]
async fn partial_publication_recovery(pool: PgPool) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    assert!(
        initialize_crashing_after_commit(&pool, &dir, params(), &hasher(), now)
            .await
            .is_err()
    );
    let stage = tmp.path().join(".init-staging");
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(stage.join("init-output.json")).unwrap()).unwrap();
    fs::create_dir(tmp.path().join("pki")).unwrap();
    fs::set_permissions(tmp.path().join("pki"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::rename(stage.join("pki/server.key"), dir.server_key()).unwrap();
    fs::rename(stage.join("pki/ca.crt"), dir.ca_cert()).unwrap();
    let output = initialize(&pool, &dir, params(), &hasher(), now)
        .await
        .unwrap();
    assert_eq!(output.bootstrap_token, saved["bootstrap_token"]);
    assert!(stage.join("init-output.json").is_file());
    finish_init(&dir).unwrap();
    assert!(!stage.exists());
    modes(tmp.path());
    verify_install(&pool, &load_or_renew(&dir, now).unwrap().0)
        .await
        .unwrap();
}
#[sqlx::test(migrations = "../db/migrations")]
async fn concurrent_same_directory(pool: PgPool) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = DataDir::new(tmp.path());
    let h = hasher();
    let now = SystemTime::now();
    let (a, b) = tokio::join!(
        initialize(&pool, &dir, params(), &h, now),
        initialize(&pool, &dir, params(), &h, now)
    );
    let a = a.unwrap();
    let b = b.unwrap();
    assert_eq!(a.bootstrap_token, b.bootstrap_token);
    assert_eq!(a.household_id, b.household_id);
    finish_init(&dir).unwrap();
    assert!(!dir.root.join(".init-staging").exists());
    verify_install(&pool, &load_or_renew(&dir, now).unwrap().0)
        .await
        .unwrap();
}
#[sqlx::test(migrations = "../db/migrations")]
async fn cli_json_stdin_and_exit_codes(pool: PgPool) {
    use sqlx::{ConnectOptions, postgres::PgConnectOptions};
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let database = pool.connect_options().get_database().unwrap().to_owned();
    let app: PgConnectOptions = std::env::var("PORCHLIGHT_DATABASE_URL")
        .unwrap()
        .parse()
        .unwrap();
    // Reproduce bootstrap-roles.sql's public-schema grant and migration table ownership in this fixture.
    sqlx::query("GRANT USAGE, CREATE ON SCHEMA public TO porchlight_migrator")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE public._sqlx_migrations OWNER TO porchlight_migrator")
        .execute(&pool)
        .await
        .unwrap();
    let migrator: PgConnectOptions = std::env::var("PORCHLIGHT_MIGRATOR_URL")
        .unwrap()
        .parse()
        .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_porchlight"));
    command
        .args([
            "init",
            "--household",
            "Home",
            "--admin-username",
            "admin",
            "--admin-name",
            "Sam",
            "--age-recipient",
            &params().age_recipient,
            "--json",
        ])
        .arg("--data-dir")
        .arg(tmp.path())
        .env(
            "PORCHLIGHT_DATABASE_URL",
            app.database(&database).to_url_lossy().as_str(),
        )
        .env(
            "PORCHLIGHT_MIGRATOR_URL",
            migrator.database(&database).to_url_lossy().as_str(),
        )
        .env_remove("PORCHLIGHT_INIT_ADMIN_PASSWORD")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A successful commit followed by an output error must leave the token recoverable.
    command.stdout(
        fs::OpenOptions::new()
            .write(true)
            .open("/dev/full")
            .unwrap(),
    );
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"unique horse battery staple\n")
        .unwrap();
    let failed = child.wait_with_output().unwrap();
    assert_eq!(failed.status.code(), Some(1));
    let saved: serde_json::Value = serde_json::from_slice(
        &fs::read(tmp.path().join(".init-staging/init-output.json")).unwrap(),
    )
    .unwrap();
    assert!(
        !String::from_utf8_lossy(&failed.stderr)
            .contains(saved["bootstrap_token"].as_str().unwrap())
    );
    command
        .env(
            "PORCHLIGHT_INIT_ADMIN_PASSWORD",
            "unique horse battery staple",
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    let output = command.output().unwrap();
    assert!(output.status.success(), "CLI recovery failed");
    assert!(!tmp.path().join(".init-staging").exists());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value, saved);
    assert!(
        value["bootstrap_token"]
            .as_str()
            .unwrap()
            .starts_with("plp_")
    );
    assert_eq!(output.stdout.iter().filter(|b| **b == b'\n').count(), 1);
    assert!(
        !String::from_utf8_lossy(&output.stderr)
            .contains(value["bootstrap_token"].as_str().unwrap())
    );
    let hash: String = sqlx::query_scalar("SELECT password_hash FROM porchlight.user_credentials")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(hash.contains("m=65536,t=3,p=4"));
    command
        .env(
            "PORCHLIGHT_INIT_ADMIN_PASSWORD",
            "unique horse battery staple",
        )
        .stdin(Stdio::null());
    let second = command.output().unwrap();
    assert_eq!(second.status.code(), Some(2));
    assert!(second.stdout.is_empty());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already initialized"));
    modes(tmp.path());
}
#[test]
fn settings_defaults_and_validation() {
    #[derive(clap::Parser)]
    struct Cli {
        #[command(flatten)]
        args: ServeArgs,
    }
    use clap::Parser;
    let mut args = Cli::try_parse_from([
        "test",
        "--database-url",
        "postgres://user@localhost/db",
        "--enroll-host",
        "localhost",
        "--enroll-cidrs",
        "127.0.0.0/8",
    ])
    .unwrap()
    .args;
    let settings = args.settings();
    assert_eq!(settings.device_cert_days, 365);
    assert_eq!(settings.device_renew_days, 30);
    assert_eq!(settings.heartbeat, Duration::from_secs(120));
    assert_eq!(settings.endpoints, ["https://porchlight.lan:8443"]);
    assert_eq!(
        porchlight_api::config::Settings::for_tests().heartbeat,
        Duration::from_secs(1)
    );
    args.funnel_addr = Some("127.0.0.1:9443".parse().unwrap());
    assert!(args.validate().is_err());
    args.funnel_peers.push("127.0.0.0/8".parse().unwrap());
    args.validate().unwrap();
    args.enroll_cidrs.clear();
    assert!(args.validate().is_err());
}
#[test]
fn atomic_write_creates_new_only() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("nested/key");
    write_secret_file(&path, b"first", true).unwrap();
    assert!(write_secret_file(&path, b"second", true).is_err());
    assert_eq!(fs::read(path).unwrap(), b"first");
    modes(&tmp.path().join("nested"));
}
