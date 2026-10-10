use super::*;
use tracing::{Instrument, instrument::WithSubscriber};
fn entries(path: &Path, class: &'static str) -> Vec<std::fs::DirEntry> {
    match std::fs::read_dir(path) {
        Ok(rows) => rows
            .filter_map(|row| match row {
                Ok(v) => Some(v),
                Err(_) => {
                    tracing::warn!(path_class = class, "GC directory entry failed");
                    None
                }
            })
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(_) => {
            tracing::warn!(path_class = class, "GC directory scan failed");
            Vec::new()
        }
    }
}
fn directory(entry: &std::fs::DirEntry, class: &'static str) -> bool {
    match entry.file_type() {
        Ok(kind) => kind.is_dir() && !kind.is_symlink(),
        Err(_) => {
            tracing::warn!(path_class = class, "GC entry type failed");
            false
        }
    }
}
fn old_files(root: &Path, now: DateTime<Utc>) -> Vec<(String, i64, DateTime<Utc>)> {
    let mut candidates = Vec::new();
    let inspect =
        |entry: &std::fs::DirEntry| -> std::io::Result<Option<(String, i64, DateTime<Utc>)>> {
            if !entry.file_type()?.is_file() {
                return Ok(None);
            }
            let meta = entry.metadata()?;
            let modified: DateTime<Utc> = meta.modified()?.into();
            if modified > now - chrono::Duration::days(30) {
                return Ok(None);
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') && name.ends_with(".tmp") {
                std::fs::remove_file(entry.path())?;
                std::fs::File::open(entry.path().parent().unwrap())?.sync_all()?;
            } else if blob_path(root, &name).is_ok_and(|p| p == entry.path())
                && meta.len() <= MAX_UPLOAD as u64
            {
                return Ok(Some((name, meta.len() as i64, modified)));
            }
            Ok(None)
        };
    for a in entries(&root.join("blobs"), "blob-root") {
        if !directory(&a, "blob-root") {
            continue;
        }
        let leaves = if a.file_name() == "tmp" {
            entries(&a.path(), "upload-temp")
        } else {
            entries(&a.path(), "blob-shard")
                .into_iter()
                .filter(|b| directory(b, "blob-shard"))
                .flat_map(|b| entries(&b.path(), "blob-shard"))
                .collect()
        };
        for entry in leaves {
            match inspect(&entry) {
                Ok(Some(v)) => candidates.push(v),
                Ok(None) => {}
                Err(_) => tracing::warn!(
                    path_class = "blob-or-temp",
                    "GC candidate I/O failed; continuing"
                ),
            }
        }
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    candidates
}
/// Own metadata transactions; never unlink under an uncommitted caller transaction.
/// A sharded filesystem gate stays locked in an independent transaction across commit/unlink.
/// Daily scheduler callback. Blob row UPDATE locks serialize upload, lease creation and deletion.
/// A missing unreferenced file is harmless (a prior interrupted sweep); never follow a temp filename.
/// Crash-safe recovery of files written before the upload transaction committed.
/// Filesystem names are candidates only: INSERT ON CONFLICT first waits on any concurrent upload,
/// then ordinary GC rechecks authoritative references and active leases under blob row locks.
pub async fn gc(
    pool: &sqlx::PgPool,
    metadata_pool: &sqlx::PgPool,
    root: &Path,
    now: DateTime<Utc>,
) -> anyhow::Result<usize> {
    gc_with_unlink(
        pool,
        metadata_pool,
        root,
        now,
        std::sync::Arc::new(|path: &Path| std::fs::remove_file(path)),
    )
    .await
}
/// Blocking unlink GC runs on the blocking pool; `gc` uses `std::fs::remove_file`.
pub type Unlink = std::sync::Arc<dyn Fn(&Path) -> std::io::Result<()> + Send + Sync>;
/// `gc` with a per-call unlink, so tests can hold the filesystem phase (no global hooks).
#[doc(hidden)]
pub async fn gc_with_unlink(
    pool: &sqlx::PgPool,
    metadata_pool: &sqlx::PgPool,
    root: &Path,
    now: DateTime<Utc>,
    unlink: Unlink,
) -> anyhow::Result<usize> {
    let (orphans, candidates) = {
        // Returned to the pool before the per-candidate gates: the scheduler's sweep pool has one
        // connection, and each gate must be an owned transaction (see `commit_and_unlink`).
        let mut conn = pool.acquire().await?;
        let households: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM porchlight.households ORDER BY id LIMIT 2")
                .fetch_all(&mut *conn)
                .await?;
        let mut orphans = std::collections::BTreeMap::new();
        // One self-hosted household owns the data volume. Do not assign orphan bytes ambiguously.
        if let [household] = households.as_slice() {
            let root = root.to_owned();
            for (hash, size, modified) in
                tokio::task::spawn_blocking(move || old_files(&root, now)).await?
            {
                let exists: bool = sqlx::query_scalar(BLOB_EXISTS)
                    .bind(household)
                    .bind(&hash)
                    .fetch_one(&mut *conn)
                    .await?;
                if !exists {
                    orphans.insert((*household, hash), (size, modified));
                }
            }
        }
        let mut candidates: Vec<(Uuid, String)> = sqlx::query_as(GC_CANDIDATES)
            .bind(now)
            .fetch_all(&mut *conn)
            .await?;
        candidates.extend(orphans.keys().cloned());
        candidates.sort();
        candidates.dedup();
        (orphans, candidates)
    };
    let mut count = 0;
    // Acquire/adopt in the SAME global order as uploads and leases; never stage all orphans first.
    let mut skipped_shards = std::collections::BTreeSet::new();
    for (household, hash) in candidates {
        if skipped_shards.contains(&hash[..2]) {
            continue;
        }
        let mut gate = porchlight_db::tx::begin(pool).await?;
        if let Err(error) = sqlx::query(LOCK_STORAGE_SHARD)
            .bind(&hash[..2])
            .fetch_one(&mut *gate)
            .await
        {
            let lock_timeout = error
                .as_database_error()
                .is_some_and(|e| e.code().as_deref() == Some("55P03"));
            gate.rollback().await?;
            if lock_timeout {
                tracing::warn!(category = "shard_lock_timeout", "GC skips busy shard");
                skipped_shards.insert(hash[..2].to_owned());
                continue;
            }
            return Err(error.into());
        }
        let mut tx = porchlight_db::tx::begin(metadata_pool).await?;
        let other = match delete_candidate(
            &mut tx,
            household,
            &hash,
            now,
            orphans.get(&(household, hash.clone())),
        )
        .await
        {
            Ok(Some(other)) => other,
            Ok(None) => {
                tx.rollback().await?;
                gate.commit().await?;
                continue;
            }
            Err(error) => {
                // Flush both rollbacks before releasing either pool connection; do not queue a
                // drop rollback on an aborted metadata transaction during sweep teardown.
                tx.rollback().await?;
                gate.rollback().await?;
                return Err(error);
            }
        };
        let remove = (!other).then(|| (root.to_owned(), hash, unlink.clone()));
        // Awaited, but owned by its own task: dropping this sweep cannot cut the phase short.
        let phase = commit_and_unlink(tx, gate, remove)
            .in_current_span()
            .with_current_subscriber();
        tokio::spawn(phase).await??;
        count += 1;
    }
    sqlx::query("DELETE FROM porchlight.blob_leases WHERE expires_at <= $1")
        .bind(now)
        .execute(pool)
        .await?;
    Ok(count)
}

/// Metadata COMMIT → unlink + shard fsync → gate release, run to completion by its own task.
/// The scheduler drops an active tick on shutdown while HTTP requests drain, and blocking
/// filesystem work is never cancelled: if dropping the sweep could drop `gate`, a same-shard
/// upload waiting on it could re-reference the hash and commit before the still-pending unlink
/// removed its bytes. Cancelled earlier, both transactions roll back and nothing is deleted; a
/// crash inside this phase still leaves at worst an unreachable orphan for a later sweep.
async fn commit_and_unlink(
    tx: sqlx::Transaction<'static, sqlx::Postgres>,
    gate: sqlx::Transaction<'static, sqlx::Postgres>,
    remove: Option<(PathBuf, String, Unlink)>,
) -> anyhow::Result<()> {
    tx.commit().await?;
    if let Some((root, hash, unlink)) = remove {
        let path = blob_path(&root, &hash).map_err(|_| anyhow::anyhow!("invalid blob hash"))?;
        let (removed, synced) = tokio::task::spawn_blocking(move || {
            let removed = !matches!(
                unlink(&path),
                Err(e) if e.kind() != std::io::ErrorKind::NotFound
            );
            let dir = path.parent().unwrap();
            let synced =
                !dir.exists() || std::fs::File::open(dir).and_then(|d| d.sync_all()).is_ok();
            (removed, synced)
        })
        .await?;
        if !removed {
            tracing::warn!(
                path_class = "blob",
                "GC unlink failed; retry as orphan next sweep"
            );
        }
        if !synced {
            tracing::warn!(path_class = "blob-shard", "GC fsync failed");
        }
    }
    gate.commit().await?;
    Ok(())
}

async fn delete_candidate(
    conn: &mut PgConnection,
    household: Uuid,
    hash: &str,
    now: DateTime<Utc>,
    orphan: Option<&(i64, DateTime<Utc>)>,
) -> anyhow::Result<Option<bool>> {
    if let Some((size, modified)) = orphan {
        sqlx::query("SELECT porchlight.pl_adopt_orphan_blob($1,$2,$3,$4,$5)")
            .bind(household)
            .bind(hash)
            .bind(size)
            .bind(modified)
            .bind(now)
            .execute(&mut *conn)
            .await?;
    }
    let locked: Option<bool> = sqlx::query_scalar(LOCK_OLD_BLOB)
        .bind(household)
        .bind(hash)
        .bind(now)
        .fetch_optional(&mut *conn)
        .await?;
    if locked.is_none() {
        return Ok(None);
    }
    let eligible: bool = sqlx::query_scalar(BLOB_IS_UNREFERENCED_AND_UNLEASED)
        .bind(household)
        .bind(hash)
        .bind(now)
        .fetch_one(&mut *conn)
        .await?;
    if !eligible {
        return Ok(None);
    }
    // Soft-deleted links retain attribution/name but no longer require a FK to a GC'd blob.
    sqlx::query(DELETE_BLOB_LEASE_ITEMS)
        .bind(household)
        .bind(hash)
        .execute(&mut *conn)
        .await?;
    let other: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM porchlight.blobs WHERE sha256=$1 AND household_id<>$2)",
    )
    .bind(hash)
    .bind(household)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query(DELETE_BLOB_METADATA)
        .bind(household)
        .bind(hash)
        .execute(&mut *conn)
        .await?;
    Ok(Some(other))
}
const BLOB_EXISTS: &str =
    "SELECT EXISTS(SELECT 1 FROM porchlight.blobs WHERE household_id=$1 AND sha256=$2)";
