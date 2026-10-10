//! One read-only RR snapshot, one anonymous table spool, bounded gzip backpressure.
use super::HOUSEHOLD_TABLES;
use crate::{blobs::blob_path, state::AppState};
use axum::body::Bytes;
use futures_util::TryStreamExt;
use serde_json::{Value, json};
use sqlx::{PgPool, types::Json};
use std::{
    future::Future,
    io::{BufWriter, Seek, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{runtime::Handle, sync::mpsc::Sender};
use uuid::Uuid;

type Output = Sender<Result<Bytes, std::io::Error>>;
fn interrupted() -> std::io::Error {
    std::io::Error::other("export interrupted")
}
fn db_error(_: sqlx::Error) -> std::io::Error {
    interrupted()
}

/// Also runs when preparation fails, the client disconnects, or the task unwinds.
struct LeaseGuard {
    pool: PgPool,
    id: Uuid,
    runtime: Handle,
}
impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let (pool, id) = (self.pool.clone(), self.id);
        self.runtime.spawn(async move {
            if sqlx::query("delete from porchlight.blob_leases where id=$1")
                .bind(id)
                .execute(&pool)
                .await
                .is_err()
            {
                tracing::warn!("household export lease release failed");
            }
        });
    }
}
// Declared after the tar: poison output before tar/gzip drop finalizers, also on panic.
struct AbortOnDrop(Arc<AtomicBool>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
struct ChannelWriter {
    aborted: Arc<AtomicBool>,
    send: Output,
    pool: PgPool,
    lease: Uuid,
    runtime: Handle,
    renewed: Instant,
}
impl Write for ChannelWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.aborted.load(Ordering::Acquire) {
            return Err(interrupted());
        }
        for chunk in bytes.chunks(64 * 1024) {
            // Renew only on observed gzip output progress, never a heartbeat
            // for a stalled client. Expiry fails closed instead of reviving a pin.
            if self.renewed.elapsed() >= Duration::from_secs(300) {
                let live: bool = wait(
                    &self.runtime,
                    &self.send,
                    sqlx::query_scalar("select porchlight.pl_renew_blob_lease($1)")
                        .bind(self.lease)
                        .fetch_one(&self.pool),
                )?;
                if !live {
                    return Err(interrupted());
                }
                self.renewed = Instant::now();
            }
            self.runtime.block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(300),
                    self.send.send(Ok(Bytes::copy_from_slice(chunk))),
                )
                .await
                .map_err(|_| interrupted())?
                .map_err(|_| interrupted())
            })?;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
// Every DB wait after acquisition is cancellable, including a table blocked by DDL.
fn wait<T>(
    runtime: &Handle,
    send: &Output,
    f: impl Future<Output = Result<T, sqlx::Error>>,
) -> std::io::Result<T> {
    runtime.block_on(async {
        tokio::select! { biased;
            _ = send.closed() => Err(interrupted()),
            value = f => value.map_err(db_error),
        }
    })
}
fn append(
    tar: &mut tar::Builder<flate2::write::GzEncoder<ChannelWriter>>,
    name: &str,
    mut file: std::fs::File,
) -> std::io::Result<()> {
    file.rewind()?;
    tar.append_file(name, &mut file)
}

pub(super) fn produce(
    s: AppState,
    household: Uuid,
    send: Output,
    runtime: Handle,
) -> std::io::Result<()> {
    produce_with(s, household, send, runtime, |_| Ok(()))
}

