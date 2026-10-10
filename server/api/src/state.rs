//! Cheap-clone shared application state.
use crate::{config::Settings, pki_store::ServerPki, tls::RevokedSet};
use sqlx::PgPool;
use std::{ops::Deref, sync::Arc};

#[derive(Clone)]
pub struct AppState(pub Arc<AppInner>);
pub struct AppInner {
    pub upload_limits: crate::blobs::UploadLimits,
    pub upload_streams: crate::blobs::UploadStreams,
    pub ingest_slots: Arc<tokio::sync::Semaphore>,
    pub pool: PgPool,
    pub pki: Arc<ServerPki>,
    pub revoked: RevokedSet,
    pub revocations: tokio::sync::broadcast::Sender<uuid::Uuid>,
    pub settings: Settings,
    pub client_release: tokio::sync::OnceCell<Option<Arc<crate::release::Store>>>,
    pub release_watch: crate::release_watch::ReleaseWatch,
    pub hub: crate::stream_hub::StreamHub,
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
        let revoked = RevokedSet::default();
        revoked.extend(porchlight_db::devices::revoked_fingerprints(&pool).await?);
        Ok(Self(Arc::new(AppInner {
            pool,
            upload_limits: crate::blobs::UploadLimits::default(),
            upload_streams: crate::blobs::UploadStreams::default(),
            ingest_slots: Arc::new(tokio::sync::Semaphore::new(2)),
            pki,
            revoked,
            client_release: tokio::sync::OnceCell::new(),
            release_watch: crate::release_watch::ReleaseWatch::new(&settings),
            settings,
            hasher,
            limits,
            calendar_fetcher: crate::calendar::fetch::Fetcher::default(),
            item_clock: Arc::new(chrono::Utc::now),
            revocations: tokio::sync::broadcast::channel(1024).0,
            hub: crate::stream_hub::StreamHub::default(),
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
