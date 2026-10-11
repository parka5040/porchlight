//! HTTP/1 client for the PAT-authenticated agent socket. No credentials appear in diagnostics.
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::{Request, body::Bytes};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use std::{fmt, path::PathBuf, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::UnixStream,
};
use uuid::Uuid;

pub const DEADLINE: Duration = Duration::from_secs(15);
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub type Result<T> = std::result::Result<T, Error>;

pub enum Error {
    Problem(Value),
    RetryableProblem(Value, Duration),
    Local(&'static str),
    // Only controlled messages: validated device labels or rustls certificate/handshake kinds.
    Message(String),
    Usage(&'static str),
}
impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CLI error (response redacted)")
    }
}
impl Error {
    fn problem(&self) -> Value {
        match self {
            Self::Problem(p) | Self::RetryableProblem(p, _) => p.clone(),
            Self::Local(message) | Self::Usage(message) => Self::local_problem(message),
            Self::Message(message) => Self::local_problem(message),
        }
    }
    fn local_problem(message: &str) -> Value {
        json!({"type":"urn:porchlight:problem:client-error", "title":"CLI request failed", "status":null, "detail":message})
    }
    pub fn print(&self, json_output: bool) {
        let problem = self.problem();
        if json_output {
            println!("{problem}");
        } else {
            eprintln!(
                "{}: {} [{}]",
                problem["title"].as_str().unwrap_or("HTTP request failed"),
                problem["detail"].as_str().unwrap_or("Request failed"),
                problem["type"].as_str().unwrap_or("client-error")
            );
        }
    }
}

