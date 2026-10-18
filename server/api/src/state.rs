//! Cheap-clone shared application state.
use crate::{config::Settings, pki_store::ServerPki, tls::RevokedSet};
use sqlx::PgPool;
use std::{ops::Deref, sync::Arc};

#[derive(Clone)]
pub struct AppState(pub Arc<AppInner>);
pub struct AppInner {
    pub disk_budget: crate::blobs::DiskBudget,
    pub upload_limits: crate::blobs::UploadLimits,
    pub upload_streams: crate::blobs::UploadStreams,
    pub ingest_slots: Arc<tokio::sync::Semaphore>,
    /// Calendar import bodies buffered at once (each up to ~30 MiB as JSON), held until commit.
    pub calendar_body_slots: Arc<tokio::sync::Semaphore>,
    pub pool: PgPool,
    pub pki: Arc<ServerPki>,
    pub revoked: RevokedSet,
    pub revocations: tokio::sync::broadcast::Sender<uuid::Uuid>,
    pub settings: Settings,
    pub maintenance: crate::maintenance::Fence,
    pub client_release: tokio::sync::OnceCell<Option<Arc<crate::release::Store>>>,
    pub release_watch: crate::release_watch::ReleaseWatch,
    pub hub: crate::stream_hub::StreamHub,
    pub member_spool_limits: crate::concurrency::Limits,
    pub export_limits: crate::concurrency::Limits,
    pub stream_limits: crate::concurrency::Limits,
    pub release_downloads: crate::concurrency::Limits,
    pub release_hash_slots: Arc<tokio::sync::Semaphore>,
    #[cfg(feature = "testing")]
    pub release_open_hook: std::sync::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(feature = "testing")]
    pub stream_pre_subscribe_delay: std::time::Duration,
    #[cfg(feature = "testing")]
    pub stream_pre_subscribe_started: tokio::sync::Notify,
    #[cfg(feature = "testing")]
    pub mutation_body_started: tokio::sync::Notify,
    #[cfg(feature = "testing")]
    pub background_derivation: bool,
    pub hasher: Arc<porchlight_core::auth::password::PasswordHasher>,
    pub hash_slots: Arc<tokio::sync::Semaphore>,
    pub idem: crate::idempotency::IdempotencyKeys,
    pub limits: crate::ratelimit::RateLimiter,
    /// Injectable domain clock; authentication retains its own real-time validity checks.
    pub calendar_fetcher: crate::calendar::fetch::Fetcher,
    pub item_clock: Arc<dyn Fn() -> chrono::DateTime<chrono::Utc> + Send + Sync>,
}
impl AppState {
    /// Single production/test constructor. All keys and runtime hooks are initialized here.
    pub async fn new(
        pool: PgPool,
        pki: Arc<ServerPki>,
        settings: Settings,
        master_key: &[u8; 32],
        hasher: Arc<porchlight_core::auth::password::PasswordHasher>,
        limits: crate::ratelimit::RateLimiter,
    ) -> sqlx::Result<Self> {
        let root = settings.data_dir.join("blobs");
        let disk_budget = crate::blobs::DiskBudget::default();
        let budget = disk_budget.clone();
        tokio::task::spawn_blocking(move || budget.reconcile(&root))
            .await
            .map_err(|_| {
                sqlx::Error::Io(std::io::Error::other("blob quota reconciliation failed"))
            })?
            .map_err(sqlx::Error::Io)?;
        let revoked = RevokedSet::default();
        revoked.extend(porchlight_db::devices::revoked_fingerprints(&pool).await?);
        Ok(Self(Arc::new(AppInner {
            pool,
            disk_budget,
            upload_limits: crate::blobs::UploadLimits::default(),
            upload_streams: crate::blobs::UploadStreams::default(),
            ingest_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            calendar_body_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            pki,
            revoked,
            client_release: tokio::sync::OnceCell::new(),
            release_watch: crate::release_watch::ReleaseWatch::new(&settings),
            maintenance: crate::maintenance::Fence::new(settings.maintenance_file.clone()),
            settings,
            hasher,
            limits,
            calendar_fetcher: crate::calendar::fetch::Fetcher::default(),
            item_clock: Arc::new(chrono::Utc::now),
            revocations: tokio::sync::broadcast::channel(1024).0,
            hub: crate::stream_hub::StreamHub::default(),
            member_spool_limits: crate::concurrency::Limits::new(2, None),
            export_limits: crate::concurrency::Limits::new(1, None),
            stream_limits: crate::concurrency::Limits::new(3, Some(8)),
            release_downloads: crate::concurrency::Limits::new(2, None),
            release_hash_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            #[cfg(feature = "testing")]
            release_open_hook: std::sync::Mutex::new(None),
            hash_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            idem: crate::idempotency::IdempotencyKeys::new(master_key),
            #[cfg(feature = "testing")]
            stream_pre_subscribe_delay: std::time::Duration::ZERO,
            #[cfg(feature = "testing")]
            stream_pre_subscribe_started: tokio::sync::Notify::new(),
            #[cfg(feature = "testing")]
            mutation_body_started: tokio::sync::Notify::new(),
            #[cfg(feature = "testing")]
            background_derivation: true,
        })))
    }
}
impl Deref for AppState {
    type Target = AppInner;
    fn deref(&self) -> &AppInner {
        &self.0
    }
}
