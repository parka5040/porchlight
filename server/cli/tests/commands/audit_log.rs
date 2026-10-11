use http_body_util::Full;
use hyper::{Request, body::Bytes};
use porchlight_api::testing::{TestClient, TestServer};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use sqlx::PgPool;
use std::{fs, os::unix::fs::PermissionsExt, path::Path, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn run(
    s: &TestServer,
    out: &Path,
    addr: std::net::SocketAddr,
    password: Option<&str>,
) -> std::process::Output {
    let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_porch"));
    c.args([
        "dev-init",
        "--json",
        "--devices",
        "fixture:desktop",
        "--out",
    ])
    .arg(out)
    .args(["--enroll-addr", &addr.to_string()])
    .env("PORCHLIGHT_SOCKET", &s.agent_socket)
    .env("PORCHLIGHT_TOKEN", s.bootstrap_token())
    .env("PORCHLIGHT_PASSWORD", password.unwrap_or(&s.admin_password));
    tokio::task::spawn_blocking(move || c.output().unwrap())
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn security_events_json_golden_and_human_output(pool: PgPool) {
    use porchlight_core::auth::session::Platform;
    use serde_json::{Value, json};
    let s = TestServer::start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let request = d.request(
        "POST",
        "/v1/auth/login",
        json!({"password":"incorrect fixture"}),
        None,
    );
    assert_eq!(
        TestClient::new(&s.ca_pem, Some(&d))
            .request(s.lan, request)
            .await
            .unwrap()
            .status,
        401
    );
    for as_json in [false, true] {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_porch"));
        c.args([
            "security-events",
            "--kind",
            "login_failed",
            "--since",
            "2020-01-01T00:00:00Z",
        ])
        .env("PORCHLIGHT_SOCKET", &s.agent_socket)
        .env("PORCHLIGHT_TOKEN", s.bootstrap_token());
        if as_json {
            c.arg("--json");
        }
        let out = tokio::task::spawn_blocking(move || c.output().unwrap())
            .await
            .unwrap();
        assert!(out.status.success());
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(!text.contains(s.bootstrap_token()));
        if !as_json {
            assert!(text.contains("OCCURRED AT\tKIND\tOUTCOME") && text.contains("login_failed"));
            continue;
        }
        let mut value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["items"].as_array().unwrap().len(), 1);
        let event = &mut value["items"][0];
        assert_eq!(event["actor"]["device_id"], d.device_id.to_string());
        assert_eq!(event["target_id"], s.init.admin_member_id.to_string());
        for (field, replacement) in [
            ("id", "<event>"),
            ("household_id", "<household>"),
            ("target_id", "<member>"),
            ("occurred_at", "<time>"),
        ] {
            assert!(event[field].is_string());
            event[field] = json!(replacement);
        }
        for (field, replacement) in [
            ("member_id", "<member>"),
            ("device_id", "<device>"),
            ("request_id", "<request>"),
            ("db_user", "<db-role>"),
        ] {
            assert!(event["actor"][field].is_string());
            event["actor"][field] = json!(replacement);
        }
        let actual = serde_json::to_string_pretty(&value).unwrap() + "\n";
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/security-events.json");
        if std::env::var("PORCH_UPDATE_GOLDENS").as_deref() == Ok("1") {
            fs::write(&path, &actual).unwrap();
        }
        assert_eq!(
            actual,
            fs::read_to_string(path).expect("security-events golden; update explicitly")
        );
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn dev_init_replays_committed_enrollment_after_reply_loss(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let chain = format!("{}{}", s.state.pki.server_cert_pem, s.ca_pem);
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        CertificateDer::pem_slice_iter(chain.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .unwrap(),
        PrivateKeyDer::from_pem_slice(s.state.pki.server_key_pem.as_bytes()).unwrap(),
    )
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    for after_headers in [false, true] {
        let dir =
            tempfile::tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var")).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ca = s.ca_pem.clone();
        let remote = s.enroll;
        let acceptor = acceptor.clone();
        let proxy = tokio::spawn(async move {
            let mut original = None;
            for attempt in 0..3 {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut tls = acceptor.accept(tcp).await.unwrap();
                let mut bytes = Vec::new();
                let end = loop {
                    bytes.push(tls.read_u8().await.unwrap());
                    if bytes.ends_with(b"\r\n\r\n") {
                        break bytes.len();
                    }
                };
                let headers = String::from_utf8(bytes.clone()).unwrap();
                let mut builder = Request::builder()
                    .method("POST")
                    .uri("/v1/enrollment/complete");
                let mut len = 0;
                for line in headers
                    .split("\r\n")
                    .skip(1)
                    .filter(|line| !line.is_empty())
                {
                    let (name, value) = line.split_once(':').unwrap();
                    if name.eq_ignore_ascii_case("content-length") {
                        len = value.trim().parse().unwrap();
                    }
                    builder = builder.header(name, value.trim());
                }
                bytes.resize(end + len, 0);
                tls.read_exact(&mut bytes[end..]).await.unwrap();
                if attempt == 2 {
                    let first = original.as_ref().unwrap();
                    assert!(
                        *first == bytes,
                        "retry must reuse the exact request identity"
                    );
                } else if attempt == 0 {
                    original = Some(bytes.clone());
                }
                let request = builder
                    .body(Full::new(Bytes::copy_from_slice(&bytes[end..])))
                    .unwrap();
                let response = TestClient::new(&ca, None)
                    .request(remote, request)
                    .await
                    .unwrap();
                assert_eq!(response.status, if attempt == 1 { 422 } else { 201 });
                if attempt == 0 {
                    if after_headers {
                        tls.write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 100000\r\nContent-Type: application/json\r\n\r\n{").await.unwrap();
                    }
                } else {
                    let header = format!(
                        "HTTP/1.1 {} Reply\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
                        response.status,
                        response.body.len()
                    );
                    tls.write_all(header.as_bytes()).await.unwrap();
                    tls.write_all(&response.body).await.unwrap();
                }
                let _ = tls.shutdown().await;
            }
        });
        let failed = run(&s, dir.path(), addr, None).await;
        assert!(!failed.status.success());
        let pending = dir.path().join("fixture/pending.json");
        assert!(
            pending.exists(),
            "ambiguous enrollment must retain its pending attempt"
        );
        assert_eq!(
            fs::metadata(&pending).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(pending.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let original = fs::read(&pending).unwrap();
        assert!(
            !run(&s, dir.path(), addr, Some("different fixture password"))
                .await
                .status
                .success()
        );
        assert!(
            fs::read(&pending).unwrap() == original,
            "mismatched retries must preserve the original attempt"
        );
        fs::set_permissions(&pending, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!run(&s, dir.path(), addr, None).await.status.success());
        fs::set_permissions(&pending, fs::Permissions::from_mode(0o600)).unwrap();
        // Simulate a crash during credential staging; replay safely republishes it.
        fs::write(
            dir.path().join("fixture/client.key.new"),
            b"partial fixture",
        )
        .unwrap();
        assert!(
            run(&s, dir.path(), addr, None).await.status.success(),
            "retry must recover credentials"
        );
        assert!(!pending.exists());
        assert!(dir.path().join("fixture/client.key").exists());
        proxy.await.unwrap();
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.devices")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2, "reply loss must not orphan extra devices");
    s.stop().await;
}