pub struct Client {
    socket: PathBuf,
    token: String,
}
impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("token", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}
impl Client {
    pub fn new(socket: PathBuf, token: String) -> Self {
        Self { socket, token }
    }
    pub async fn request(&self, method: &str, path: &str, body: Value) -> Result<Value> {
        self.request_inner(method, path, body, None, None).await
    }
    pub async fn request_version(
        &self,
        method: &str,
        path: &str,
        body: Value,
        version: Option<i64>,
    ) -> Result<Value> {
        if version.is_some_and(|v| v < 0) {
            return Err(Error::Local("Version must be nonnegative"));
        }
        self.request_inner(method, path, body, version, None).await
    }
    /// Three attempts at most, honoring only short 409 Retry-After delays.
    /// The same idempotency key is retained across retries of this one operation.
    pub async fn revoke_token(&self, token_id: Uuid) -> Result<Value> {
        let path = format!("/v1/admin/tokens/{token_id}/revoke");
        retry_revoke(|key| {
            let path = &path;
            async move {
                self.request_inner("POST", path, json!({}), None, Some(&key))
                    .await
            }
        })
        .await
    }
    async fn request_inner(
        &self,
        method: &str,
        path: &str,
        body: Value,
        version: Option<i64>,
        key: Option<&str>,
    ) -> Result<Value> {
        let stream = tokio::time::timeout(DEADLINE, UnixStream::connect(&self.socket))
            .await
            .map_err(|_| Error::Local("Agent connection timed out"))?
            .map_err(|_| Error::Local("Could not connect to agent socket"))?;
        let mut req = request(method, path, body, Some(&self.token), "porchlight")?;
        if let Some(key) = key {
            req.headers_mut().insert(
                "idempotency-key",
                key.parse()
                    .map_err(|_| Error::Local("Invalid idempotency key"))?,
            );
        }
        if let Some(version) = version {
            req.headers_mut().insert(
                "if-match",
                version
                    .to_string()
                    .parse()
                    .map_err(|_| Error::Local("Invalid version"))?,
            );
        }
        send(stream, req).await
    }
    /// Stream an export without the normal 4MiB JSON-response cap. The caller
    /// chose the output; no token, key or credential table is in these responses.
    pub async fn download(
        &self,
        path: &str,
        out: Option<&std::path::Path>,
        prepare_timeout: Duration,
    ) -> Result<Value> {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let stream = tokio::time::timeout(DEADLINE, UnixStream::connect(&self.socket))
            .await
            .map_err(|_| Error::Local("Agent connection timed out"))?
            .map_err(|_| Error::Local("Could not connect to agent socket"))?;
        let req = request("GET", path, Value::Null, Some(&self.token), "porchlight")?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|_| Error::Local("HTTP handshake failed"))?;
        let task = tokio::spawn(connection);
        let result = async {
            let response = tokio::time::timeout(prepare_timeout, sender.send_request(req))
                .await
                .map_err(|_| Error::Local("Export request timed out"))?
                .map_err(|_| Error::Local("Export request failed"))?;
            if !response.status().is_success() {
                let body = Limited::new(response.into_body(), MAX_RESPONSE_BYTES)
                    .collect()
                    .await
                    .map_err(|_| Error::Local("Could not read export problem"))?
                    .to_bytes();
                return Err(Error::Problem(
                    serde_json::from_slice(&body)
                        .map_err(|_| Error::Local("Invalid export problem"))?,
                ));
            }
            struct Output {
                file: Option<std::fs::File>,
                path: Option<PathBuf>,
                complete: bool,
            }
            impl Drop for Output {
                fn drop(&mut self) {
                    if !self.complete
                        && let Some(path) = &self.path
                    {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }
            let mut output = Output {
                file: None,
                path: None,
                complete: false,
            };
            if let Some(path) = out {
                output.file = Some(
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(path)
                        .map_err(|_| {
                            Error::Local(
                                "Cannot create output; existing files are never overwritten",
                            )
                        })?,
                );
                output.path = Some(path.to_owned());
            }
            let verification = (path == "/v1/admin/export/household")
                .then(crate::export_verify::Verification::start);
            let mut body = response.into_body();
            let mut bytes = 0u64;
            while let Some(frame) = tokio::time::timeout(Duration::from_secs(300), body.frame())
                .await
                .map_err(|_| Error::Local("Export download stalled"))?
            {
                let frame = frame.map_err(|_| Error::Local("Export download interrupted"))?;
                if let Ok(data) = frame.into_data() {
                    if let Some(verify) = &verification {
                        verify.chunk(data.clone()).await?;
                    }
                    if let Some(file) = &mut output.file {
                        file.write_all(&data)
                            .map_err(|_| Error::Local("Could not write export"))?;
                    } else {
                        std::io::stdout()
                            .write_all(&data)
                            .map_err(|_| Error::Local("Could not write export stdout"))?;
                    }
                    bytes += data.len() as u64;
                }
            }
            if let Some(verify) = verification {
                verify.finish().await?;
            }
            if let Some(file) = &output.file {
                file.sync_all()
                    .map_err(|_| Error::Local("Could not sync export"))?;
            }
            output.complete = true;
            Ok(json!({"bytes":bytes,"out":out.map(|p|p.to_string_lossy()),"stdout":out.is_none()}))
        }
        .await;
        drop(sender);
        task.abort();
        let _ = task.await;
        result
    }
    /// Multipart bytes still use the same bounded socket client and response/problem handling.
    pub async fn upload(&self, path: &str, name: &str, bytes: &[u8]) -> Result<Value> {
        let name: String = name
            .chars()
            .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
            .take(180)
            .collect();
        let boundary = format!("porch-{}", Uuid::now_v7());
        let mut body=format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let mut req = request("POST", path, json!({}), Some(&self.token), "porchlight")?
            .map(|_| Full::new(Bytes::from(body)));
        req.headers_mut().insert(
            "content-type",
            format!("multipart/form-data; boundary={boundary}")
                .parse()
                .map_err(|_| Error::Local("Invalid upload headers"))?,
        );
        let stream = tokio::time::timeout(DEADLINE, UnixStream::connect(&self.socket))
            .await
            .map_err(|_| Error::Local("Agent connection timed out"))?
            .map_err(|_| Error::Local("Could not connect to agent socket"))?;
        send(stream, req).await
    }
    pub async fn calendar_member(&self, who: &str) -> Result<Uuid> {
        if let Ok(id) = who.parse() {
            return Ok(id);
        }
        let response = self
            .request(
                "GET",
                &format!("/v1/admin/feeds/target?username={}", segment(who)),
                Value::Null,
            )
            .await?;
        response["member_id"]
            .as_str()
            .and_then(|v| v.parse().ok())
            .ok_or(Error::Local("Invalid calendar target response"))
    }
    pub async fn enrollment_token(&self, username: &str, label: &str) -> Result<Value> {
        self.request(
            "POST",
            "/v1/admin/enrollment-tokens",
            json!({"username":username, "device_label":label}),
        )
        .await
    }
}

pub fn request(
    method: &str,
    path: &str,
    body: Value,
    token: Option<&str>,
    host: &str,
) -> Result<Request<Full<Bytes>>> {
    let mut builder = Request::builder()
        .header("porchlight-client-version", porchlight_core::VERSION)
        .method(method)
        .uri(path)
        .header("host", host);
    if method != "GET" {
        builder = builder.header("idempotency-key", Uuid::now_v7().to_string());
    }
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let bytes = if method == "GET" {
        Bytes::new()
    } else {
        builder = builder.header("content-type", "application/json");
        Bytes::from(
            serde_json::to_vec(&body).map_err(|_| Error::Local("Could not encode request"))?,
        )
    };
    builder
        .body(Full::new(bytes))
        .map_err(|_| Error::Local("Invalid request headers or path"))
}

pub async fn send<I>(stream: I, request: Request<Full<Bytes>>) -> Result<Value>
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|_| Error::Local("HTTP handshake failed"))?;
    let task = tokio::spawn(connection);
    let result = tokio::time::timeout(DEADLINE, async {
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| Error::Local("HTTP request failed"))?;
        let status = response.status();
        let retry = short_revoke_delay(
            status.as_u16(),
            response
                .headers()
                .get("retry-after")
                .and_then(|h| h.to_str().ok()),
        );
        let body = Limited::new(response.into_body(), MAX_RESPONSE_BYTES)
            .collect()
            .await
            .map_err(|error| {
                if error.is::<LengthLimitError>() {
                    Error::Local("HTTP response exceeds 4 MiB limit")
                } else {
                    Error::Local("Could not read HTTP response")
                }
            })?
            .to_bytes();
        let value: Value = if body.is_empty() && status.is_success() {
            json!({})
        } else {
            match serde_json::from_slice(&body) {
                Ok(value) => value,
                Err(_) if !status.is_success() => json!({
                    "type":"urn:porchlight:problem:http-error",
                    "title":format!("HTTP {status}"), "status":status.as_u16(),
                    "detail":"Server returned a non-JSON error response"
                }),
                Err(_) => return Err(Error::Local("Invalid JSON response")),
            }
        };
        if status.is_success() {
            Ok(value)
        } else {
            Err(match retry {
                Some(delay) => Error::RetryableProblem(value, delay),
                None => Error::Problem(value),
            })
        }
    })
    .await
    .map_err(|_| Error::Local("HTTP request timed out"));
    drop(sender);
    task.abort();
    let _ = task.await;
    result?
}

