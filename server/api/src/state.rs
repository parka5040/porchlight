//! Cheap-clone shared application state.
use crate::{config::Settings, pki_store::ServerPki, tls::RevokedSet};
use sqlx::PgPool;
use std::{ops::Deref, sync::Arc};

// Default admitted heap estimate: 1*512 MiB image decoder (including input/output
// copies) + 2*64 MiB Argon2 + 2*(30 MiB+64 KiB) calendar bodies + 4*12 MiB
// snapshot working sets = 748.125 MiB, below the 768 MiB workload ceiling.
// SN-01 isolated requested-live-heap probe: 1000 items + 10000 occurrences,
// comments and nested metadata produce 9,283,408 JSON bytes at 10,547,301 peak
// heap bytes (old tree writer: 91,699,418). Round up to 12 MiB with headroom;
// scripts/check-snapshot-alloc.sh checks both this estimate and <=2.5x output.
// This is a measured reference workload, NOT a data-size cap: household history
// is unbounded; snapshots are never truncated.
pub const INGEST_SLOTS: usize = 1;
pub const HASH_SLOTS: usize = 2;
pub const CALENDAR_BODY_SLOTS: usize = 2;
pub const CALENDAR_MEMBER_IMPORTS: usize = 1;
pub const SNAPSHOT_DEVICE: usize = 1;
pub const SNAPSHOT_MEMBER: usize = 3;
pub const SNAPSHOT_GLOBAL: usize = 4;
pub const SNAPSHOT_ESTIMATED_HEAP: usize = 12 * 1024 * 1024;
pub const SNAPSHOT_WAIT_SECS: u64 = 15;
pub const DOWNLOAD_DEVICE: usize = 3;
pub const DOWNLOAD_MEMBER: usize = 6;

#[derive(Clone)]
pub struct AppState(pub Arc<AppInner>);
pub struct AppInner {
    pub disk_budget: crate::blobs::DiskBudget,
    pub upload_limits: crate::blobs::UploadLimits,
    pub upload_streams: crate::blobs::UploadStreams,
    pub ingest_slots: Arc<tokio::sync::Semaphore>,
    /// Calendar import bodies buffered at once (each up to ~30 MiB as JSON), held until commit.
    pub calendar_body_slots: Arc<tokio::sync::Semaphore>,
    pub calendar_member_imports: crate::concurrency::Limits,
    pub snapshot_limits: crate::concurrency::Limits,
    pub item_sync_limits: crate::concurrency::Limits,
    pub snapshot_members: crate::concurrency::MemberSlots,
    pub snapshot_slots: Arc<tokio::sync::Semaphore>,
    pub downloads: crate::concurrency::Limits,
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
    pub async fn admit_snapshot(
        &self,
        caller: &crate::extract::Caller,
    ) -> Result<crate::concurrency::SnapshotPermit, crate::error::ApiError> {
        let device = self
            .snapshot_limits
            .acquire(caller.admission_id(), caller.member_id())?;
        // Immediate device admission bounds waiters by the number of devices.
        // Both FIFO waits happen before any DB connection or transaction is acquired.
        let (member, global) = tokio::time::timeout(self.settings.snapshot_wait, async {
            let member = self.snapshot_members.acquire(caller.member_id()).await?;
            let global = self
                .snapshot_slots
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| crate::error::ApiError::internal("quota_closed"))?;
            Ok::<_, crate::error::ApiError>((member, global))
        })
        .await
        .map_err(|_| crate::error::ApiError::new(crate::error::Problem::RateLimited).retry(2))??;
        Ok(crate::concurrency::SnapshotPermit {
            device,
            member,
            global,
        })
    }
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
            ingest_slots: Arc::new(tokio::sync::Semaphore::new(INGEST_SLOTS)),
            calendar_body_slots: Arc::new(tokio::sync::Semaphore::new(CALENDAR_BODY_SLOTS)),
            calendar_member_imports: crate::concurrency::Limits::new(CALENDAR_MEMBER_IMPORTS, None),
            snapshot_limits: crate::concurrency::Limits::new(SNAPSHOT_DEVICE, None),
            item_sync_limits: crate::concurrency::Limits::new(4, None),
            snapshot_members: crate::concurrency::MemberSlots::new(SNAPSHOT_MEMBER),
            snapshot_slots: Arc::new(tokio::sync::Semaphore::new(SNAPSHOT_GLOBAL)),
            downloads: crate::concurrency::Limits::new(DOWNLOAD_DEVICE, Some(DOWNLOAD_MEMBER)),
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
            hash_slots: Arc::new(tokio::sync::Semaphore::new(HASH_SLOTS)),
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

#[cfg(test)]
mod admission_budget_tests {
    use super::*;
    #[test]
    fn admitted_workloads_fit_container_headroom() {
        let mib = 1024 * 1024;
        let image = porchlight_core::ingest::Limits::production().max_alloc_bytes as usize;
        let argon = 65536 * 1024; // production PasswordHasher m_cost in KiB
        let calendar = crate::calendar::fetch::MAX_BYTES * 6 + 64 * 1024;
        let snapshot_estimate = SNAPSHOT_ESTIMATED_HEAP;
        let total = image * INGEST_SLOTS
            + argon * HASH_SLOTS
            + calendar * CALENDAR_BODY_SLOTS
            + snapshot_estimate * SNAPSHOT_GLOBAL;
        assert!(
            total < 768 * mib,
            "default admitted heap estimate {total} exceeds workload ceiling"
        );
    }
}
