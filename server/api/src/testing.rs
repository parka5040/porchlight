//! Shared integration harness. Generated keys and sockets live only under the project's var directory.
mod auth_helpers;
mod enrollment_helpers;
mod fixture;
mod notification_probe;
mod stream_helpers;
pub(crate) use notification_probe::record_notification_source_query;
pub use notification_probe::with_notification_source_probe;
pub mod trust;
use crate::{
    config::Settings,
    db::APP_SETTINGS,
    init::{InitOutput, InitParams, finish_init, initialize},
    pki_store::{DataDir, load},
    server::{self, Listeners, NetPolicy},
    state::AppState,
};
use axum::http::{HeaderMap, Request};
pub use fixture::{FixtureUser, HouseholdFixture};
use futures_util::FutureExt;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use porchlight_core::auth::{password::PasswordHasher, session::Platform};
use porchlight_db::{
    testing::{Role, connect_options_as},
    tx::{Actor, begin_as},
};
use porchlight_pki::{ServerSans, generate_csr};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};
pub use stream_helpers::{SseFrame, TestStream};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream, UnixStream},
    sync::oneshot,
    task::JoinHandle,
};
use tokio_rustls::{TlsConnector, client::TlsStream};
use uuid::Uuid;

pub struct TestOptions {
    pub upload_limits: crate::blobs::UploadLimits,
    pub settings: Settings,
    pub watch_revocations: bool,
    /// Disable worker/scheduler only when testing transport or intentionally stopped derivation.
    pub background_derivation: bool,
    pub calendar_fetcher: crate::calendar::fetch::Fetcher,
    pub fail_seed_once: bool,
    pub db_pool_size: u32,
    pub stream_pre_subscribe_delay: Duration,
    pub funnel_peers: Vec<ipnet::IpNet>,
    pub enroll_cidrs: Vec<ipnet::IpNet>,
    pub proxy_timeout: Duration,
    pub tls_timeout: Duration,
    pub header_timeout: Duration,
    pub max_connections: usize,
    pub limits: crate::ratelimit::RateLimits,
    pub rate_clock: Arc<dyn Fn() -> std::time::Instant + Send + Sync>,
    pub item_clock: Arc<dyn Fn() -> chrono::DateTime<chrono::Utc> + Send + Sync>,
}
impl Default for TestOptions {
    fn default() -> Self {
        Self {
            upload_limits: crate::blobs::UploadLimits::default(),
            settings: Settings::for_tests(),
            watch_revocations: true,
            background_derivation: true,
            calendar_fetcher: crate::calendar::fetch::Fetcher::default(),
            fail_seed_once: false,
            db_pool_size: 4,
            stream_pre_subscribe_delay: Duration::ZERO,
            funnel_peers: vec!["127.0.0.1/32".parse().unwrap()],
            enroll_cidrs: vec!["127.0.0.0/8".parse().unwrap()],
            proxy_timeout: Duration::from_secs(1),
            tls_timeout: Duration::from_secs(2),
            header_timeout: Duration::from_secs(30),
            max_connections: 512,
            limits: crate::ratelimit::RateLimits::generous(),
            rate_clock: Arc::new(std::time::Instant::now),
            item_clock: Arc::new(chrono::Utc::now),
        }
    }
}
// A private runtime directory, rather than the OS temp directory (keys must remain in var/).
struct RuntimeDir(PathBuf);
impl RuntimeDir {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var");
        std::fs::create_dir_all(&root).unwrap();
        let path = root
            .canonicalize()
            .unwrap()
            // Short random name: the agent socket inside must fit SUN_LEN (108 bytes) even from a deep
            // workstream copy (scripts/wt.sh); the tail of a UUIDv7 is random.
            .join(format!("t{}", &Uuid::now_v7().simple().to_string()[20..]));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for RuntimeDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
pub struct TestServer {
    pub lan: SocketAddr,
    pub funnel: SocketAddr,
    pub enroll: SocketAddr,
    pub agent_socket: PathBuf,
    pub ca_pem: String,
    pub ca_fingerprint: [u8; 32],
    pub state: AppState,
    pub init: InitOutput,
    pub admin_password: String,
    _dir: RuntimeDir,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<anyhow::Result<()>>>,
}
impl TestServer {
    pub async fn start(pool: &PgPool) -> Self {
        Self::start_with(pool, TestOptions::default()).await
    }
    pub async fn start_with(pool: &PgPool, mut opts: TestOptions) -> Self {
        let dir = RuntimeDir::new();
        let data = DataDir::new(&dir.0);
        let pool = PgPoolOptions::new()
            .max_connections(opts.db_pool_size)
            .connect_with(
                connect_options_as(pool, Role::App, Some("porchlight-api-test"))
                    .unwrap()
                    .options(APP_SETTINGS.iter().copied()),
            )
            .await
            .unwrap();
        let admin_password = "correct horse battery staple".to_owned();
        let init = initialize(
            &pool,
            &data,
            InitParams {
                household_name: "Test Household".into(),
                tzid: "America/New_York".into(),
                admin_username: "admin".into(),
                admin_display_name: "Admin".into(),
                admin_password: admin_password.clone().into(),
                sans: ServerSans {
                    dns: vec!["localhost".into()],
                    ips: vec!["127.0.0.1".parse().unwrap()],
                },
                age_recipient: format!("age1{}", "q".repeat(58)),
            },
            &PasswordHasher::with_params(8, 1, 1),
            SystemTime::now(),
        )
        .await
        .unwrap();
        finish_init(&data).unwrap();
        let pki = Arc::new(load(&data).unwrap());
        let ca_pem = pki.ca.cert().cert_pem.clone();
        let ca_fingerprint = pki.ca.cert().fingerprint;
        let lan_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let funnel_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let enroll_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let lan = lan_listener.local_addr().unwrap();
        let funnel = funnel_listener.local_addr().unwrap();
        let enroll = enroll_listener.local_addr().unwrap();
        if opts.settings.enroll_host == "localhost" {
            opts.settings.enroll_host = format!("localhost:{}", enroll.port());
        }
        opts.settings.data_dir = dir.0.clone();
        let mut state = AppState::new(
            pool,
            pki,
            opts.settings,
            &crate::pki_store::load_idempotency_key(&data).unwrap(),
            Arc::new(PasswordHasher::with_params(8, 1, 1)),
            crate::ratelimit::RateLimiter::with_clock(opts.limits, opts.rate_clock),
        )
        .await
        .unwrap();
        Arc::get_mut(&mut state.0)
            .expect("new state is uniquely owned")
            .stream_pre_subscribe_delay = opts.stream_pre_subscribe_delay;
        Arc::get_mut(&mut state.0)
            .expect("new state is uniquely owned")
            .background_derivation = opts.background_derivation;
        Arc::get_mut(&mut state.0)
            .expect("new state is uniquely owned")
            .item_clock = opts.item_clock;
        Arc::get_mut(&mut state.0)
            .expect("new state is uniquely owned")
            .upload_limits = opts.upload_limits;
        Arc::get_mut(&mut state.0)
            .expect("new state uniquely owned")
            .calendar_fetcher = opts.calendar_fetcher;
        let agent_socket = dir.0.join("agent.sock");
        let listeners = Listeners {
            lan: lan_listener,
            funnel: Some(funnel_listener),
            enroll: enroll_listener,
            agent: Some(server::bind_agent_socket(&agent_socket).unwrap()),
        };
        let policy = NetPolicy {
            funnel_peers: opts.funnel_peers,
            enroll_cidrs: opts.enroll_cidrs,
            proxy_timeout: opts.proxy_timeout,
            tls_timeout: opts.tls_timeout,
            header_timeout: opts.header_timeout,
            max_connections: opts.max_connections,
        };
        let (shutdown, rx) = oneshot::channel();
        let shared_shutdown = async move {
            let _ = rx.await;
        }
        .shared();
        let run_state = state.clone();
        let task = tokio::spawn(async move {
            crate::lifecycle::serve_all(
                run_state,
                listeners,
                policy,
                shared_shutdown,
                opts.watch_revocations,
                opts.fail_seed_once,
            )
            .await?;
            Ok(())
        });
        Self {
            lan,
            funnel,
            enroll,
            agent_socket,
            ca_pem,
            ca_fingerprint,
            state,
            init,
            admin_password,
            _dir: dir,
            shutdown: Some(shutdown),
            task: Some(task),
        }
    }
    /// Direct certificate issuance; no enrollment or session. Writes still carry actor attribution.
    pub async fn issue_device(&self, member_id: Uuid, platform: Platform) -> TestDevice {
        let id = Uuid::now_v7();
        let (csr, key) = generate_csr("Test device").unwrap();
        let cert = self
            .state
            .pki
            .ca
            .issue_device(
                &csr,
                &id.to_string(),
                SystemTime::now(),
                self.state.settings.device_cert_days,
            )
            .unwrap();
        let mut tx = begin_as(
            &self.state.pool,
            &Actor {
                member_id: Some(member_id),
                device_id: None,
                request_id: Uuid::now_v7(),
                agent_label: None,
                token_id: None,
            },
        )
        .await
        .unwrap();
        let household: Uuid =
            sqlx::query_scalar("SELECT household_id FROM porchlight.household_members WHERE id=$1")
                .bind(member_id)
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        sqlx::query("INSERT INTO porchlight.devices (id,household_id,member_id,label,platform) VALUES ($1,$2,$3,'Test device',$4)").bind(id).bind(household).bind(member_id).bind(platform.as_str()).execute(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO porchlight.device_certificates (household_id,device_id,fingerprint_sha256,serial,not_before,not_after) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(household).bind(id).bind(cert.fingerprint.as_slice()).bind(&cert.serial_hex).bind(chrono::DateTime::<chrono::Utc>::from(cert.not_before)).bind(chrono::DateTime::<chrono::Utc>::from(cert.not_after)).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        TestDevice {
            device_id: id,
            cert_pem: cert.cert_pem,
            key_pem: key.to_string(),
            fingerprint: cert.fingerprint,
            session: None,
        }
    }
    pub async fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        }
        self.state.pool.close().await;
    }
}
impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
pub struct TestDevice {
    pub device_id: Uuid,
    pub cert_pem: String,
    pub key_pem: String,
    pub fingerprint: [u8; 32],
    pub session: Option<String>,
}
impl std::fmt::Debug for TestDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestDevice")
            .field("device_id", &self.device_id)
            .field("key_pem", &"[REDACTED]")
            .field("session", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}
