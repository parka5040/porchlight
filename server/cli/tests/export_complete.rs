use std::{path::Path, process::Command};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn archive(complete: bool) -> Vec<u8> {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (name, bytes) in [
        ("data.json", b"{\"format_version\":2}".as_slice()),
        (
            "complete.json",
            b"{\"format_version\":2,\"complete\":true}".as_slice(),
        ),
    ] {
        if name == "complete.json" && !complete {
            continue;
        }
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        tar.append_data(&mut header, name, bytes).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

async fn download(bytes: Vec<u8>, to_file: bool) -> (std::process::Output, bool) {
    let dir =
        tempfile::tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var")).unwrap();
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
    }
}

#[tokio::test]
async fn cli_accepts_complete_archive() {
    let (out, exists) = download(archive(true), true).await;
    assert_eq!(out.status.code(), Some(0));
    assert!(exists);
}
