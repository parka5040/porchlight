//! Outbox projector: assigns stream sequence numbers to committed outbox rows and announces them.
//!
//! Assignment is safe for concurrent callers (see `porchlight_db::outbox::assign_stream_seqs`), so several
//! projectors running at once are harmless; no election is needed.

use porchlight_db::outbox::assign_stream_seqs;
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::time::{MissedTickBehavior, interval, sleep};

const BATCH: i64 = 500;
const TICK: Duration = Duration::from_secs(5);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Runs the projector until `shutdown` completes.
pub async fn run(pool: PgPool, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    tokio::pin!(shutdown);
    let listeners = crate::db::listener_pool(&pool, "porchlight-outbox-listener");
    let mut ticker = interval(TICK);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut backoff = BACKOFF_MIN;
    'reconnect: loop {
        let mut listener = match connect_listener(&listeners).await {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, ?backoff, "outbox listener connect failed");
                tokio::select! {
                    () = shutdown.as_mut() => return Ok(()),
                    () = sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(BACKOFF_MAX);
                continue 'reconnect;
            }
        };
        // Catch-up after (re)connecting: notifications sent while disconnected are lost.
        if !drain_retry(&pool, shutdown.as_mut()).await {
            return Ok(());
        }
        loop {
            tokio::select! {
                () = shutdown.as_mut() => return Ok(()),
                _ = ticker.tick() => {
                    if !drain_retry(&pool, shutdown.as_mut()).await {
                        return Ok(());
                    }
                }
                n = listener.try_recv() => match n {
                    // Ok(None): connection was lost and re-established; catch up either way.
                    Ok(_) => {
                        backoff = BACKOFF_MIN;
                        if !drain_retry(&pool, shutdown.as_mut()).await {
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, ?backoff, "outbox listener failed; reconnecting");
                        tokio::select! {
                            () = shutdown.as_mut() => return Ok(()),
                            () = sleep(backoff) => {}
                        }
                        backoff = (backoff * 2).min(BACKOFF_MAX);
                        continue 'reconnect;
                    }
                },
            }
        }
    }
}

async fn connect_listener(pool: &PgPool) -> Result<PgListener, sqlx::Error> {
    let mut listener = PgListener::connect_with(pool).await?;
    listener.listen("porchlight_outbox").await?;
    Ok(listener)
}

/// Assigns seqs batch by batch until nothing is pending (each batch announces itself in its own transaction).
/// Each call races `shutdown`: dropping an in-flight call abandons its query and the transaction rolls back.
/// Returns Ok(false) if shutdown fired.
async fn drain(
    pool: &PgPool,
    mut shutdown: Pin<&mut impl Future<Output = ()>>,
) -> Result<bool, sqlx::Error> {
    loop {
        let assigned = tokio::select! {
            () = shutdown.as_mut() => return Ok(false),
            r = assign_stream_seqs(pool, BATCH) => r?,
        };
        match assigned {
            Some(max) => tracing::info!(stream_seq = max, "projected outbox batch"),
            None => return Ok(true),
        }
    }
}

/// Like [`drain`] but retries transient errors with backoff (1 s to 30 s). Returns false if shutdown fired.
async fn drain_retry(pool: &PgPool, mut shutdown: Pin<&mut impl Future<Output = ()>>) -> bool {
    let mut backoff = BACKOFF_MIN;
    loop {
        match drain(pool, shutdown.as_mut()).await {
            Ok(done) => return done,
            Err(e) => {
                tracing::warn!(error = %e, ?backoff, "assigning stream seqs failed; retrying")
            }
        }
        tokio::select! {
            () = shutdown.as_mut() => return false,
            () = sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}