// The phase boundary is injectable so backpressure regressions need no sleeps.
fn produce_with(
    s: AppState,
    household: Uuid,
    send: Output,
    runtime: Handle,
    before_blobs: impl FnOnce(i32) -> std::io::Result<()>,
) -> std::io::Result<()> {
    // Acquire in this owned task, not a cancel-prone handler. Even if the client
    // leaves during acquisition, the returned lease immediately gains its guard.
    let lease = runtime
        .block_on(
            sqlx::query_scalar("select porchlight.pl_take_blob_lease('household-export')")
                .fetch_one(&s.pool),
        )
        .map_err(db_error)?;
    let _lease = LeaseGuard {
        pool: s.pool.clone(),
        id: lease,
        runtime: runtime.clone(),
    };
    let mut renewed = Instant::now();
    // Cancellable wait, detached BEGIN: a disconnect mid-BEGIN must not strand the transaction
    // on a pooled connection (the next export's lease and SET TRANSACTION would run inside it).
    let mut tx = wait(&runtime, &send, porchlight_db::tx::begin(&s.pool))?;
    wait(
        &runtime,
        &send,
        sqlx::query("set transaction isolation level repeatable read read only").execute(&mut *tx),
    )?;
    #[cfg(not(test))]
    let backend = 0;
    #[cfg(test)]
    let backend: i32 = wait(
        &runtime,
        &send,
        sqlx::query_scalar("select pg_backend_pid()").fetch_one(&mut *tx),
    )?;
    let missing: bool = wait(&runtime, &send, sqlx::query_scalar("select exists(select 1 from porchlight.attachments a cross join lateral unnest(array[a.blob_sha256,a.thumbnail_sha256]) hash where a.household_id=$1 and a.deleted_at is null and hash is not null and not exists(select 1 from porchlight.blob_lease_items li where li.lease_id=$2 and li.household_id=a.household_id and li.sha256=hash))").bind(household).bind(lease).fetch_one(&mut *tx))?;
    if missing {
        return Err(interrupted());
    }
    let aborted = Arc::new(AtomicBool::new(false));
    let gzip = flate2::write::GzEncoder::new(
        ChannelWriter {
            aborted: aborted.clone(),
            send: send.clone(),
            pool: s.pool.clone(),
            lease,
            runtime: runtime.clone(),
            renewed: Instant::now(),
        },
        flate2::Compression::default(),
    );
    let mut tar = tar::Builder::new(gzip);
    let _abort = AbortOnDrop(aborted);
    let mut table_rows = std::collections::BTreeMap::new();
    let manifest = serde_json::to_vec(&json!({"format_version":2,"exported_at":(s.item_clock)(),"tables":HOUSEHOLD_TABLES.iter().map(|n| (*n, format!("tables/{n}.jsonl"))).collect::<std::collections::BTreeMap<_,_>>()})).map_err(|_| interrupted())?;
    let mut header = tar::Header::new_gnu();
    header.set_size(manifest.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    tar.append_data(&mut header, "data.json", manifest.as_slice())?;
    for &name in HOUSEHOLD_TABLES {
        let has_h: bool = wait(&runtime, &send, sqlx::query_scalar("select exists(select 1 from pg_attribute where attrelid=to_regclass('porchlight.'||$1) and attname='household_id' and not attisdropped)").bind(name).fetch_one(&mut *tx))?;
        let condition = if name == "households" {
            "t.id=$1"
        } else if has_h {
            "t.household_id=$1"
        } else {
            match name {
                "member_daily_load" | "member_load_jobs" | "notification_delivery_retry" => {
                    "t.member_id in (select id from porchlight.household_members where household_id=$1)"
                }
                "notification_sources" | "notification_event_delivery" => {
                    "t.event_id in (select id from porchlight.notification_events where household_id=$1)"
                }
                "notification_kinds" | "scheduler_jobs" | "backup_runs" => "true",
                _ => return Err(interrupted()),
            }
        };
        // No whole-table fetch or sort. SQLx streams a cursor's rows; only the
        // current row is decoded. Anonymous 0600 files live under runtime state.
        let sql = format!(
            "select to_jsonb(t)-'search_document'-'during' from porchlight.{name} t where {condition} and $1::uuid is not null"
        );
        let mut rows = sqlx::query_scalar::<_, Json<Value>>(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(household)
            .fetch(&mut *tx);
        let mut spool = BufWriter::new(tempfile::tempfile_in(&s.settings.data_dir)?);
        let mut count = 0u64;
        while let Some(Json(row)) = wait(&runtime, &send, rows.try_next())? {
            serde_json::to_writer(&mut spool, &row)?;
            spool.write_all(b"\n")?;
            count += 1;
            if renewed.elapsed() >= Duration::from_secs(300) {
                let live: bool = wait(
                    &runtime,
                    &send,
                    sqlx::query_scalar("select porchlight.pl_renew_blob_lease($1)")
                        .bind(lease)
                        .fetch_one(&s.pool),
                )?;
                if !live {
                    return Err(interrupted());
                }
                renewed = Instant::now();
            }
        }
        drop(rows);
        let spool = spool.into_inner().map_err(|_| interrupted())?;
        append(&mut tar, &format!("tables/{name}.jsonl"), spool)?;
        table_rows.insert(name, count);
    }
    // Fully consume blob metadata in the RR snapshot. No SELECT/cursor or
    // pooled snapshot connection may span client-paced blob writes.
    let hashes: Vec<String> = wait(&runtime, &send, sqlx::query_scalar("select distinct li.sha256 from porchlight.blob_lease_items li where li.lease_id=$1 and li.household_id=$2 and exists(select 1 from porchlight.attachments a where a.household_id=li.household_id and a.deleted_at is null and (a.blob_sha256=li.sha256 or a.thumbnail_sha256=li.sha256)) order by li.sha256").bind(lease).bind(household).fetch_all(&mut *tx))?;
    wait(&runtime, &send, tx.commit())?;
    before_blobs(backend)?;
    for hash in &hashes {
        use std::os::unix::fs::OpenOptionsExt;
        let path = blob_path(&s.settings.data_dir, hash).map_err(|_| interrupted())?;
        if path.ancestors().any(|p| p.is_symlink()) {
            return Err(interrupted());
        }
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err(interrupted());
        }
        tar.append_file(
            format!("blobs/{}/{}/{}", &hash[..2], &hash[2..4], hash),
            &mut file,
        )?;
    }
    // A stalled consumer cannot make an expired snapshot pin look successful.
    let live: bool = wait(
        &runtime,
        &send,
        sqlx::query_scalar("select porchlight.pl_renew_blob_lease($1)")
            .bind(lease)
            .fetch_one(&s.pool),
    )?;
    if !live {
        return Err(interrupted());
    }
    // Written LAST, after every table/blob, commit and final live-pin check.
    // Absence of this marker is incomplete even if an HTTP peer hides errors.
    let complete = serde_json::to_vec(&json!({"format_version":2,"complete":true,"table_rows":table_rows,"blob_count":hashes.len(),"blob_sha256":hashes})).map_err(|_| interrupted())?;
    let mut header = tar::Header::new_gnu();
    header.set_size(complete.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    tar.append_data(&mut header, "complete.json", complete.as_slice())?;
    tar.finish()?;
    tar.get_mut().try_finish()?;
    Ok(())
}

#[cfg(all(test, feature = "testing"))]
mod tests {
    use super::*;
    use crate::testing::{TestOptions, TestServer};
    use std::io::Read;

    async fn fixture(pool: &PgPool, missing_second: bool) -> TestServer {
        let s = TestServer::start_with(
            pool,
            TestOptions {
                background_derivation: false,
                ..Default::default()
            },
        )
        .await;
        let h = s.init.household_id;
        let item: Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,kind,title) values($1,$2,'todo','Blob phase') returning id").bind(h).bind(s.init.admin_member_id).fetch_one(pool).await.unwrap();
        // Deterministic incompressible bytes, enough to saturate the bounded sink.
        let mut seed = 1u64;
        let data: Vec<u8> = (0..512 * 1024)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as u8
            })
            .collect();
        for hash in ["a".repeat(64), "b".repeat(64)] {
            sqlx::query("insert into porchlight.blobs(household_id,sha256,size,content_type) values($1,$2,$3,'text/plain')").bind(h).bind(&hash).bind(data.len() as i64).execute(pool).await.unwrap();
            if !missing_second || hash.starts_with('a') {
                let path = blob_path(&s.state.settings.data_dir, &hash).unwrap();
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, &data).unwrap();
            }
        }
        sqlx::query("insert into porchlight.attachments(household_id,item_id,uploader_id,original_name,blob_sha256,thumbnail_sha256) values($1,$2,$3,'blob.bin',$4,$5)").bind(h).bind(item).bind(s.init.admin_member_id).bind("a".repeat(64)).bind("b".repeat(64)).execute(pool).await.unwrap();
        s
    }

    #[sqlx::test(migrations = "../db/migrations")]
    async fn mid_blob_failure_does_not_finalize_gzip(pool: PgPool) {
        let s = fixture(&pool, true).await;
        let (send, mut receive) = tokio::sync::mpsc::channel(2);
        let state = s.state.clone();
        let h = s.init.household_id;
        let runtime = Handle::current();
        let worker = tokio::task::spawn_blocking(move || produce(state, h, send, runtime));
        let mut compressed = Vec::new();
        while let Some(chunk) = receive.recv().await {
            compressed.extend_from_slice(&chunk.unwrap());
        }
        assert!(worker.await.unwrap().is_err());
        let mut decoded = Vec::new();
        let result = flate2::read::GzDecoder::new(compressed.as_slice()).read_to_end(&mut decoded);
        assert!(result.is_err(), "failed export finalized gzip");
        assert!(
            !decoded
                .windows(b"complete.json".len())
                .any(|w| w == b"complete.json")
        );
        s.stop().await;
    }

    #[sqlx::test(migrations = "../db/migrations")]
    async fn unwind_also_aborts_drop_finalizers(pool: PgPool) {
        let s = fixture(&pool, false).await;
        let (send, mut receive) = tokio::sync::mpsc::channel(2);
        let state = s.state.clone();
        let h = s.init.household_id;
        let runtime = Handle::current();
        let worker = tokio::task::spawn_blocking(move || {
            produce_with(state, h, send, runtime, |_| {
                panic!("injected export unwind")
            })
        });
        let mut compressed = Vec::new();
        while let Some(chunk) = receive.recv().await {
            compressed.extend_from_slice(&chunk.unwrap());
        }
        assert!(worker.await.unwrap_err().is_panic());
        let mut decoded = Vec::new();
        assert!(
            flate2::read::GzDecoder::new(compressed.as_slice())
                .read_to_end(&mut decoded)
                .is_err()
        );
        s.stop().await;
    }

    #[sqlx::test(migrations = "../db/migrations")]
    async fn paused_blob_sink_has_no_open_snapshot_or_statement(pool: PgPool) {
        let s = fixture(&pool, false).await;
        let (send, mut receive) = tokio::sync::mpsc::channel(2);
        let (phase_send, mut phase_receive) = tokio::sync::oneshot::channel();
        let (resume, gate) = std::sync::mpsc::channel();
        let state = s.state.clone();
        let h = s.init.household_id;
        let runtime = Handle::current();
        let worker = tokio::task::spawn_blocking(move || {
            produce_with(state, h, send, runtime, |pid| {
                phase_send.send(pid).unwrap();
                gate.recv().unwrap();
                Ok(())
            })
        });
        let mut compressed = Vec::new();
        let pid = loop {
            tokio::select! {
                pid = &mut phase_receive => break pid.unwrap(),
                chunk = receive.recv() => compressed.extend_from_slice(&chunk.unwrap().unwrap()),
            }
        };
        // Release the phase gate, but deliberately stop draining the sink.
        resume.send(()).unwrap();
        while receive.len() < 2 && !worker.is_finished() {
            tokio::task::yield_now().await;
        }
        let (state, no_snapshot): (String, bool) =
            sqlx::query_as("select state,xact_start is null from pg_stat_activity where pid=$1")
                .bind(pid)
                .fetch_one(&pool)
                .await
                .unwrap();
        while let Some(chunk) = receive.recv().await {
            compressed.extend_from_slice(&chunk.unwrap());
        }
        worker.await.unwrap().unwrap();
        assert_eq!(state, "idle", "statement spans paused blob sink");
        assert!(no_snapshot, "RR snapshot spans paused blob sink");
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(compressed.as_slice()));
        let paths: Vec<_> = archive
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().into_owned())
            .collect();
        assert_eq!(paths.last().unwrap().to_str(), Some("complete.json"));
        s.stop().await;
    }
}