// HTTP status, not the untrusted problem body's status, controls retries.
fn short_revoke_delay(status: u16, header: Option<&str>) -> Option<Duration> {
    let seconds: u64 = header?.parse().ok()?;
    (status == 409 && seconds <= 2).then(|| Duration::from_secs(seconds))
}
async fn retry_revoke<F, Fut>(mut send: F) -> Result<Value>
where
    F: FnMut(String) -> Fut,
    Fut: std::future::Future<Output = Result<Value>>,
{
    let key = Uuid::now_v7().to_string();
    for attempt in 0..3 {
        match send(key.clone()).await {
            Err(Error::RetryableProblem(_, delay)) if attempt < 2 => {
                tokio::time::sleep(delay).await
            }
            result => return result,
        }
    }
    unreachable!("bounded revoke loop returns on final attempt")
}

pub fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value[name]
        .as_str()
        .ok_or(Error::Local("Missing field in server response"))
}

/// Percent-encode a single path segment (usernames are not URL syntax).
pub fn segment(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revoke_retry_requires_short_409_header() {
        assert_eq!(
            short_revoke_delay(409, Some("1")),
            Some(Duration::from_secs(1))
        );
        for (status, header) in [
            (409, None),
            (409, Some("invalid")),
            (409, Some("3")),
            (409, Some("-1")),
            (401, Some("1")),
            (503, Some("1")),
        ] {
            assert_eq!(short_revoke_delay(status, header), None);
        }
    }
    #[tokio::test]
    async fn revoke_retries_keep_key_and_stop_at_three() {
        let mut keys = Vec::new();
        let result = retry_revoke(|key| {
            keys.push(key);
            std::future::ready(if keys.len() < 3 {
                Err(Error::RetryableProblem(
                    json!({"status":409}),
                    Duration::ZERO,
                ))
            } else {
                Ok(json!({"revoked_at":"now"}))
            })
        })
        .await
        .unwrap();
        assert_eq!(result["revoked_at"], "now");
        assert_eq!(keys.len(), 3);
        assert!(keys.iter().all(|key| key == &keys[0]));
        keys.clear();
        let result = retry_revoke(|key| {
            keys.push(key);
            std::future::ready(Err(Error::RetryableProblem(
                json!({"status":409}),
                Duration::ZERO,
            )))
        })
        .await;
        assert!(matches!(result, Err(Error::RetryableProblem(_, _))));
        assert_eq!(keys.len(), 3);
        let mut calls = 0;
        let result = retry_revoke(|_| {
            calls += 1;
            std::future::ready(Err(Error::Problem(json!({"status":409}))))
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[tokio::test]
    async fn revoke_client_retries_real_409_headers_with_same_key() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let var = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../var")
            .canonicalize()
            .unwrap();
        let dir = tempfile::tempdir_in(var).unwrap();
        let socket = dir.path().join("revoke.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let target = Uuid::now_v7();
        let server = tokio::spawn(async move {
            let mut keys = Vec::new();
            for attempt in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") {
                    bytes.push(stream.read_u8().await.unwrap());
                }
                let headers = std::str::from_utf8(&bytes).unwrap();
                assert!(headers.starts_with(&format!(
                    "POST /v1/admin/tokens/{target}/revoke HTTP/1.1\r\n"
                )));
                let key = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("idempotency-key: "))
                    .unwrap()
                    .to_owned();
                keys.push(key);
                let mut body = [0; 2];
                stream.read_exact(&mut body).await.unwrap();
                assert_eq!(&body, b"{}");
                let status = if attempt < 2 {
                    "409 Conflict\r\nRetry-After: 0"
                } else {
                    "200 OK"
                };
                stream
                    .write_all(
                        format!("HTTP/1.1 {status}\r\nContent-Length: 2\r\n\r\n{{}}").as_bytes(),
                    )
                    .await
                    .unwrap();
            }
            assert_eq!(keys.len(), 3);
            assert!(keys.iter().all(|key| key == &keys[0]));
        });
        Client::new(socket, "test-token".into())
            .revoke_token(target)
            .await
            .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn revoke_response_retry_uses_transport_status() {
        assert!(
            matches!(response("409 Conflict\r\nRetry-After: 1", br#"{"status":200}"#.to_vec(), false).await, Err(Error::RetryableProblem(_, d)) if d == Duration::from_secs(1))
        );
        assert!(matches!(
            response("409 Conflict", br#"{"status":409}"#.to_vec(), false).await,
            Err(Error::Problem(_))
        ));
        assert!(
            response("200 OK\r\nRetry-After: 1", b"{}".to_vec(), false)
                .await
                .is_ok()
        );
    }

    #[test]
    fn writes_have_fresh_keys_and_host() {
        let a = request("POST", "/v1/test", json!({}), None, "porchlight").unwrap();
        let b = request("POST", "/v1/test", json!({}), None, "porchlight").unwrap();
        assert_ne!(
            a.headers()["idempotency-key"],
            b.headers()["idempotency-key"]
        );
        assert_eq!(a.headers()["host"], "porchlight");
        assert!(
            !request("GET", "/v1/test", Value::Null, None, "porchlight")
                .unwrap()
                .headers()
                .contains_key("idempotency-key")
        );
        assert_eq!(segment("a/b?# é"), "a%2Fb%3F%23%20%C3%A9");
    }
    #[tokio::test]
    async fn get_is_bodyless_and_enrollment_uses_uri_host() {
        let get = request(
            "GET",
            "/v1/test",
            json!({"ignored":true}),
            None,
            "porchlight",
        )
        .unwrap();
        assert!(!get.headers().contains_key("content-type"));
        assert!(!get.headers().contains_key("idempotency-key"));
        assert!(
            get.into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .is_empty()
        );
        for host in ["localhost", "localhost:8444", "[::1]:12345"] {
            let post = request("POST", "/v1/enrollment/complete", json!({}), None, host).unwrap();
            assert_eq!(post.headers()["host"], host);
            assert_eq!(post.headers()["content-type"], "application/json");
            assert!(post.headers().contains_key("idempotency-key"));
            assert!(!post.headers().contains_key("authorization"));
        }
    }

    async fn response(status: &str, body: Vec<u8>, chunked: bool) -> Result<Value> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (client, mut server) = tokio::io::duplex(8192);
        let status = status.to_owned();
        let task = tokio::spawn(async move {
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(server.read_u8().await.unwrap());
            }
            let framing = if chunked {
                "Transfer-Encoding: chunked".into()
            } else {
                format!("Content-Length: {}", body.len())
            };
            server
                .write_all(format!("HTTP/1.1 {status}\r\n{framing}\r\n\r\n").as_bytes())
                .await
                .unwrap();
            if chunked {
                server
                    .write_all(format!("{:x}\r\n", body.len()).as_bytes())
                    .await
                    .unwrap();
            }
            // The client may close early upon hitting the response limit.
            let _ = server.write_all(&body).await;
            if chunked {
                let _ = server.write_all(b"\r\n0\r\n\r\n").await;
            }
        });
        let result = send(
            client,
            request("GET", "/", Value::Null, None, "porchlight").unwrap(),
        )
        .await;
        task.await.unwrap();
        result
    }

    #[tokio::test]
    async fn responses_are_limited_with_and_without_content_length() {
        for chunked in [false, true] {
            let mut exact = vec![b' '; MAX_RESPONSE_BYTES];
            exact[..2].copy_from_slice(b"{}");
            assert_eq!(response("200 OK", exact, chunked).await.unwrap(), json!({}));
            for status in ["200 OK", "502 Bad Gateway"] {
                let error = response(status, vec![b'x'; MAX_RESPONSE_BYTES + 1], chunked)
                    .await
                    .unwrap_err();
                assert!(matches!(
                    error,
                    Error::Local("HTTP response exceeds 4 MiB limit")
                ));
            }
        }
    }

    #[tokio::test]
    async fn non_json_errors_keep_http_status_without_exposing_body() {
        for status in ["502 Bad Gateway", "404 Not Found"] {
            let error = response(status, b"do_not_display_response_body".to_vec(), false)
                .await
                .unwrap_err();
            let problem = error.problem();
            assert_eq!(problem["title"], format!("HTTP {status}"));
            assert!(problem["status"].is_number());
            assert!(!problem.to_string().contains("do_not_display_response_body"));
        }
        let error = response("200 OK", b"invalid JSON".to_vec(), false)
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Local("Invalid JSON response")));
        assert!(error.problem()["status"].is_null());
        assert!(Error::Message("Controlled local error".into()).problem()["status"].is_null());
    }

    #[test]
    fn secret_debug_is_redacted() {
        let client = Client::new("ignored".into(), "never_show_this".into());
        assert!(!format!("{client:?}").contains("never_show_this"));
        assert!(
            !format!("{:?}", Error::Problem(json!({"detail":"never_show_this"})))
                .contains("never_show_this")
        );
    }
}
