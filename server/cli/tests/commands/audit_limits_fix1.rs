use std::{path::Path, process::Command, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn delayed_export(extra: &[&str], delay: Duration) -> std::process::Output {
    let dir = tempfile::tempdir_in(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../var")
            .canonicalize()
            .unwrap(),
    )
    .unwrap();
    let socket = dir.path().join("agent.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let server = tokio::spawn(async move {
        let (mut io, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = io.read(&mut buffer).await.unwrap();
            assert!(n > 0);
            request.extend_from_slice(&buffer[..n]);
        }
        assert!(request.starts_with(b"GET /v1/export/member?member_id="));
        tokio::time::sleep(delay).await;
        let _ = io.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await;
    });
    let extra: Vec<_> = extra.iter().map(|s| s.to_string()).collect();
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_porch"))
            .args(["export", "--member", "00000000-0000-0000-0000-000000000001"])
            .args(extra)
            .env("PORCHLIGHT_SOCKET", socket)
            .env("PORCHLIGHT_TOKEN", "controlled-test-token")
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    server.abort();
    output
}

#[tokio::test]
async fn member_export_waits_beyond_ordinary_header_deadline() {
    let output = delayed_export(&[], Duration::from_secs(16)).await;
    assert_eq!(
        output.status.code(),
        Some(0),
        "export preparation must outlive the ordinary 15-second header deadline"
    );
    assert_eq!(output.stdout, b"{}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("Preparing export"));
}

#[tokio::test]
async fn export_preparation_deadline_is_configurable() {
    let output = delayed_export(&["--prepare-timeout-seconds", "1"], Duration::from_secs(2)).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Export request timed out"));
    assert!(output.stdout.is_empty());
}
