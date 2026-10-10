use porchlight_api::{
    init::{InitParams, finish_init, initialize},
    pki_store::{DataDir, load, reissue_server_cert, renew_loaded},
};
use porchlight_core::auth::password::PasswordHasher;
use porchlight_pki::{ServerSans, key_matches_cert, parse_cert_pem};
use sqlx::{ConnectOptions, PgPool};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::Command,
    time::{Duration, SystemTime},
};

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir_in(concat!(env!("CARGO_MANIFEST_DIR"), "/../../var")).unwrap()
}
fn initial_sans() -> ServerSans {
    ServerSans {
        dns: vec!["localhost".into(), "porchlight.lan".into()],
        ips: vec!["127.0.0.1".parse().unwrap()],
    }
}
fn new_sans() -> ServerSans {
    ServerSans {
        dns: vec!["porchlight.lan".into(), "host.example.ts.net".into()],
        ips: vec![
            "127.0.0.1".parse().unwrap(),
            "192.168.77.50".parse().unwrap(),
        ],
    }
}
async fn initialized(pool: &PgPool, dir: &DataDir, now: SystemTime) {
    initialize(
        pool,
        dir,
        InitParams {
            household_name: "Home".into(),
            tzid: "America/New_York".into(),
            admin_username: "admin".into(),
            admin_display_name: "Sam".into(),
            admin_password: "unique horse battery staple".to_owned().into(),
            sans: initial_sans(),
            age_recipient: format!("age1{}", "q".repeat(58)),
        },
        &PasswordHasher::with_params(8, 1, 1),
        now,
    )
    .await
    .unwrap();
    finish_init(dir).unwrap();
}
fn same_set(mut a: ServerSans, mut b: ServerSans) {
    a.dns.sort();
    b.dns.sort();
    a.ips.sort();
    b.ips.sort();
    assert_eq!(a, b);
}
fn cli(dir: &DataDir) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_porchlight"));
    cmd.env_remove("PORCHLIGHT_DATABASE_URL")
        .env_remove("PORCHLIGHT_MIGRATOR_URL")
        .args(["server-cert", "--data-dir"])
        .arg(&dir.root);
    cmd
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reissue_changes_sans_not_keys_or_installation(pool: PgPool) {
    let tmp = tempdir();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    initialized(&pool, &dir, now).await;
    let unchanged = [
        dir.server_key(),
        dir.ca_cert(),
        dir.ca_key(),
        dir.idempotency_key(),
        dir.backup_recipient(),
    ];
    let before: Vec<_> = unchanged.iter().map(|p| fs::read(p).unwrap()).collect();
    let old_leaf = fs::read(dir.server_cert()).unwrap();
    let out = reissue_server_cert(&dir, &new_sans(), now).unwrap();
    assert_eq!(out.previous, initial_sans());
    same_set(out.current.clone(), new_sans());
    let pki = load(&dir).unwrap();
    let parsed = parse_cert_pem(&pki.server_cert_pem).unwrap();
    same_set(parsed.sans, new_sans());
    assert_eq!(parsed.not_after, out.not_after);
    assert_eq!(
        chrono::DateTime::<chrono::Utc>::from(out.not_after).timestamp()
            - chrono::DateTime::<chrono::Utc>::from(now).timestamp(),
        730 * 86400
    );
    pki.ca
        .verify_server_signature(&pki.server_cert_pem)
        .unwrap();
    assert!(key_matches_cert(&pki.server_key_pem, &pki.server_cert_pem).unwrap());
    assert_ne!(old_leaf, fs::read(dir.server_cert()).unwrap());
    assert_eq!(
        before,
        unchanged
            .iter()
            .map(|p| fs::read(p).unwrap())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        fs::metadata(dir.server_cert())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn empty_sans_refused_without_writing(pool: PgPool) {
    let tmp = tempdir();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    initialized(&pool, &dir, now).await;
    let before = fs::read(dir.server_cert()).unwrap();
    let err = reissue_server_cert(
        &dir,
        &ServerSans {
            dns: vec![],
            ips: vec![],
        },
        now,
    )
    .unwrap_err();
    assert!(err.to_string().contains("at least one SAN"));
    assert_eq!(before, fs::read(dir.server_cert()).unwrap());
    let output = cli(&dir).output().unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("at least one SAN")
    );
    assert_eq!(before, fs::read(dir.server_cert()).unwrap());
}

#[test]
fn uninitialized_directory_refused_without_creating_anything() {
    let tmp = tempdir();
    let dir = DataDir::new(tmp.path().join("missing"));
    assert!(reissue_server_cert(&dir, &new_sans(), SystemTime::now()).is_err());
    assert!(!dir.root.exists());
    assert!(!cli(&dir).arg("--show").output().unwrap().status.success());
    assert!(!dir.root.exists());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn renewal_preserves_reissued_sans(pool: PgPool) {
    let tmp = tempdir();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    initialized(&pool, &dir, now).await;
    reissue_server_cert(&dir, &new_sans(), now).unwrap();
    let key = fs::read(dir.server_key()).unwrap();
    let (pki, renewed) = renew_loaded(
        &dir,
        load(&dir).unwrap(),
        now + Duration::from_secs(680 * 86400),
    )
    .unwrap();
    assert!(renewed);
    same_set(
        parse_cert_pem(&pki.server_cert_pem).unwrap().sans,
        new_sans(),
    );
    assert_eq!(key, fs::read(dir.server_key()).unwrap());
    pki.ca
        .verify_server_signature(&pki.server_cert_pem)
        .unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn invalid_dns_refused_without_writing(pool: PgPool) {
    let tmp = tempdir();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    initialized(&pool, &dir, now).await;
    let before = fs::read(dir.server_cert()).unwrap();
    for bad in [
        "bad name",
        "",
        "-bad.example",
        "bad-.example",
        "bad..example",
        "*.example",
        "é.example",
    ] {
        let err = reissue_server_cert(
            &dir,
            &ServerSans {
                dns: vec![bad.into()],
                ips: vec![],
            },
            now,
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid DNS SAN"));
        assert_eq!(before, fs::read(dir.server_cert()).unwrap());
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn normalizes_like_init_and_keeps_canonical_name(pool: PgPool) {
    let tmp = tempdir();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    initialized(&pool, &dir, now).await;
    let out = reissue_server_cert(
        &dir,
        &ServerSans {
            dns: vec!["HOST.EXAMPLE.TS.NET".into(), "host.example.ts.net".into()],
            ips: vec!["192.168.77.50".parse().unwrap(); 2],
        },
        now,
    )
    .unwrap();
    assert_eq!(out.current.dns, ["host.example.ts.net", "porchlight.lan"]);
    assert_eq!(
        out.current.ips,
        ["192.168.77.50".parse::<std::net::IpAddr>().unwrap()]
    );
    assert_eq!(
        out.current,
        parse_cert_pem(&fs::read_to_string(dir.server_cert()).unwrap())
            .unwrap()
            .sans
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn cli_show_is_read_only_and_reissue_outputs_public_metadata(pool: PgPool) {
    let tmp = tempdir();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    initialized(&pool, &dir, now).await;
    let before = fs::read(dir.server_cert()).unwrap();
    let output = cli(&dir)
        .args(["--show", "--json", "--san-dns", "bad name"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let shown: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        shown["current"]["dns"],
        serde_json::json!(["localhost", "porchlight.lan"])
    );
    assert_eq!(shown["previous"], shown["current"]);
    assert!(shown["not_after"].as_str().unwrap().ends_with('Z'));
    assert_eq!(before, fs::read(dir.server_cert()).unwrap());
    let output = cli(&dir)
        .args([
            "--san-dns",
            "host.example.ts.net",
            "--san-dns",
            "porchlight.lan",
            "--san-ip",
            "192.168.77.50",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let changed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(changed["previous"], shown["current"]);
    assert_eq!(
        changed["current"]["dns"],
        serde_json::json!(["host.example.ts.net", "porchlight.lan"])
    );
    assert_eq!(
        changed["current"]["ips"],
        serde_json::json!(["192.168.77.50"])
    );
    assert_eq!(changed.as_object().unwrap().len(), 3);
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .contains("PRIVATE KEY")
    );
    let output = cli(&dir).arg("--show").output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("previous: "));
    assert!(text.contains("\ncurrent: "));
    assert!(text.contains("\nnot_after: "));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn cli_verifies_database_before_show_or_reissue(pool: PgPool) {
    let tmp = tempdir();
    let dir = DataDir::new(tmp.path());
    let now = SystemTime::now();
    initialized(&pool, &dir, now).await;
    let url = pool.connect_options().to_url_lossy();
    assert!(
        cli(&dir)
            .env("PORCHLIGHT_DATABASE_URL", url.as_str())
            .arg("--show")
            .output()
            .unwrap()
            .status
            .success()
    );
    sqlx::query("UPDATE porchlight.install_state SET ca_fingerprint = $1 WHERE id = true")
        .bind(vec![0_u8; 32])
        .execute(&pool)
        .await
        .unwrap();
    let before = fs::read(dir.server_cert()).unwrap();
    for args in [vec!["--show"], vec!["--san-dns", "host.example.ts.net"]] {
        let output = cli(&dir)
            .env("PORCHLIGHT_DATABASE_URL", url.as_str())
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("not the same installation")
        );
        assert_eq!(before, fs::read(dir.server_cert()).unwrap());
    }
}
