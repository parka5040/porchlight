use crate::common;
use axum::http::Request;
use http_body_util::Full;
use hyper::body::Bytes;
use porchlight_api::testing::{
    TestClient, TestOptions, TestServer, agent_request, proxy_v2_header,
};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::{ConnectOptions, PgPool};
use std::time::Duration;

#[sqlx::test(migrations = "../db/migrations")]
async fn maintenance_fences_every_transport_but_not_reads_or_health(pool: PgPool) {
    let opts = TestOptions::default();
    let fence = opts.settings.data_dir.join("maintenance");
    std::fs::write(&fence, b"").unwrap();
    let server = TestServer::start_with(&pool, opts).await;
    let device = server
        .issue_device(server.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&server.ca_pem, Some(&device));
    let enrollment = TestClient::new(&server.ca_pem, None);
    for method in ["POST", "PUT", "PATCH", "DELETE"] {
        let request = || {
            device.request(
                method,
                "/v1/auth/login",
                json!({"password":server.admin_password}),
                None,
            )
        };
        let lan = client.request(server.lan, request()).await.unwrap();
        common::problem(&lan, 503, "maintenance");
        assert_eq!(lan.headers["retry-after"], "1");
        let prefix = proxy_v2_header("192.168.77.2:1234".parse().unwrap(), server.funnel);
        let funnel = client
            .request_with_prefix(server.funnel, &prefix, request())
            .await
            .unwrap();
        common::problem(&funnel, 503, "maintenance");
        let enroll = enrollment.request(server.enroll, request()).await.unwrap();
        common::problem(&enroll, 503, "maintenance");
        let agent = agent_request(
            &server.agent_socket,
            Some(&server.init.bootstrap_token),
            request(),
        )
        .await;
        common::problem(&agent, 503, "maintenance");
    }
    let read = enrollment
        .request(
            server.enroll,
            Request::builder()
                .uri("/internal/readiness")
                .header("Porchlight-Client-Version", "0.1.0")
                .body(Full::new(Bytes::new()))
                .unwrap(),
        )
        .await
        .unwrap();
    common::problem(&read, 404, "not-found");
    let url = pool.connect_options().to_url_lossy();
    let socket = server.agent_socket.clone();
    let healthy = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_porchlight"))
            .arg("healthcheck")
            .env("PORCHLIGHT_DATABASE_URL", url.as_str())
            .env("PORCHLIGHT_AGENT_SOCKET", socket)
            .output()
            .unwrap()
            .status
            .success()
    })
    .await
    .unwrap();
    assert!(healthy, "maintenance must not mask readiness");
    std::fs::remove_file(&fence).unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let mut device = device;
    common::login(&server, &mut device).await;
    // A running process also notices an operator fence within one second.
    std::fs::write(&fence, b"").unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let response = common::post(
        &server,
        &device,
        "/v1/auth/login",
        json!({"password":server.admin_password}),
    )
    .await;
    common::problem(&response, 503, "maintenance");
    let read = common::get(&server, &device, "/v1/me").await;
    assert_eq!(
        read.status, 200,
        "authenticated reads must remain available"
    );
    server.stop().await;
}
