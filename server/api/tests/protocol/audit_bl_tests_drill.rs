//! Independent deployed TLS readiness probe.
use porchlight_api::testing::TestServer;
use sqlx::PgPool;
use std::{path::Path, time::Duration};

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