const GC_CANDIDATES: &str = "SELECT b.household_id,b.sha256 FROM porchlight.blobs b WHERE b.last_referenced_at <= $1 - interval '30 days' AND NOT EXISTS(SELECT 1 FROM porchlight.attachments a WHERE a.household_id=b.household_id AND a.deleted_at IS NULL AND (a.blob_sha256=b.sha256 OR a.thumbnail_sha256=b.sha256)) ORDER BY b.household_id,b.sha256";
const LOCK_STORAGE_SHARD: &str =
    "SELECT shard FROM porchlight.blob_storage_locks WHERE shard=$1 FOR UPDATE";
const LOCK_OLD_BLOB: &str = "SELECT true FROM porchlight.blobs WHERE household_id=$1 AND sha256=$2 AND last_referenced_at <= $3 - interval '30 days' FOR UPDATE SKIP LOCKED";
const BLOB_IS_UNREFERENCED_AND_UNLEASED: &str = "SELECT NOT EXISTS(SELECT 1 FROM porchlight.attachments a WHERE a.household_id=$1 AND a.deleted_at IS NULL AND (a.blob_sha256=$2 OR a.thumbnail_sha256=$2)) AND NOT EXISTS(SELECT 1 FROM porchlight.blob_lease_items li JOIN porchlight.blob_leases l ON l.id=li.lease_id WHERE li.household_id=$1 AND li.sha256=$2 AND l.expires_at>$3)";
const DELETE_BLOB_LEASE_ITEMS: &str =
    "DELETE FROM porchlight.blob_lease_items WHERE household_id=$1 AND sha256=$2";
const DELETE_BLOB_METADATA: &str =
    "DELETE FROM porchlight.blobs WHERE household_id=$1 AND sha256=$2";
