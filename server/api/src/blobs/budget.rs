//! Count all blob-root files, including fresh orphans and temps, plus pending writes.
//! Metadata remains a second conservative bound for retained or missing committed blobs.
use super::*;
use std::sync::{Arc, Mutex};

// The lowest permanent shard doubles as the volume admission gate. This uses existing
// fixed, non-writable lock rows, and preserves ascending shard order with GC.
pub(super) const LOCK_BUDGET: &str =
    "SELECT shard FROM porchlight.blob_storage_locks WHERE shard='00' FOR UPDATE";

#[derive(Clone, Default)]
pub struct DiskBudget {
    reserved: Arc<Mutex<u64>>,
    quota: Arc<Mutex<Quota>>,
    #[cfg(any(test, feature = "testing"))]
    scans: Arc<std::sync::atomic::AtomicUsize>,
}
#[derive(Default)]
struct Quota {
    used: u64,
    pending: u64,
}
pub(super) struct QuotaReservation {
    quota: Arc<Mutex<Quota>>,
    bytes: u64,
}
pub struct DiskReservation {
    reserved: Arc<Mutex<u64>>,
    bytes: u64,
}
impl DiskBudget {
    #[cfg(any(test, feature = "testing"))]
    pub fn reserved_bytes(&self) -> u64 {
        *self.reserved.lock().unwrap()
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn scan_count(&self) -> usize {
        self.scans.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// Only startup and GC walk the volume. The same mutex serializes every file mutation,
    /// so reconciliation cannot race a write/rename/unlink or lose outstanding reservations.
    pub(crate) fn reconcile(&self, root: &Path) -> std::io::Result<u64> {
        let mut quota = self
            .quota
            .lock()
            .map_err(|_| std::io::Error::other("blob quota poisoned"))?;
        let result = self.scan(root);
        quota.used = result.as_ref().copied().unwrap_or(u64::MAX);
        result
    }
    /// This volume belongs to one household. Never follow links outside its blob root.
    fn scan(&self, root: &Path) -> std::io::Result<u64> {
        #[cfg(any(test, feature = "testing"))]
        self.scans
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match std::fs::symlink_metadata(root) {
            Ok(meta) if meta.is_dir() => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(e),
            _ => return Err(std::io::Error::other("unexpected blob storage root")),
        }
        // Retain directory iterators, not an inventory proportional to the number of files.
        let mut directories = vec![std::fs::read_dir(root)?];
        let mut bytes = 0_u64;
        while let Some(directory) = directories.last_mut() {
            let Some(entry) = directory.next() else {
                directories.pop();
                continue;
            };
            let entry = entry?;
            let meta = match entry.metadata() {
                Ok(meta) => meta,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            if meta.is_dir() {
                directories.push(std::fs::read_dir(entry.path())?);
            } else if meta.is_file() {
                bytes = bytes.saturating_add(meta.len());
            } else {
                return Err(std::io::Error::other("unexpected blob storage entry"));
            }
        }
        Ok(bytes)
    }
    /// Held by the actual writer, including unabortable work after request cancellation.
    /// Files already written may be counted twice until it finishes (conservative).
    pub(super) fn reserve_quota(
        &self,
        bytes: u64,
        limit: u64,
    ) -> Result<QuotaReservation, ApiError> {
        let mut quota = self
            .quota
            .lock()
            .map_err(|_| ApiError::internal("blob_quota"))?;
        if quota.used == u64::MAX
            || quota
                .used
                .checked_add(quota.pending)
                .and_then(|used| used.checked_add(bytes))
                .is_none_or(|used| used > limit)
        {
            return Err(ApiError::new(Problem::StorageFull));
        }
        quota.pending = quota.pending.saturating_add(bytes);
        Ok(QuotaReservation {
            quota: self.quota.clone(),
            bytes,
        })
    }
    /// Account only the paths changed by one blocking operation (O(1) for each chunk).
    /// Inspect after errors too: partial writes, failed cleanup and post-rename fsync failures
    /// still own physical bytes. An unreadable result fails closed until GC reconciles it.
    pub(super) fn change<T>(
        &self,
        paths: &[&Path],
        write: impl FnOnce() -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        let mut quota = self
            .quota
            .lock()
            .map_err(|_| std::io::Error::other("blob quota poisoned"))?;
        let sizes = || {
            paths
                .iter()
                .try_fold(0_u64, |sum, path| match std::fs::symlink_metadata(path) {
                    Ok(meta) if meta.is_file() => Ok(sum.saturating_add(meta.len())),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(sum),
                    Err(e) => Err(e),
                    _ => Err(std::io::Error::other("unexpected blob storage entry")),
                })
        };
        let before = match sizes() {
            Ok(before) => before,
            Err(e) => {
                quota.used = u64::MAX;
                return Err(e);
            }
        };
        let result = write();
        match sizes() {
            Ok(after) if quota.used != u64::MAX => {
                quota.used = quota.used.saturating_sub(before).saturating_add(after);
            }
            Ok(_) => (),
            Err(e) => {
                quota.used = u64::MAX;
                return Err(e);
            }
        }
        result
    }
    /// Conservatively charge allocated blocks plus directory/temp-file headroom. Active staging
    /// and blocking writes retain their reservations even if their HTTP future is cancelled.
    pub(crate) fn reserve(
        &self,
        root: &Path,
        bytes: u64,
        reserve: u64,
    ) -> Result<DiskReservation, ApiError> {
        let mut reserved = self
            .reserved
            .lock()
            .map_err(|_| ApiError::internal("disk_budget"))?;
        let fs = rustix::fs::statvfs(root).map_err(|_| ApiError::internal("disk_space"))?;
        let block = fs.f_frsize.max(4096);
        let bytes = bytes.saturating_add(block - 1) / block * block;
        let bytes = bytes.saturating_add(16 * block);
        let available = fs.f_bavail.saturating_mul(fs.f_frsize);
        if available < reserve.saturating_add(*reserved).saturating_add(bytes) {
            return Err(ApiError::new(Problem::StorageFull));
        }
        *reserved = reserved.saturating_add(bytes);
        Ok(DiskReservation {
            reserved: self.reserved.clone(),
            bytes,
        })
    }
}
impl Drop for QuotaReservation {
    fn drop(&mut self) {
        let mut quota = self.quota.lock().unwrap_or_else(|e| e.into_inner());
        quota.pending -= self.bytes;
    }
}
impl DiskReservation {
    /// Grow a retained anonymous spool reservation before writing. Unlike a chunk
    /// reservation, these bytes stay in the ledger until the actual file is closed.
    pub(crate) fn grow(&mut self, root: &Path, bytes: u64, reserve: u64) -> Result<(), ApiError> {
        let mut reserved = self
            .reserved
            .lock()
            .map_err(|_| ApiError::internal("disk_budget"))?;
        let fs = rustix::fs::statvfs(root).map_err(|_| ApiError::internal("disk_space"))?;
        let block = fs.f_frsize.max(4096);
        let bytes = bytes.saturating_add(block - 1) / block * block;
        let available = fs.f_bavail.saturating_mul(fs.f_frsize);
        if available < reserve.saturating_add(*reserved).saturating_add(bytes) {
            return Err(ApiError::new(Problem::StorageFull));
        }
        *reserved = reserved.saturating_add(bytes);
        self.bytes = self.bytes.saturating_add(bytes);
        Ok(())
    }
}
impl Drop for DiskReservation {
    fn drop(&mut self) {
        let mut reserved = self.reserved.lock().unwrap_or_else(|e| e.into_inner());
        *reserved -= self.bytes;
    }
}

pub(super) async fn admit(
    conn: &mut PgConnection,
    state: &AppState,
    household: Uuid,
    blobs: &[(&String, &str, &Vec<u8>)],
) -> Result<(QuotaReservation, DiskReservation), ApiError> {
    let used: i64 = sqlx::query_scalar(
        "SELECT coalesce(sum(size),0)::bigint FROM porchlight.blobs WHERE household_id=$1",
    )
    .bind(household)
    .fetch_one(&mut *conn)
    .await?;
    let mut additional = 0_u64;
    let mut writing = 0_u64;
    for (hash, _, bytes) in blobs {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM porchlight.blobs WHERE household_id=$1 AND sha256=$2)",
        )
        .bind(household)
        .bind(hash)
        .fetch_one(&mut *conn)
        .await?;
        if !exists {
            additional += bytes.len() as u64;
        }
        // durable_write also stages replacements of deduplicated content.
        writing += bytes.len() as u64;
    }
    if (used as u64).saturating_add(additional) > state.settings.attachment_quota_bytes {
        return Err(ApiError::new(Problem::StorageFull));
    }
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let root = state.settings.data_dir.join("blobs");
        let quota = state
            .disk_budget
            .reserve_quota(writing, state.settings.attachment_quota_bytes)?;
        let disk = state
            .disk_budget
            .reserve(&root, writing, state.settings.disk_reserve_bytes)?;
        Ok((quota, disk))
    })
    .await
    .map_err(|_| ApiError::internal("blob_quota_task"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconciliation_keeps_pending_bytes_and_errors_keep_partial_writes() {
        let dir =
            tempfile::tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var")).unwrap();
        let budget = DiskBudget::default();
        let pending = budget.reserve_quota(8, 8).unwrap();
        budget.reconcile(dir.path()).unwrap();
        assert!(
            budget.reserve_quota(1, 8).is_err(),
            "GC must not erase an outstanding writer"
        );
        let file = dir.path().join(".partial.tmp");
        let result: std::io::Result<()> = budget.change(&[&file], || {
            std::fs::write(&file, b"1234")?;
            Err(std::io::Error::other("partial write fixture"))
        });
        assert!(result.is_err());
        drop(pending);
        assert!(
            budget.reserve_quota(5, 8).is_err(),
            "partial write stays charged after failure"
        );
        assert!(budget.reserve_quota(4, 8).is_ok());
        budget
            .change(&[&file], || std::fs::write(&file, b"12"))
            .unwrap();
        assert!(
            budget.reserve_quota(6, 8).is_ok(),
            "replacement only charges its final size"
        );
        assert!(budget.reserve_quota(7, 8).is_err());
        assert!(
            budget
                .change::<()>(&[&file], || Err(std::io::Error::other("unlink fixture")))
                .is_err()
        );
        assert!(
            budget.reserve_quota(7, 8).is_err(),
            "failed unlink cannot release bytes"
        );
        budget
            .change(&[&file], || std::fs::remove_file(&file))
            .unwrap();
        assert!(budget.reserve_quota(8, 8).is_ok());
        assert_eq!(budget.scan_count(), 1);
        let invalid = dir.path().join("not-a-directory");
        std::fs::write(&invalid, b"x").unwrap();
        assert!(budget.reconcile(&invalid).is_err());
        assert!(
            budget.reserve_quota(1, u64::MAX).is_err(),
            "scan errors fail closed even at the largest quota"
        );
        budget.reconcile(dir.path()).unwrap();
        assert!(
            budget.reserve_quota(7, 8).is_ok(),
            "a later successful GC scan repairs the ledger"
        );
    }

    #[test]
    fn fresh_temps_and_cancelled_writer_reservations_count_until_deleted() {
        let dir =
            tempfile::tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var")).unwrap();
        let root = dir.path();
        let budget = DiskBudget::default();
        let pending = budget.reserve_quota(8, 8).unwrap();
        assert!(budget.reserve_quota(1, 8).is_err());
        // Simulate the unabortable writer completing after cancellation, without metadata.
        let temp = root.join(".fresh.tmp");
        budget
            .change(&[&temp], || std::fs::write(&temp, b"12345678"))
            .unwrap();
        drop(pending);
        assert!(budget.reserve_quota(1, 8).is_err());
        assert_eq!(budget.reconcile(root).unwrap(), 8);
        let restarted = DiskBudget::default();
        restarted.reconcile(root).unwrap();
        assert!(restarted.reserve_quota(1, 8).is_err());
        restarted
            .change(&[&temp], || std::fs::remove_file(&temp))
            .unwrap();
        assert!(restarted.reserve_quota(8, 8).is_ok());
    }
}
