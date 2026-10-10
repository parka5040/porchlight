//! Bounded, DNS-pinned outbound text fetch. Errors are static categories (never remote text).
use http_body_util::{BodyExt, Empty};
use hyper::{Request, body::Bytes};
use hyper_util::rt::TokioIo;
use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
};
use url::Url;

pub const MAX_BYTES: usize = 5 * 1024 * 1024;
#[derive(Clone, Copy)]
enum Content {
    Calendar,
    Release,
}
impl Content {
    fn accept(self) -> &'static str {
        match self {
            Self::Calendar => "text/calendar",
            Self::Release => "application/json, text/plain, application/octet-stream",
        }
    }
    fn types(self) -> &'static [&'static str] {
        match self {
            Self::Calendar => &[
                "text/calendar",
                "application/ics",
                "text/plain",
                "application/octet-stream",
            ],
            Self::Release => &["application/json", "text/plain", "application/octet-stream"],
        }
    }
}
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct FetchError(pub &'static str);
#[derive(Clone)]
pub struct Fetcher {
    tls: Arc<rustls::ClientConfig>,
    #[cfg(feature = "testing")]
    loopback: bool,
}
pub struct Fetched {
    pub content: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}
impl Default for Fetcher {
    fn default() -> Self {
        let roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Self {
            tls: Arc::new(
                rustls::ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            ),
            #[cfg(feature = "testing")]
            loopback: false,
        }
    }
}
impl Fetcher {
    /// Compile-time test-only escape hatch: production cannot trust a private address or a test CA.
    #[cfg(feature = "testing")]
    pub fn local_test(tls: rustls::ClientConfig) -> Self {
        Self {
            tls: Arc::new(tls),
            loopback: true,
        }
    }
    pub async fn fetch(
        &self,
        value: &str,
        allow_http: bool,
        etag: Option<&str>,
        modified: Option<&str>,
    ) -> Result<Fetched, FetchError> {
        tokio::time::timeout(
            Duration::from_secs(20),
            self.inner(value, allow_http, etag, modified, Content::Calendar),
        )
        .await
        .map_err(|_| FetchError("timeout"))?
    }
    pub async fn fetch_release(&self, value: &str) -> Result<String, FetchError> {
        let allow_http = false;
        #[cfg(feature = "testing")]
        let allow_http = allow_http || self.loopback;
        tokio::time::timeout(
            Duration::from_secs(20),
            self.inner(value, allow_http, None, None, Content::Release),
        )
        .await
        .map_err(|_| FetchError("timeout"))??
        .content
        .ok_or(FetchError("http_status"))
    }
    async fn inner(
        &self,
        value: &str,
        allow_http: bool,
        etag: Option<&str>,
        modified: Option<&str>,
        content: Content,
    ) -> Result<Fetched, FetchError> {
        let mut url = validate_url(value, allow_http)?;
        for hop in 0..=3 {
            let host = url.host_str().ok_or(FetchError("invalid_url"))?;
            let port = url
                .port_or_known_default()
                .ok_or(FetchError("invalid_url"))?;
            let addresses: Vec<SocketAddr> =
                tokio::net::lookup_host((host.trim_matches(['[', ']']), port))
                    .await
                    .map_err(|_| FetchError("dns"))?
                    .collect();
            if addresses.is_empty() {
                return Err(FetchError("dns"));
            }
            for addr in &addresses {
                let admitted = public_address(addr.ip());
                #[cfg(feature = "testing")]
                let admitted = admitted || (self.loopback && addr.ip().is_loopback());
                if !admitted {
                    return Err(FetchError("ssrf_refused"));
                }
            }
            // Connect only to a vetted literal SocketAddr; DNS is never consulted by the connector.
            let mut connected = None;
            for address in addresses {
                if let Ok(stream) = TcpStream::connect(address).await {
                    connected = Some(stream);
                    break;
                }
            }
            let tcp = connected.ok_or(FetchError("connect"))?;
            let mut path = url.path().to_owned();
            if let Some(q) = url.query() {
                path.push('?');
                path.push_str(q);
            }
            let mut authority = host.to_owned();
            if let Some(port) = url.port() {
                authority.push(':');
                authority.push_str(&port.to_string());
            }
            let mut builder = Request::builder()
                .uri(path)
                .header("host", authority)
                .header("accept", content.accept());
            // Do not forward source validators across origins on redirects.
            if hop == 0 {
                if let Some(e) = etag {
                    builder = builder.header("if-none-match", e);
                }
                if let Some(m) = modified {
                    builder = builder.header("if-modified-since", m);
                }
            }
            let request = builder
                .body(Empty::<Bytes>::new())
                .map_err(|_| FetchError("invalid_validator"))?;
            let response = if url.scheme() == "https" {
                let name = rustls_pki_types::ServerName::try_from(
                    host.trim_matches(['[', ']']).to_owned(),
                )
                .map_err(|_| FetchError("invalid_url"))?;
                let tls = tokio_rustls::TlsConnector::from(self.tls.clone())
                    .connect(name, tcp)
                    .await
                    .map_err(|_| FetchError("tls"))?;
                send(tls, request).await?
            } else {
                send(tcp, request).await?
            };
            let status = response.status();
            if status.is_redirection() && status.as_u16() != 304 {
                if hop == 3 {
                    return Err(FetchError("redirect_limit"));
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|h| h.to_str().ok())
                    .ok_or(FetchError("redirect"))?;
                let next = url.join(location).map_err(|_| FetchError("redirect"))?;
                url = validate_url(next.as_str(), allow_http)?;
                continue;
            }
            let header = |name| {
                response
                    .headers()
                    .get(name)
                    .and_then(|h| h.to_str().ok())
                    .map(str::to_owned)
            };
            let etag = header("etag");
            let last_modified = header("last-modified");
            if status.as_u16() == 304 {
                return Ok(Fetched {
                    content: None,
                    etag,
                    last_modified,
                });
            }
            if !status.is_success() {
                return Err(FetchError("http_status"));
            }
            let ct = header("content-type").ok_or(FetchError("content_type"))?;
            if !content.types().contains(
                &ct.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
                    .as_str(),
            ) {
                return Err(FetchError("content_type"));
            }
            if header("content-length")
                .and_then(|h| h.parse::<usize>().ok())
                .is_some_and(|n| n > MAX_BYTES)
            {
                return Err(FetchError("too_large"));
            }
            let mut body = response.into_body();
            let mut bytes = Vec::new();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|_| FetchError("body"))?;
                if let Some(data) = frame.data_ref() {
                    if data.len() > MAX_BYTES.saturating_sub(bytes.len()) {
                        return Err(FetchError("too_large"));
                    }
                    bytes.extend_from_slice(data);
                }
            }
            let content = String::from_utf8(bytes).map_err(|_| FetchError("encoding"))?;
            return Ok(Fetched {
                content: Some(content),
                etag,
                last_modified,
            });
        }
        Err(FetchError("redirect_limit"))
    }
}
async fn send<T: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    stream: T,
    request: Request<Empty<Bytes>>,
) -> Result<hyper::Response<hyper::body::Incoming>, FetchError> {
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(|_| FetchError("http"))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    sender
        .send_request(request)
        .await
        .map_err(|_| FetchError("http"))
}
pub fn validate_url(value: &str, allow_http: bool) -> Result<Url, FetchError> {
    let url = Url::parse(value).map_err(|_| FetchError("invalid_url"))?;
    if !matches!(url.scheme(), "https" | "http")
        || (url.scheme() == "http" && !allow_http)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err(FetchError("invalid_url"));
    }
    Ok(url)
}
pub fn public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let value = u32::from(v);
            // Include documentation, benchmarking, reserved/broadcast and protocol-assignment blocks.
            let blocked: &[(u32, u8)] = &[
                (0, 8),
                (0x0a000000, 8),
                (0x64400000, 10),
                (0x7f000000, 8),
                (0xa9fe0000, 16),
                (0xac100000, 12),
                (0xc0000000, 24),
                (0xc0000200, 24),
                (0xc0586300, 24),
                (0xc0a80000, 16),
                (0xc6120000, 15),
                (0xc6336400, 24),
                (0xcb007100, 24),
                (0xe0000000, 3),
            ];
            !blocked
                .iter()
                .any(|(net, bits)| value & (u32::MAX << (32 - bits)) == *net)
        }
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return public_address(IpAddr::V4(v4));
            }
            let n = u128::from(v);
            // Only native global unicast, excluding documentation, Teredo and 6to4 tunneling.
            n >> 125 == 1
                && n >> 96 != 0x20010db8
                && n >> 96 != 0x20010000
                && n >> 112 != 0x2002
                && n >> 100 != 0x2001001
                && n >> 100 != 0x2001002
                && n >> 80 != 0x200100020000
                && n >> 108 != 0x3fff0
        }
    }
}
