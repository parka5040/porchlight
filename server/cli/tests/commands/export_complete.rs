use std::{path::Path, process::Command};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[path = "../support/export_archive.rs"]
mod archive_fixture;
use archive_fixture::{archive, unfinished_deflate};

async fn download(bytes: Vec<u8>, to_file: bool) -> (std::process::Output, bool) {
    let dir = tempfile::tempdir_in(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../var")
            .canonicalize()
            .unwrap(),
    )
    .unwrap();
    let socket = dir.path().join("mock.sock");
    let path = dir.path().join("export.tar.gz");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let server = tokio::spawn(async move {
        let (mut io, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = io.read(&mut buffer).await.unwrap();
            assert!(n > 0);
            request.extend_from_slice(&buffer[..n]);
        }
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            bytes.len()
        );
        io.write_all(header.as_bytes()).await.unwrap();
        io.write_all(&bytes).await.unwrap();
    });
    let target = path.clone();
    let output = tokio::task::spawn_blocking(move || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_porch"));
        command
            .args(["export", "--household"])
            .env("PORCHLIGHT_SOCKET", socket)
            .env("PORCHLIGHT_TOKEN", "controlled-test-token");
        if to_file {
            command.arg("--out").arg(target);
        }
        command.output().unwrap()
    })
    .await
    .unwrap();
    server.await.unwrap();
    (output, path.exists())
}

#[tokio::test]
async fn cli_rejects_complete_http_response_without_completion_or_gzip_trailer() {
    for to_file in [false, true] {
        let (out, exists) = download(archive(false), to_file).await;
        assert_eq!(
            out.status.code(),
            Some(1),
            "CLI accepted archive without completion"
        );
        assert!(!exists, "incomplete output file retained");
        let mut bytes = archive(true);
        bytes.truncate(bytes.len() - 8);
        let (out, exists) = download(bytes, to_file).await;
        assert_eq!(
            out.status.code(),
            Some(1),
            "CLI accepted missing gzip trailer"
        );
        assert!(!exists);
        let (out, exists) = download(unfinished_deflate(), to_file).await;
        assert_eq!(
            out.status.code(),
            Some(1),
            "CLI accepted unfinished deflate stream"
        );
        assert!(!exists, "unfinished output file retained");
        // Stdout is intentionally streaming; consumers must honor nonzero exit.
    }
}

#[tokio::test]
async fn cli_accepts_complete_archive() {
    let (out, exists) = download(archive(true), true).await;
    assert_eq!(out.status.code(), Some(0));
    assert!(exists);
}