#[derive(Debug, thiserror::Error)]
pub enum TestClientError {
    #[error("test client I/O failed")]
    Io(#[from] std::io::Error),
    #[error("test client HTTP failed")]
    Http(#[from] hyper::Error),
    #[error("test client deadline exceeded")]
    Timeout(#[from] tokio::time::error::Elapsed),
}
pub struct TestClient {
    config: Arc<rustls::ClientConfig>,
    server_name: String,
    timeout: Duration,
}
/// Default response deadline of [`TestClient`] requests and [`TestClient::send`].
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
impl TestClient {
    pub fn new(ca_pem: &str, device: Option<&TestDevice>) -> Self {
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(CertificateDer::from_pem_slice(ca_pem.as_bytes()).unwrap())
            .unwrap();
        let builder = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots);
        let mut config = if let Some(d) = device {
            builder
                .with_client_auth_cert(
                    vec![CertificateDer::from_pem_slice(d.cert_pem.as_bytes()).unwrap()],
                    PrivateKeyDer::from_pem_slice(d.key_pem.as_bytes()).unwrap(),
                )
                .unwrap()
        } else {
            builder.with_no_client_auth()
        };
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Self {
            config: Arc::new(config),
            server_name: "localhost".into(),
            timeout: RESPONSE_TIMEOUT,
        }
    }
    /// Longer response deadline for a request that deliberately queues behind a test-held lock until
    /// [`wait_blocked_behind`] releases it, so the liveness bound exceeds that wait's own bound.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    pub async fn connect(&self, addr: SocketAddr) -> Result<TlsStream<TcpStream>, TestClientError> {
        self.connect_stream(TcpStream::connect(addr).await?).await
    }
    /// Supports inspecting peer certificates/HandshakeKind and custom fragmented prefaces.
    pub async fn connect_stream(
        &self,
        stream: TcpStream,
    ) -> Result<TlsStream<TcpStream>, TestClientError> {
        Ok(tokio::time::timeout(
            Duration::from_secs(5),
            TlsConnector::from(self.config.clone()).connect(
                ServerName::try_from(self.server_name.clone()).unwrap(),
                stream,
            ),
        )
        .await??)
    }
    pub async fn request(
        &self,
        addr: SocketAddr,
        req: Request<Full<Bytes>>,
    ) -> Result<TestResponse, TestClientError> {
        self.request_with_prefix(addr, &[], req).await
    }
    pub async fn request_with_prefix(
        &self,
        addr: SocketAddr,
        prefix: &[u8],
        req: Request<Full<Bytes>>,
    ) -> Result<TestResponse, TestClientError> {
        let mut stream = TcpStream::connect(addr).await?;
        stream.write_all(prefix).await?;
        Self::send_within(self.connect_stream(stream).await?, req, self.timeout).await
    }
    pub async fn send<I>(io: I, req: Request<Full<Bytes>>) -> Result<TestResponse, TestClientError>
    where
        I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        Self::send_within(io, req, RESPONSE_TIMEOUT).await
    }
    pub async fn send_within<I>(
        io: I,
        req: Request<Full<Bytes>>,
        timeout: Duration,
    ) -> Result<TestResponse, TestClientError>
    where
        I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut sender, connection) =
            hyper::client::conn::http1::handshake(TokioIo::new(io)).await?;
        let task = tokio::spawn(connection);
        let result = tokio::time::timeout(timeout, async {
            let response = sender.send_request(req).await?;
            let (parts, body) = response.into_parts();
            let body = body.collect().await?.to_bytes();
            Ok::<_, TestClientError>(TestResponse {
                status: parts.status.as_u16(),
                headers: parts.headers,
                body,
            })
        })
        .await;
        drop(sender);
        task.abort();
        let _ = task.await;
        result?
    }
}
pub struct TestResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
}
impl std::fmt::Debug for TestResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestResponse")
            .field("status", &self.status)
            .field("headers", &"[REDACTED]")
            .field("body", &"[REDACTED]")
            .finish()
    }
}
impl TestResponse {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}
pub fn proxy_v2_header(src: SocketAddr, dst: SocketAddr) -> Vec<u8> {
    let mut bytes = b"\r\n\r\n\0\r\nQUIT\n".to_vec();
    bytes.push(0x21);
    match (src, dst) {
        (SocketAddr::V4(s), SocketAddr::V4(d)) => {
            bytes.push(0x11);
            bytes.extend_from_slice(&12u16.to_be_bytes());
            bytes.extend_from_slice(&s.ip().octets());
            bytes.extend_from_slice(&d.ip().octets());
        }
        (SocketAddr::V6(s), SocketAddr::V6(d)) => {
            bytes.push(0x21);
            bytes.extend_from_slice(&36u16.to_be_bytes());
            bytes.extend_from_slice(&s.ip().octets());
            bytes.extend_from_slice(&d.ip().octets());
        }
        _ => panic!("PROXY source and destination families must match"),
    }
    bytes.extend_from_slice(&src.port().to_be_bytes());
    bytes.extend_from_slice(&dst.port().to_be_bytes());
    bytes
}
pub async fn agent_request(
    socket: &Path,
    token: Option<&str>,
    req: Request<Full<Bytes>>,
) -> TestResponse {
    agent_request_within(socket, token, req, RESPONSE_TIMEOUT).await
}
/// Scoped watchdog for probes that deliberately wait for the server's 5 s lock
/// timeout. The ordinary agent transport retains its strict default deadline.
pub async fn agent_request_within(
    socket: &Path,
    token: Option<&str>,
    mut req: Request<Full<Bytes>>,
    timeout: Duration,
) -> TestResponse {
    if let Some(token) = token {
        req.headers_mut().insert(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
    }
    TestClient::send_within(UnixStream::connect(socket).await.unwrap(), req, timeout)
        .await
        .unwrap()
}

/// Waits until at least `waiters` backends of the current database are blocked behind `holder`
/// (the `pg_backend_pid()` of the transaction holding the lock), directly or through a queue of
/// waiters (a second row-lock waiter queues on the first one's tuple lock). Proves that requests
/// overlap on THAT lock. Never count every `wait_event_type = 'Lock'` backend: PostgreSQL
/// serializes the commits of all NOTIFYing transactions in the cluster on one heavyweight lock
/// (every outbox trigger and the projector NOTIFY), so unrelated commits also wait on a lock.
/// `within` is a liveness bound only; keep it well above scheduling and commit latency.
pub async fn wait_blocked_behind(
    pool: &PgPool,
    holder: i32,
    waiters: i64,
    within: Duration,
) -> Result<(), tokio::time::error::Elapsed> {
    tokio::time::timeout(within, async {
        loop {
            let blocked: i64 = sqlx::query_scalar(
                "WITH RECURSIVE waiting AS MATERIALIZED (
                    SELECT pid, pg_blocking_pids(pid) AS blockers FROM pg_stat_activity
                    WHERE datname = current_database() AND wait_event_type = 'Lock'
                 ), behind(pid) AS (
                    SELECT pid FROM waiting WHERE $1 = ANY(blockers)
                    UNION
                    SELECT w.pid FROM waiting w JOIN behind b ON b.pid = ANY(w.blockers)
                 ) SELECT count(*) FROM behind",
            )
            .bind(holder)
            .fetch_one(pool)
            .await
            .unwrap();
            if blocked >= waiters {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
}

/// Task-local calendar admission observations and deterministic deadline injection.
#[derive(Default, Debug)]
pub struct CalendarApplyProbe {
    pub duration: Option<Duration>,
    pub expire_after_write: bool,
    pub contention_after_write: bool,
    pub work: Vec<usize>,
    pub projected_rows: Vec<usize>,
    pub existing_rows: Vec<usize>,
    /// Once set, the apply deadline fires on the next poll, before the apply is polled again:
    /// a hand-polled test cancels it at an exact protocol step (R86).
    pub cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Set while `expire_after_write` holds the clock paused; the fired deadline resumes it.
    pub paused: bool,
}
tokio::task_local! {
    static CALENDAR_APPLY_PROBE: std::cell::RefCell<CalendarApplyProbe>;
}
pub async fn with_calendar_apply_probe<F: std::future::Future>(
    probe: CalendarApplyProbe,
    future: F,
) -> (F::Output, CalendarApplyProbe) {
    CALENDAR_APPLY_PROBE
        .scope(std::cell::RefCell::new(probe), async {
            let output = future.await;
            (output, CALENDAR_APPLY_PROBE.with(|p| p.take()))
        })
        .await
}
pub(crate) fn calendar_apply_duration() -> Duration {
    CALENDAR_APPLY_PROBE
        .try_with(|p| p.borrow().duration)
        .ok()
        .flatten()
        .unwrap_or(Duration::from_secs(20))
}
/// Ready once the probe's `cancel` flag is set; pending forever without a probe or flag.
pub(crate) fn calendar_apply_cancelled() -> impl std::future::Future<Output = ()> {
    let flag = CALENDAR_APPLY_PROBE
        .try_with(|p| p.borrow().cancel.clone())
        .ok()
        .flatten();
    std::future::poll_fn(move |_| match &flag {
        Some(flag) if flag.load(std::sync::atomic::Ordering::Acquire) => std::task::Poll::Ready(()),
        _ => std::task::Poll::Pending,
    })
}
pub(crate) fn calendar_apply_work(work: usize, projected: usize, existing: usize) {
    let _ = CALENDAR_APPLY_PROBE.try_with(|p| {
        let mut p = p.borrow_mut();
        p.work.push(work);
        p.projected_rows.push(projected);
        p.existing_rows.push(existing);
    });
}
pub(crate) fn calendar_apply_contention() -> bool {
    CALENDAR_APPLY_PROBE
        .try_with(|p| p.borrow().contention_after_write)
        .unwrap_or(false)
}
/// The apply deadline fired: hand a clock paused by `expire_after_write` back to real time before
/// the caller's follow-up work, whose pool acquire timeout must not be auto-advanced (R86).
pub(crate) fn calendar_apply_deadline_fired() {
    let paused = CALENDAR_APPLY_PROBE
        .try_with(|p| std::mem::take(&mut p.borrow_mut().paused))
        .unwrap_or(false);
    if paused {
        tokio::time::resume();
    }
}
pub(crate) async fn calendar_after_write() {
    let expire = CALENDAR_APPLY_PROBE
        .try_with(|p| p.borrow().expire_after_write)
        .unwrap_or(false);
    if expire {
        // Advance only after a real domain write; no elapsed wall-clock assumption.
        tokio::time::pause();
        let _ = CALENDAR_APPLY_PROBE.try_with(|p| p.borrow_mut().paused = true);
        tokio::time::advance(calendar_apply_duration()).await;
        std::future::pending::<()>().await;
    }
}
