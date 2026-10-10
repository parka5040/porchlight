//! Operational readiness cannot be inferred from a healthy database alone.
async fn tls_check(port: u16, dir: &std::path::Path) -> bool {
    let dir = dir.to_owned();
    tokio::task::spawn_blocking(move || {
        std::process::Command::new("python3")
            .arg("-I")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/lib/tls-readiness.py"))
            .arg("--port")
            .arg(port.to_string())
            .args(["--version", env!("CARGO_PKG_VERSION"), "--ca"])
            .arg(dir.join("ca.crt"))
            .arg("--server-cert")
            .arg(dir.join("server.crt"))
            .output()
            .unwrap()
            .status
            .success()
    })
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn independent_drill_checks_tls_and_real_application_response(pool: PgPool) {
    let server = TestServer::start(&pool).await;
    let dir = tempfile::Builder::new()
        .prefix("tls")
        .tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var"))
        .unwrap();
    std::fs::write(dir.path().join("ca.crt"), &server.ca_pem).unwrap();
    std::fs::write(
        dir.path().join("server.crt"),
        &server.state.pki.server_cert_pem,
    )
    .unwrap();
    assert!(
        tls_check(server.enroll.port(), dir.path()).await,
        "drill TLS probe must accept the real application"
    );
    // CA is valid but the pinned leaf is different.
    std::fs::write(dir.path().join("server.crt"), &server.ca_pem).unwrap();
    assert!(!tls_check(server.enroll.port(), dir.path()).await);
    std::fs::write(
        dir.path().join("server.crt"),
        &server.state.pki.server_cert_pem,
    )
    .unwrap();
    server.stop().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let stalled = tokio::spawn(async move {
        let (_connection, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let result = tokio::time::timeout(Duration::from_secs(15), tls_check(port, dir.path())).await;
    stalled.abort();
    assert!(!result.expect("TLS probe must time out itself"));
}

use porchlight_api::testing::TestServer;
use sqlx::{ConnectOptions, PgPool};
use std::{path::Path, time::Duration};

async fn check(pool: &PgPool, socket: &Path) -> bool {
    let url = pool.connect_options().to_url_lossy();
    let socket = socket.to_owned();
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_porchlight"))
            .arg("healthcheck")
            .env("PORCHLIGHT_DATABASE_URL", url.as_str())
            .env("PORCHLIGHT_AGENT_SOCKET", socket)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    // Never print URL or external process diagnostics (credentials stay in env).
    output.status.success()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn healthcheck_rejects_stalled_serving_process_with_healthy_db(pool: PgPool) {
    let server = TestServer::start(&pool).await;
    let socket = server.agent_socket.clone();
    // Prove the same credentials/schema are healthy with the real serving process.
    assert!(check(&pool, &socket).await);
    server.stop().await;
    // Replace the API with a listener accepting a connection but never answering it.
    // This models SIGSTOP after accept, without stopping the test or Postgres process.
    let dir = tempfile::Builder::new()
        .prefix("hc")
        .tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var"))
        .unwrap();
    let stalled_socket = dir.path().join("s.sock");
    let listener = tokio::net::UnixListener::bind(&stalled_socket).unwrap();
    let stalled = tokio::spawn(async move {
        let (_connection, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let started = std::time::Instant::now();
    let healthy = tokio::time::timeout(Duration::from_secs(15), check(&pool, &stalled_socket))
        .await
        .expect("healthcheck must have its own bounded deadline");
    stalled.abort();
    assert!(
        !healthy,
        "DB-only probe accepted a nonresponsive serving process"
    );
    assert!(started.elapsed() < Duration::from_secs(15));
    assert!(!check(&pool, &dir.path().join("missing.sock")).await);
}
