//! Listener ownership and per-connection transport identity. Peer policy precedes any reads.
use crate::{
    proxy_protocol::{ProxyError, read_proxy_v2},
    routes,
    state::AppState,
    tls,
};
use axum::{Extension, Router};
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use ipnet::IpNet;
use std::{
    future::{Future, pending},
    io,
    net::SocketAddr,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream, UnixListener},
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinSet,
    time::Instant,
};
use tokio_rustls::TlsAcceptor;
use tracing::instrument::WithSubscriber;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Lan,
    Funnel,
    Enrollment,
    Agent,
}
impl Transport {
    fn label(self) -> &'static str {
        match self {
            Self::Lan => "lan",
            Self::Funnel => "funnel",
            Self::Enrollment => "enrollment",
            Self::Agent => "agent",
        }
    }
}
#[derive(Clone, Debug)]
pub struct ConnInfo {
    pub transport: Transport,
    pub client_addr: Option<SocketAddr>,
    pub client_cert_fingerprint: Option<[u8; 32]>,
}
pub struct Listeners {
    pub lan: TcpListener,
    pub funnel: Option<TcpListener>,
    pub enroll: TcpListener,
    pub agent: Option<UnixListener>,
}
#[derive(Clone, Debug)]
pub struct NetPolicy {
    pub funnel_peers: Vec<IpNet>,
    pub enroll_cidrs: Vec<IpNet>,
    pub proxy_timeout: Duration,
    pub tls_timeout: Duration,
    /// Includes waiting for the next request head on an idle HTTP/1 keep-alive connection.
    pub header_timeout: Duration,
    /// Independent budget for each listener, held from accept through HTTP completion.
    pub max_connections: usize,
}
impl Default for NetPolicy {
    fn default() -> Self {
        Self {
            funnel_peers: Vec::new(),
            enroll_cidrs: Vec::new(),
            proxy_timeout: Duration::from_secs(5),
            tls_timeout: Duration::from_secs(10),
            header_timeout: Duration::from_secs(30),
            max_connections: 512,
        }
    }
}
pub fn bind_agent_socket(path: &Path) -> io::Result<UnixListener> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => std::fs::remove_file(path)?,
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "agent path is not a socket",
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))?;
    Ok(listener)
}
// Only classified errors are retained/logged: no certificate, request, or payload data.
struct ConnectionFailure {
    stage: &'static str,
    kind: String,
}
impl ConnectionFailure {
    fn new(stage: &'static str, kind: impl Into<String>) -> Self {
        Self {
            stage,
            kind: kind.into(),
        }
    }
    fn proxy(error: ProxyError) -> Self {
        Self::new(
            "proxy",
            match error {
                ProxyError::BadSignature => "bad_signature",
                ProxyError::Unsupported(_) => "unsupported",
                ProxyError::TooLong(_) => "too_long",
                ProxyError::Truncated => "truncated",
                ProxyError::Timeout => "timeout",
                ProxyError::Io(_) => "io",
            },
        )
    }
}
fn log_failure(transport: Transport, failure: ConnectionFailure) {
    tracing::debug!(
        transport = transport.label(),
        stage = failure.stage,
        error_kind = failure.kind,
        "connection failed or refused"
    );
}
async fn http<I>(
    io: I,
    router: Router,
    info: ConnInfo,
    header_timeout: Duration,
    hub: crate::stream_hub::StreamHub,
) -> Result<(), ConnectionFailure>
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let connection = hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(header_timeout)
        .serve_connection(
            TokioIo::new(io),
            TowerToHyperService::new(router.layer(Extension(info))),
        );
    tokio::pin!(connection);
    let result = tokio::select! {
        result=connection.as_mut()=>result,
        ()=hub.draining()=>{
            connection.as_mut().graceful_shutdown();
            connection.await
        }
    };
    result.map_err(|e| {
        ConnectionFailure::new(
            "http",
            if e.is_timeout() {
                "timeout"
            } else if e.is_parse() {
                "parse"
            } else {
                "io_or_service"
            },
        )
    })
}
fn canonical_addr(addr: SocketAddr) -> SocketAddr {
    SocketAddr::new(addr.ip().to_canonical(), addr.port())
}
fn peer_allowed(peer: SocketAddr, nets: &[IpNet]) -> bool {
    nets.iter()
        .any(|net| net.contains(&peer.ip().to_canonical()))
}
// Owned by the accept loop: bounded memory, no peer identifiers, one summary per minute.
#[derive(Default)]
struct PeerPolicyCounts {
    allowed: u64,
    refused: u64,
}
impl PeerPolicyCounts {
    fn check(&mut self, transport: Transport, peer: SocketAddr, nets: &[IpNet]) -> bool {
        let allowed = check_peer(transport, peer, nets);
        if allowed {
            self.allowed = self.allowed.saturating_add(1);
        } else {
            self.refused = self.refused.saturating_add(1);
        }
        allowed
    }
    fn report(&mut self, transport: Transport) {
        let counts = std::mem::take(self);
        if counts.allowed != 0 || counts.refused != 0 {
            tracing::info!(
                transport = transport.label(),
                allowed = counts.allowed,
                refused = counts.refused,
                "peer policy summary"
            );
        }
    }
}
fn check_peer(transport: Transport, peer: SocketAddr, nets: &[IpNet]) -> bool {
    let allowed = peer_allowed(peer, nets);
    if transport == Transport::Enrollment {
        tracing::debug!(transport = "enrollment", peer = %peer.ip().to_canonical(), allowed,
            "enrollment peer policy");
    } else if !allowed {
        tracing::debug!(transport = transport.label(), peer = %peer.ip().to_canonical(), "peer refused by policy");
    }
    if !allowed {
        log_failure(transport, ConnectionFailure::new("policy", "peer_refused"));
    }
    allowed
}
async fn tcp(
    mut stream: TcpStream,
    peer: SocketAddr,
    transport: Transport,
    acceptor: TlsAcceptor,
    router: Router,
    policy: NetPolicy,
    hub: crate::stream_hub::StreamHub,
) -> Result<(), ConnectionFailure> {
    let addr = if transport == Transport::Funnel {
        canonical_addr(
            read_proxy_v2(&mut stream, policy.proxy_timeout)
                .await
                .map_err(ConnectionFailure::proxy)?
                .source,
        )
    } else {
        canonical_addr(peer)
    };
    let stream = tokio::time::timeout(policy.tls_timeout, acceptor.accept(stream))
        .await
        .map_err(|_| ConnectionFailure::new("tls", "timeout"))?
        .map_err(|e| ConnectionFailure::new("tls", format!("{:?}", e.kind())))?;
    let fp = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certs| certs.first())
        .map(|cert| porchlight_pki::fingerprint(cert.as_ref()));
    http(
        stream,
        router,
        ConnInfo {
            transport,
            client_addr: Some(addr),
            client_cert_fingerprint: fp,
        },
        policy.header_timeout,
        hub,
    )
    .await
}
async fn optional_tcp(listener: &Option<TcpListener>) -> io::Result<(TcpStream, SocketAddr)> {
    match listener {
        Some(l) => l.accept().await,
        None => pending().await,
    }
}
async fn optional_unix(listener: &Option<UnixListener>) -> io::Result<tokio::net::UnixStream> {
    match listener {
        Some(l) => l.accept().await.map(|(s, _)| s),
        None => pending().await,
    }
}
// The retry deadline lives outside select!: another listener accepting, a completed connection,
// or cancellation of this accept future must not erase a resource-exhaustion backoff.
struct AcceptState {
    retry_at: Instant,
    #[cfg(test)]
    injected_error: Option<io::Error>,
}
impl Default for AcceptState {
    fn default() -> Self {
        Self {
            retry_at: Instant::now(),
            #[cfg(test)]
            injected_error: None,
        }
    }
}
impl AcceptState {
    async fn accept<T>(
        &mut self,
        transport: Transport,
        accept: impl Future<Output = io::Result<T>>,
    ) -> Option<T> {
        tokio::time::sleep_until(self.retry_at).await;
        #[cfg(test)]
        let result = match self.injected_error.take() {
            Some(error) => Err(error),
            None => accept.await,
        };
        #[cfg(not(test))]
        let result = accept.await;
        match result {
            Ok(stream) => Some(stream),
            Err(error) => {
                tracing::warn!(transport = transport.label(), error_kind = ?error.kind(),
                    os_error = error.raw_os_error(), "listener accept failed; continuing");
                // Like axum::serve, connection-local errors need no delay; all other errors
                // (including EMFILE/ENFILE/ENOMEM/ENOBUFS) back off without stopping other listeners.
                if !matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionRefused
                ) {
                    self.retry_at = Instant::now() + Duration::from_millis(250);
                }
                None
            }
        }
    }
}
fn permit(budget: &Arc<Semaphore>, transport: Transport) -> Option<OwnedSemaphorePermit> {
    match budget.clone().try_acquire_owned() {
        Ok(permit) => Some(permit),
        Err(_) => {
            log_failure(
                transport,
                ConnectionFailure::new("capacity", "connection_limit"),
            );
            None
        }
    }
}
fn spawn_connection(
    connections: &mut JoinSet<()>,
    permit: OwnedSemaphorePermit,
    transport: Transport,
    connection: impl Future<Output = Result<(), ConnectionFailure>> + Send + 'static,
) {
    connections.spawn(
        async move {
            let _permit = permit;
            if let Err(failure) = connection.await {
                log_failure(transport, failure);
            }
        }
        .with_current_subscriber(),
    );
}
/// Owns listeners and connections. Shutdown drains HTTP/SSE, then bounds cleanup to five seconds.
pub async fn run(
    state: AppState,
    listeners: Listeners,
    policy: NetPolicy,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    run_inner(
        state,
        listeners,
        policy,
        shutdown,
        std::array::from_fn(|_| AcceptState::default()),
    )
    .await
}
async fn run_inner(
    state: AppState,
    listeners: Listeners,
    policy: NetPolicy,
    shutdown: impl Future<Output = ()>,
    [
        mut lan_accept,
        mut funnel_accept,
        mut enroll_accept,
        mut agent_accept,
    ]: [AcceptState; 4],
) -> anyhow::Result<()> {
    let mtls = TlsAcceptor::from(tls::mtls_server_config(&state.pki, state.revoked.clone())?);
    let enroll_tls = TlsAcceptor::from(tls::enroll_server_config(&state.pki)?);
    let mtls_router = routes::mtls_router(state.clone());
    let enroll_router = routes::enroll_router(state.clone());
    let hub = state.hub.clone();
    let agent_router = routes::agent_router(state);
    let [lan_budget, funnel_budget, enroll_budget, agent_budget] =
        std::array::from_fn::<_, 4, _>(|_| Arc::new(Semaphore::new(policy.max_connections)));
    let mut connections = JoinSet::new();
    let mut enroll_policy = PeerPolicyCounts::default();
    let mut funnel_policy = PeerPolicyCounts::default();
    let period = Duration::from_secs(60);
    let mut policy_reports = tokio::time::interval_at(Instant::now() + period, period);
    policy_reports.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            _ = policy_reports.tick() => {
                enroll_policy.report(Transport::Enrollment);
                funnel_policy.report(Transport::Funnel);
            }
            accepted = lan_accept.accept(Transport::Lan, listeners.lan.accept()) => {
                let Some((stream, peer)) = accepted else { continue };
                let Some(permit) = permit(&lan_budget, Transport::Lan) else { continue };
                spawn_connection(&mut connections, permit, Transport::Lan, tcp(stream, peer, Transport::Lan, mtls.clone(), mtls_router.clone(), policy.clone(), hub.clone()));
            }
            accepted = funnel_accept.accept(Transport::Funnel, optional_tcp(&listeners.funnel)) => {
                let Some((stream, peer)) = accepted else { continue };
                if funnel_policy.check(Transport::Funnel, peer, &policy.funnel_peers) {
                    let Some(permit) = permit(&funnel_budget, Transport::Funnel) else { continue };
                    spawn_connection(&mut connections, permit, Transport::Funnel, tcp(stream, peer, Transport::Funnel, mtls.clone(), mtls_router.clone(), policy.clone(), hub.clone()));
                }
            }
            accepted = enroll_accept.accept(Transport::Enrollment, listeners.enroll.accept()) => {
                let Some((stream, peer)) = accepted else { continue };
                if enroll_policy.check(Transport::Enrollment, peer, &policy.enroll_cidrs) {
                    let Some(permit) = permit(&enroll_budget, Transport::Enrollment) else { continue };
                    spawn_connection(&mut connections, permit, Transport::Enrollment, tcp(stream, peer, Transport::Enrollment, enroll_tls.clone(), enroll_router.clone(), policy.clone(), hub.clone()));
                }
            }
            accepted = agent_accept.accept(Transport::Agent, optional_unix(&listeners.agent)) => {
                let Some(stream) = accepted else { continue };
                let Some(permit) = permit(&agent_budget, Transport::Agent) else { continue };
                spawn_connection(&mut connections, permit, Transport::Agent, http(stream, agent_router.clone(), ConnInfo { transport: Transport::Agent, client_addr: None, client_cert_fingerprint: None }, policy.header_timeout, hub.clone()));
            }
            result = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(_)) = result {
                    tracing::warn!(error_kind = "task_failure", "connection task failed");
                }
            }
        }
    }
    enroll_policy.report(Transport::Enrollment);
    funnel_policy.report(Transport::Funnel);
    // Stop accepting, terminate SSE bodies with a drain hint, and disable keepalive.
    // Slow requests/handshakes receive a bounded grace period, then are cancelled.
    drop(listeners);
    hub.drain();
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while connections.join_next().await.is_some() {}
    })
    .await;
    connections.abort_all();
    while connections.join_next().await.is_some() {}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestClient, TestServer, agent_request, proxy_v2_header};
    use porchlight_core::auth::session::Platform;

    #[test]
    fn connection_logs_are_classified_and_redacted() {
        crate::error::tests::ensure_global_warn_sink();
        use std::io::Write;
        #[derive(Clone)]
        struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);
        impl Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let output = Capture(Arc::new(std::sync::Mutex::new(Vec::new())));
        let writer = output.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let peer = "127.0.0.1:1234".parse().unwrap();
            assert!(check_peer(
                Transport::Enrollment,
                peer,
                &["127.0.0.0/8".parse().unwrap()]
            ));
            assert!(!check_peer(
                Transport::Enrollment,
                peer,
                &["10.0.0.0/8".parse().unwrap()]
            ));
            assert!(!check_peer(Transport::Funnel, peer, &[]));
            log_failure(
                Transport::Funnel,
                ConnectionFailure::proxy(ProxyError::Unsupported("secret-payload-marker")),
            );
            log_failure(
                Transport::Funnel,
                ConnectionFailure::proxy(ProxyError::Io(io::Error::other("secret-payload-marker"))),
            );
            log_failure(
                Transport::Lan,
                ConnectionFailure::new("tls", format!("{:?}", io::ErrorKind::InvalidData)),
            );
            log_failure(
                Transport::Enrollment,
                ConnectionFailure::new("tls", "timeout"),
            );
        });
        let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        assert!(
            text.contains("enrollment")
                && text.contains("peer=127.0.0.1")
                && text.contains("allowed=true")
                && text.contains("allowed=false"),
            "{text}"
        );
        for kind in [
            "peer_refused",
            "unsupported",
            "io",
            "InvalidData",
            "timeout",
        ] {
            assert!(text.contains(kind), "{text}");
        }
        assert!(text.contains("DEBUG") && text.contains("funnel") && text.contains("lan"));
        assert!(
            !text.contains("INFO"),
            "per-peer diagnostics must be debug-only"
        );
        assert!(!text.contains("secret-payload-marker"));
    }

    #[test]
    fn audit_ops_peer_summary_bounds_logs_without_losing_refusals() {
        use std::io::Write;
        #[derive(Clone, Default)]
        struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);
        impl Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let output = Capture::default();
        let writer = output.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let mut counts = PeerPolicyCounts::default();
            let peer = "192.168.77.2:1234".parse().unwrap();
            let nets = ["192.168.77.0/24".parse().unwrap()];
            for _ in 0..5_000 {
                assert!(counts.check(Transport::Enrollment, peer, &nets));
                assert!(!counts.check(Transport::Enrollment, peer, &[]));
            }
            assert!(output.0.lock().unwrap().is_empty());
            counts.report(Transport::Enrollment);
            counts.report(Transport::Enrollment); // No empty summaries during idle periods.
            assert_eq!(counts.allowed, 0);
            assert_eq!(counts.refused, 0);
        });
        let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
        assert_eq!(text.lines().count(), 1, "{text}");
        for field in [
            "peer policy summary",
            "enrollment",
            "allowed=5000",
            "refused=5000",
        ] {
            assert!(text.contains(field), "{text}");
        }
        assert!(!text.contains("192.168.77.2"));
    }

    #[tokio::test]
    async fn accept_backoff_survives_cancellation() {
        let mut state = AcceptState {
            injected_error: Some(io::Error::from_raw_os_error(24)),
            ..Default::default()
        };
        let start = Instant::now();
        assert!(
            state
                .accept(Transport::Lan, pending::<io::Result<()>>())
                .await
                .is_none()
        );
        let retry_at = state.retry_at;
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                state.accept(Transport::Lan, pending::<io::Result<()>>())
            )
            .await
            .is_err()
        );
        assert_eq!(state.retry_at, retry_at);
        assert!(
            state
                .accept(Transport::Lan, async { Ok(()) })
                .await
                .is_some()
        );
        assert!(start.elapsed() >= Duration::from_millis(250));
    }

    #[tokio::test]
    async fn drain_finishes_non_sse_in_flight_response_and_closes_keepalive() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let (started, finish) = (entered.clone(), release.clone());
        let router = Router::new().route(
            "/",
            axum::routing::get(move || {
                let (started, finish) = (started.clone(), finish.clone());
                async move {
                    started.notify_one();
                    finish.notified().await;
                    "finished"
                }
            }),
        );
        let hub = crate::stream_hub::StreamHub::default();
        let (mut client, socket) = tokio::io::duplex(4096);
        let task = tokio::spawn(http(
            socket,
            router,
            ConnInfo {
                transport: Transport::Lan,
                client_addr: None,
                client_cert_fingerprint: None,
            },
            Duration::from_secs(1),
            hub.clone(),
        ));
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), entered.notified())
            .await
            .unwrap();
        hub.drain();
        assert!(
            !task.is_finished(),
            "in-flight request was cancelled at drain"
        );
        release.notify_one();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), client.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        let response = String::from_utf8(response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(
            response.ends_with("finished"),
            "active response was not flushed: {response}"
        );
        // read_to_end returning above proves EOF, not just a completed response.
        // The explicit close header is optional if headers race the drain poll.
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_or_else(|_| panic!("HTTP failed during graceful drain"));
    }

    #[test]
    fn mapped_ipv4_peer_matches_ipv4_policy() {
        let mapped = "[::ffff:127.0.0.1]:1234".parse().unwrap();
        assert!(peer_allowed(mapped, &["127.0.0.0/8".parse().unwrap()]));
        assert!(!peer_allowed(mapped, &["10.0.0.0/8".parse().unwrap()]));
        assert_eq!(canonical_addr(mapped), "127.0.0.1:1234".parse().unwrap());
        assert_eq!(
            canonical_addr("[2001:db8::1]:1234".parse().unwrap()),
            "[2001:db8::1]:1234".parse().unwrap()
        );
    }

    #[sqlx::test(migrations = "../db/migrations")]
    async fn accept_errors_do_not_stop_run(pool: sqlx::PgPool) {
        let fixture = TestServer::start(&pool).await;
        let device = fixture
            .session_device(fixture.init.admin_member_id, Platform::Desktop)
            .await;
        let lan = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let funnel = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let enroll = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addresses = [
            lan.local_addr().unwrap(),
            funnel.local_addr().unwrap(),
            enroll.local_addr().unwrap(),
        ];
        let agent_path = fixture.agent_socket.with_file_name("retry.sock");
        let agent = bind_agent_socket(&agent_path).unwrap();
        let (shutdown, rx) = tokio::sync::oneshot::channel();
        // Linux EMFILE, ENFILE, ENOBUFS, ENOMEM, respectively, on each listener's
        // real accept path. No process-wide descriptor limits or other tests are affected.
        let accepts = [24, 23, 105, 12].map(|errno| AcceptState {
            injected_error: Some(io::Error::from_raw_os_error(errno)),
            ..Default::default()
        });
        let state = fixture.state.clone();
        let task = tokio::spawn(run_inner(
            state,
            Listeners {
                lan,
                funnel: Some(funnel),
                enroll,
                agent: Some(agent),
            },
            NetPolicy {
                funnel_peers: vec!["127.0.0.1/32".parse().unwrap()],
                enroll_cidrs: vec!["127.0.0.0/8".parse().unwrap()],
                ..Default::default()
            },
            async {
                let _ = rx.await;
            },
            accepts,
        ));
        let request = || device.request("GET", "/v1/connection", serde_json::json!({}), None);
        let client = TestClient::new(&fixture.ca_pem, Some(&device));
        assert_eq!(
            client
                .request(addresses[0], request())
                .await
                .unwrap()
                .status,
            200
        );
        let prefix = proxy_v2_header("203.0.113.7:4444".parse().unwrap(), addresses[1]);
        assert_eq!(
            client
                .request_with_prefix(addresses[1], &prefix, request())
                .await
                .unwrap()
                .status,
            200
        );
        assert_eq!(
            TestClient::new(&fixture.ca_pem, None)
                .request(addresses[2], request())
                .await
                .unwrap()
                .status,
            404
        );
        assert_eq!(
            agent_request(&agent_path, None, request()).await.status,
            404
        );
        assert!(!task.is_finished());
        shutdown.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        fixture.stop().await;
    }
}
