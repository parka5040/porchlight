use axum::http::Request;
use http_body_util::Full;
use hyper::body::Bytes;
use porchlight_api::{
    server::bind_agent_socket,
    testing::{TestClient, TestDevice, TestOptions, TestServer, proxy_v2_header},
    tls::mtls_server_config,
};
use porchlight_core::auth::session::Platform;
use porchlight_pki::{HouseholdCa, fingerprint, generate_csr};
use sqlx::PgPool;
use std::{
    os::unix::fs::PermissionsExt,
    time::{Duration, SystemTime},
};
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
#[sqlx::test(migrations = "../db/migrations")]
async fn lan_mtls_ok(pool: PgPool) {
    let server = TestServer::start(&pool).await;
    let d = device(&server).await;
    let client = TestClient::new(&server.ca_pem, Some(&d));
    let r = client
        .request(server.lan, authenticated_request(&d))
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["transport"], "lan");
    assert!(
        r.json()["client_addr"]
            .as_str()
            .unwrap()
            .starts_with("127.0.0.1:")
    );
    server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn lan_rejects_unenrolled_tls(pool: PgPool) {
    let server = TestServer::start(&pool).await;
    assert!(
        TestClient::new(&server.ca_pem, None)
            .request(server.lan, request())
            .await
            .is_err()
    );
    let foreign = HouseholdCa::generate("foreign", SystemTime::now()).unwrap();
    let (csr, key) = generate_csr("foreign device").unwrap();
    let id = uuid::Uuid::now_v7();
    let cert = foreign
        .issue_device(&csr, &id.to_string(), SystemTime::now(), 365)
        .unwrap();
    let d = TestDevice {
        device_id: id,
        cert_pem: cert.cert_pem,
        key_pem: key.to_string(),
        fingerprint: cert.fingerprint,
        session: None,
    };
    assert!(
        TestClient::new(&server.ca_pem, Some(&d))
            .request(server.lan, request())
            .await
            .is_err()
    );
    let pki = &server.state.pki;
    let d = TestDevice {
        device_id: id,
        cert_pem: pki.server_cert_pem.clone(),
        key_pem: pki.server_key_pem.to_string(),
        fingerprint: [0; 32],
        session: None,
    };
    assert!(
        TestClient::new(&server.ca_pem, Some(&d))
            .request(server.lan, request())
            .await
            .is_err()
    );
    server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn funnel_requires_proxy_header(pool: PgPool) {
    let server = TestServer::start(&pool).await;
    let d = device(&server).await;
    let client = TestClient::new(&server.ca_pem, Some(&d));
    assert!(
        client
            .request(server.funnel, authenticated_request(&d))
            .await
            .is_err()
    );
    let prefix = proxy_v2_header("203.0.113.7:4444".parse().unwrap(), server.funnel);
    let r = client
        .request_with_prefix(server.funnel, &prefix, authenticated_request(&d))
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(
        r.json(),
        serde_json::json!({"transport":"funnel","client_addr":"203.0.113.7:4444"})
    );
    let mapped_prefix = proxy_v2_header(
        "[::ffff:203.0.113.7]:4444".parse().unwrap(),
        std::net::SocketAddr::new(
            std::net::Ipv4Addr::LOCALHOST.to_ipv6_mapped().into(),
            server.funnel.port(),
        ),
    );
    let mapped = client
        .request_with_prefix(server.funnel, &mapped_prefix, authenticated_request(&d))
        .await
        .unwrap();
    assert_eq!(mapped.json()["client_addr"], "203.0.113.7:4444");
    let mut stream = TcpStream::connect(server.funnel).await.unwrap();
    for chunk in prefix.chunks(10) {
        stream.write_all(chunk).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let tls = client.connect_stream(stream).await.unwrap();
    assert_eq!(
        TestClient::send(tls, authenticated_request(&d))
            .await
            .unwrap()
            .status,
        200
    );
    let mut stream = TcpStream::connect(server.funnel).await.unwrap();
    let mut oversized = prefix.clone();
    oversized[14..16].copy_from_slice(&513u16.to_be_bytes());
    stream.write_all(&oversized).await.unwrap();
    let mut b = [0; 512];
    // Distinguish immediate length rejection from the one-second PROXY deadline.
    assert!(matches!(
        tokio::time::timeout(Duration::from_millis(500), stream.read(&mut b))
            .await
            .unwrap(),
        Ok(0) | Err(_)
    ));
    let mut stream = TcpStream::connect(server.funnel).await.unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), stream.read(&mut b))
            .await
            .unwrap(),
        Ok(0) | Err(_)
    ));
    server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn funnel_rejects_wrong_peer(pool: PgPool) {
    let server = TestServer::start_with(
        &pool,
        TestOptions {
            funnel_peers: vec!["10.9.9.9/32".parse().unwrap()],
            ..Default::default()
        },
    )
    .await;
    prompt_eof(server.funnel).await;
    server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn enrollment_listener(pool: PgPool) {
    let server = TestServer::start(&pool).await;
    let client = TestClient::new(&server.ca_pem, None);
    let tls = client.connect(server.enroll).await.unwrap();
    let certs = tls.get_ref().1.peer_certificates().unwrap();
    assert_eq!(certs.len(), 2);
    assert_eq!(
        fingerprint(certs.last().unwrap().as_ref()),
        server.ca_fingerprint
    );
    assert_eq!(TestClient::send(tls, request()).await.unwrap().status, 404);
    server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn enrollment_rejects_wrong_peer(pool: PgPool) {
    let server = TestServer::start_with(
        &pool,
        TestOptions {
            enroll_cidrs: vec!["10.0.0.0/8".parse().unwrap()],
            ..Default::default()
        },
    )
    .await;
    prompt_eof(server.enroll).await;
    server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn resumption_disabled(pool: PgPool) {
    let server = TestServer::start(&pool).await;
    let d = device(&server).await;
    let client = TestClient::new(&server.ca_pem, Some(&d));
    for _ in 0..2 {
        let tls = client.connect(server.lan).await.unwrap();
        assert_eq!(
            tls.get_ref().1.handshake_kind(),
            Some(rustls::HandshakeKind::Full)
        );
        assert_eq!(
            TestClient::send(tls, authenticated_request(&d))
                .await
                .unwrap()
                .status,
            200
        );
    }
    assert_eq!(
        mtls_server_config(&server.state.pki, server.state.revoked.clone())
            .unwrap()
            .send_tls13_tickets,
        0
    );
    server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn revoked_cert_fails_handshake_fresh_and_would_be_resumed(pool: PgPool) {
    let server = TestServer::start(&pool).await;
    let d = device(&server).await;
    let client = TestClient::new(&server.ca_pem, Some(&d));
    assert_eq!(
        client
            .request(server.lan, authenticated_request(&d))
            .await
            .unwrap()
            .status,
        200
    );
    server.state.revoked.insert(d.fingerprint);
    assert!(
        TestClient::new(&server.ca_pem, Some(&d))
            .request(server.lan, authenticated_request(&d))
            .await
            .is_err()
    );
    assert!(
        client
            .request(server.lan, authenticated_request(&d))
            .await
            .is_err()
    );
    server.stop().await;
}
#[tokio::test]
async fn agent_socket_bind() {
    // Remove /server/api/../.. before binding: Unix socket limits count path bytes,
    // including redundant components (nested worktree copies otherwise exceed SUN_LEN).
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../var")
        .canonicalize()
        .unwrap();
    let tmp = tempfile::tempdir_in(root).unwrap();
    let path = tmp.path().join("agent.sock");
    let first = bind_agent_socket(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o660
    );
    let second = bind_agent_socket(&path).unwrap();
    drop((first, second));
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, b"keep me").unwrap();
    assert!(bind_agent_socket(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"keep me");
}
#[sqlx::test(migrations = "../db/migrations")]
async fn lan_keeps_connection_alive(pool: PgPool) {
    use http_body_util::BodyExt;
    let server = TestServer::start(&pool).await;
    let d = device(&server).await;
    let client = TestClient::new(&server.ca_pem, Some(&d));
    let tls = client.connect(server.lan).await.unwrap();
    assert_eq!(
        tls.get_ref().1.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_3)
    );
    assert_eq!(
        tls.get_ref().1.alpn_protocol(),
        Some(b"http/1.1".as_slice())
    );
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(tls))
            .await
            .unwrap();
    let task = tokio::spawn(connection);
    let mut previous = None;
    for _ in 0..2 {
        let response = sender
            .send_request(authenticated_request(&d))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        if let Some(previous) = previous {
            assert_eq!(previous, body);
        }
        previous = Some(body);
    }
    drop(sender);
    task.abort();
    let _ = task.await;
    server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn tls_handshake_deadline(pool: PgPool) {
    let server = TestServer::start_with(
        &pool,
        TestOptions {
            tls_timeout: Duration::from_millis(50),
            ..Default::default()
        },
    )
    .await;
    for addr in [server.lan, server.enroll] {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let mut byte = [0];
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), stream.read(&mut byte))
                .await
                .unwrap(),
            Ok(0) | Err(_)
        ));
    }
    server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn revoked_fingerprints_includes_all_device_leaves(pool: PgPool) {
    let server = TestServer::start(&pool).await;
    let d = device(&server).await;
    let live = device(&server).await;
    sqlx::query("INSERT INTO porchlight.device_certificates (household_id,device_id,fingerprint_sha256,serial,not_before,not_after) SELECT household_id,device_id,$2,'renewal',not_before,not_after FROM porchlight.device_certificates WHERE device_id=$1")
        .bind(d.device_id).bind([99u8;32].as_slice()).execute(&pool).await.unwrap();
    sqlx::query("UPDATE porchlight.devices SET revoked_at=now() WHERE id=$1")
        .bind(d.device_id)
        .execute(&pool)
        .await
        .unwrap();
    let fps = porchlight_db::devices::revoked_fingerprints(&server.state.pool)
        .await
        .unwrap();
    assert_eq!(fps.len(), 2);
    assert!(fps.contains(&d.fingerprint));
    assert!(fps.contains(&[99; 32]));
    assert!(!fps.contains(&live.fingerprint));
    server.stop().await;
}
// A rejected TCP peer has sent no bytes, so EOF (not a TLS alert/ServerHello) is required.
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
