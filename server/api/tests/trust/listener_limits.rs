use axum::http::Request;
use http_body_util::Full;
use hyper::body::Bytes;
use porchlight_api::testing::{
    TestClient, TestDevice, TestOptions, TestServer, agent_request, proxy_v2_header,
};
use porchlight_core::auth::session::Platform;
use sqlx::PgPool;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
fn request() -> Request<Full<Bytes>> {
    Request::builder()
        .header("porchlight-client-version", porchlight_core::VERSION)
        .uri("/v1/connection")
        .header("host", "localhost")
        .body(Full::new(Bytes::new()))
        .unwrap()
}
async fn device(server: &TestServer) -> TestDevice {
    server
        .session_device(server.init.admin_member_id, Platform::Desktop)
        .await
}
fn authenticated_request(d: &TestDevice) -> Request<Full<Bytes>> {
    d.request("GET", "/v1/connection", serde_json::json!({}), None)
}
async fn prompt_eof(addr: std::net::SocketAddr) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut byte = [0];
    assert_eq!(
        tokio::time::timeout(Duration::from_millis(500), stream.read(&mut byte))
            .await
            .expect("connection must close promptly")
            .unwrap(),
        0
    );
}
#[sqlx::test(migrations = "../db/migrations")]
async fn connection_cap_releases_permits(pool: PgPool) {
    let server = TestServer::start_with(
        &pool,
        TestOptions {
            max_connections: 2,
            tls_timeout: Duration::from_secs(10),
            proxy_timeout: Duration::from_secs(10),
            ..Default::default()
        },
    )
    .await;
    let d = device(&server).await;
    let client = TestClient::new(&server.ca_pem, Some(&d));
    // Each listener has its own budget; idle pre-TLS sockets consume it too.
    for addr in [server.lan, server.funnel, server.enroll] {
        let first = TcpStream::connect(addr).await.unwrap();
        let second = TcpStream::connect(addr).await.unwrap();
        prompt_eof(addr).await;
        // Other listeners still serve while this one's budget is exhausted.
        let other = if addr == server.lan {
            server.enroll
        } else {
            server.lan
        };
        let other_client = TestClient::new(
            &server.ca_pem,
            if other == server.lan { Some(&d) } else { None },
        );
        assert!(
            other_client
                .request(other, authenticated_request(&d))
                .await
                .is_ok()
        );
        drop(first);
        let prefix = if addr == server.funnel {
            proxy_v2_header("203.0.113.7:4444".parse().unwrap(), addr)
        } else {
            Vec::new()
        };
        let target_client = if addr == server.enroll {
            TestClient::new(&server.ca_pem, None)
        } else {
            TestClient::new(&server.ca_pem, Some(&d))
        };
        // Wait for the EOF task to release its permit, rather than assuming scheduler order.
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(response) = target_client
                    .request_with_prefix(addr, &prefix, authenticated_request(&d))
                    .await
                {
                    assert_eq!(
                        response.status,
                        if addr == server.enroll { 404 } else { 200 }
                    );
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        drop(second);
    }
    assert_eq!(
        client
            .request(server.lan, authenticated_request(&d))
            .await
            .unwrap()
            .status,
        200
    );
    server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn enrollment_header_deadline(pool: PgPool) {
    let header_timeout = Duration::from_secs(1);
    let server = TestServer::start_with(
        &pool,
        TestOptions {
            header_timeout,
            ..Default::default()
        },
    )
    .await;
    let client = TestClient::new(&server.ca_pem, None);
    for partial in [b"".as_slice(), b"GET /v1/connection HTTP/1.1\r\nHost:"] {
        let mut tls = client.connect(server.enroll).await.unwrap();
        tls.write_all(partial).await.unwrap();
        let mut bytes = Vec::new();
        let result = tokio::time::timeout(
            header_timeout + Duration::from_secs(1),
            tls.read_to_end(&mut bytes),
        )
        .await
        .expect("silent/partial HTTP head must time out");
        // Bounded cancellation may close TCP without TLS close_notify.
        assert!(
            result.is_ok() || result.is_err_and(|e| e.kind() == std::io::ErrorKind::UnexpectedEof)
        );
        // Hyper may send a 408, but it must then close the socket.
        assert!(bytes.is_empty() || bytes.starts_with(b"HTTP/1.1 408"));
    }
    server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn enrollment_keep_alive_header_deadline(pool: PgPool) {
    use http_body_util::BodyExt;
    let server = TestServer::start_with(
        &pool,
        TestOptions {
            header_timeout: Duration::from_secs(1),
            ..Default::default()
        },
    )
    .await;
    let client = TestClient::new(&server.ca_pem, None);
    let tls = client.connect(server.enroll).await.unwrap();
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(tls))
            .await
            .unwrap();
    let task = tokio::spawn(connection);
    let response = sender.send_request(request()).await.unwrap();
    assert_eq!(response.status(), 404);
    response.into_body().collect().await.unwrap();
    // Hyper resets the header deadline when it begins waiting for the next request head.
    let result = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("idle keep-alive must close")
        .unwrap();
    assert!(
        result.is_ok()
            || result.is_err_and(|e| {
                e.is_incomplete_message()
                    || e.is_closed()
                    || std::error::Error::source(&e).is_some_and(|cause| {
                        cause
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|io| io.kind() == std::io::ErrorKind::UnexpectedEof)
                    })
            })
    );
    drop(sender);
    server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn serve_bind_errors_name_listener_and_endpoint(pool: PgPool) {
    use sqlx::ConnectOptions;
    let server = TestServer::start(&pool).await;
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = occupied.local_addr().unwrap().to_string();
    let bad_socket = server.agent_socket.with_file_name("regular-file");
    std::fs::write(&bad_socket, b"keep me").unwrap();
    let options = porchlight_db::testing::connect_options_as(
        &pool,
        porchlight_db::testing::Role::App,
        Some("bind-test"),
    )
    .unwrap();
    for (flag, endpoint, context) in [
        ("--lan-addr", addr.clone(), "binding LAN listener at"),
        ("--funnel-addr", addr.clone(), "binding Funnel listener at"),
        (
            "--enroll-addr",
            addr.clone(),
            "binding enrollment listener at",
        ),
        (
            "--agent-socket",
            bad_socket.display().to_string(),
            "binding agent socket at",
        ),
    ] {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_porchlight"));
        command.args([
            "serve",
            "--enroll-host",
            "localhost",
            "--enroll-cidrs",
            "127.0.0.0/8",
        ]);
        for default_flag in ["--lan-addr", "--enroll-addr"] {
            if flag != default_flag {
                command.args([default_flag, "127.0.0.1:0"]);
            }
        }
        if flag == "--funnel-addr" {
            command.args(["--funnel-peers", "127.0.0.1/32"]);
        }
        command
            .arg("--data-dir")
            .arg(server.agent_socket.parent().unwrap());
        command.args([flag, &endpoint]);
        // Avoid inherited deployment settings and never put the database credential in argv.
        command
            .env("PORCHLIGHT_DATABASE_URL", options.to_url_lossy().as_str())
            .env_remove("PORCHLIGHT_MIGRATOR_URL")
            .env_remove("PORCHLIGHT_FUNNEL_ADDR")
            .env_remove("PORCHLIGHT_FUNNEL_PEERS");
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        let text = String::from_utf8_lossy(&output.stderr);
        assert!(text.contains(context) && text.contains(&endpoint), "{text}");
    }
    assert_eq!(std::fs::read(bad_socket).unwrap(), b"keep me");
    server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn agent_router_is_separate(pool: PgPool) {
    let server = TestServer::start(&pool).await;
    assert_eq!(
        agent_request(
            &server.agent_socket,
            Some(&server.init.bootstrap_token),
            request()
        )
        .await
        .status,
        404
    );
    server.stop().await;
}
