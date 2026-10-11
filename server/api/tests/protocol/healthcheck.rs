//! Operational readiness cannot be inferred from a healthy database alone.
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
