//! Notification hints plus a five-second catch-up poll; all candidates are re-read.
use chrono::{DateTime, Duration, Utc};
use porchlight_db::{
    derive,
    tx::{Actor, begin_as},
};
use sqlx::{Acquire, PgPool, postgres::PgListener};
use std::future::Future;

/// Process at most 50 unlocked candidates. Busy item rows are skipped, never job-first locked.
/// Terminal failures remain pending until an authoritative enqueue resets their attempts.
pub async fn run_batch(pool: &PgPool, now: DateTime<Utc>) -> anyhow::Result<usize> {
    let candidates: Vec<uuid::Uuid> = sqlx::query_scalar(
        "select item_id from porchlight.item_derivation
         where applied_gen<requested_gen and retry_at<=$1 and attempts<5
         order by retry_at,item_id limit 50",
    )
    .bind(now)
    .fetch_all(pool)
    .await?;
    let mut processed = 0;
    for item in candidates {
        match process_candidate(pool, item, now).await {
            Ok(true) => processed += 1,
            Ok(false) => {}
            // Includes acquisition/recheck/member-lock errors, not just derive errors.
            // Dropping the failed transaction rolls back ALL candidate locks/effects.
            Err(error) => {
                let category = error
                    .downcast_ref::<sqlx::Error>()
                    .map(|e| match e {
                        sqlx::Error::Database(db) if db.code().as_deref() == Some("55P03") => {
                            "lock_unavailable"
                        }
                        sqlx::Error::Database(db) if db.code().as_deref() == Some("40P01") => {
                            "deadlock"
                        }
                        sqlx::Error::Database(_) => "database",
                        _ => "connection",
                    })
                    .unwrap_or("acquisition");
                // Only expected 55P03 contention is quiet; unexpected acquisition
                // failures must remain visible without disclosing database messages.
                if category == "lock_unavailable" {
                    tracing::debug!(%item,category,"derivation candidate unavailable; continuing batch");
                } else {
                    tracing::warn!(%item,category,"derivation candidate acquisition failed; continuing batch");
                }
            }
        }
    }
    Ok(processed)
}
/// Explicit post-commit drain ignores backoff eligibility for freshly enqueued jobs.
/// All item/occurrence/member locks and failure bookkeeping still use the worker path.
pub async fn drain_candidate(
    pool: &PgPool,
    item: uuid::Uuid,
    now: DateTime<Utc>,
) -> anyhow::Result<bool> {
    process_job(pool, item, now, true).await
}
async fn process_candidate(
    pool: &PgPool,
    item: uuid::Uuid,
    now: DateTime<Utc>,
) -> anyhow::Result<bool> {
    process_job(pool, item, now, false).await
}
async fn process_job(
    pool: &PgPool,
    item: uuid::Uuid,
    now: DateTime<Utc>,
    immediate: bool,
) -> anyhow::Result<bool> {
    let mut tx = begin_as(pool, &Actor::system()).await?;
    let locked: Option<uuid::Uuid> =
        sqlx::query_scalar("select id from porchlight.items where id=$1 for update skip locked")
            .bind(item)
            .fetch_optional(&mut *tx)
            .await?;
    if locked.is_none() {
        tx.rollback().await?;
        return Ok(false);
    }
    // A raw occurrence-only writer may hold a child while the item is free.
    // NOWAIT rolls this candidate back immediately, never head-of-line blocks the batch.
    sqlx::query(
        "select id from porchlight.item_occurrences
         where item_id=$1 and status not in ('done','skipped')
         order by id for update nowait",
    )
    .bind(item)
    .fetch_all(&mut *tx)
    .await?;
    sqlx::query(
        "select item_id from porchlight.item_derivation where item_id=$1 for update nowait",
    )
    .bind(item)
    .fetch_all(&mut *tx)
    .await?;
    let state: Option<(i64, i64, i32)> = sqlx::query_as(
        "select requested_gen,applied_gen,attempts
         from porchlight.item_derivation where item_id=$1 and applied_gen<requested_gen
         and ($3 or retry_at<=$2) and attempts<5",
    )
    .bind(item)
    .bind(now)
    .bind(immediate)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((seen, applied, attempts)) = state else {
        tx.rollback().await?;
        return Ok(false);
    };
    let members = derive::affected_members(&mut tx, item).await?;
    sqlx::query("select id from porchlight.household_members where id=any($1) order by id for no key update nowait").bind(&members).fetch_all(&mut *tx).await?;
    let mut savepoint = tx.begin().await?;
    let result = async {
        let outcome = derive::derive_item_prelocked(&mut savepoint, item, now).await?;
        porchlight_db::planning::recompute_load_dates(
            &mut savepoint,
            &members,
            &outcome.load_dates,
            now,
        )
        .await?;
        Ok::<_, derive::Error>(outcome)
    }
    .await;
    match result {
        Ok(_) => savepoint.commit().await?,
        Err(error) => {
            savepoint.rollback().await?;
            let next = attempts + 1;
            let delay = Duration::seconds((5_i64 * (1_i64 << next.min(20))).min(900));
            let message = safe_error(&error);
            tracing::warn!(%item, attempt=next, error=%message, "derivation failed; backing off");
            sqlx::query(
                "update porchlight.item_derivation
                 set attempts=$3,retry_at=$4,last_error=$5,updated_at=$6
                 where item_id=$1 and requested_gen=$2",
            )
            .bind(item)
            .bind(seen)
            .bind(next)
            .bind(now + delay)
            .bind(message)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            if next >= 5 {
                sqlx::query("select porchlight.pl_emit_derived($1,$2,true)")
                    .bind(item)
                    .bind(applied)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(true)
}

fn safe_error(error: &derive::Error) -> String {
    // Database Display can contain row values/credentials; never persist or log it.
    match error {
        derive::Error::Sql(sqlx::Error::Database(db)) => format!(
            "database derivation failure (SQLSTATE {})",
            db.code().as_deref().unwrap_or("unknown")
        ),
        derive::Error::Sql(_) => "database derivation connection failure".into(),
        _ => error.to_string().chars().take(2000).collect(),
    }
}
/// Cancel an in-flight transaction on shutdown. Polling remains available if LISTEN fails.
pub async fn run(pool: PgPool, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    run_with_clock(pool, std::sync::Arc::new(Utc::now), shutdown).await
}
/// The API and post-commit drains share AppState's one domain clock.
pub async fn run_with_clock(
    pool: PgPool,
    clock: std::sync::Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    tokio::pin!(shutdown);
    let listeners = crate::db::listener_pool(&pool, "porchlight-derive-listener");
    let mut listener: Option<PgListener> = None;
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            ()=shutdown.as_mut()=>return Ok(()),
            _=tick.tick()=>{},
            result=async {match &mut listener {Some(l)=>l.try_recv().await.map(|_|()),None=>std::future::pending().await}}=>{
                if result.is_err(){listener=None;tracing::warn!("derivation listener disconnected; polling until reconnect");}
            }
        }
        if listener.is_none() {
            listener = tokio::select! {()=shutdown.as_mut()=>return Ok(()),r=async {let mut l=PgListener::connect_with(&listeners).await?;l.listen("porchlight_derive").await?;Ok::<_,sqlx::Error>(l)}=>r.ok()};
        }
        // s8b: availability-only raw SQL has an independent member queue.
        tokio::select! {()=shutdown.as_mut()=>return Ok(()),r=crate::planning::run_load_batch(&pool,clock())=>{if let Err(error)=r {
            let sqlstate=error.downcast_ref::<sqlx::Error>().and_then(|e|match e {sqlx::Error::Database(db)=>db.code().map(|c|c.into_owned()),_=>None});
            tracing::warn!(category="load_batch",sqlstate=sqlstate.as_deref().unwrap_or("none"), "load batch deferred")
        }}}
        loop {
            let result = tokio::select! {()=shutdown.as_mut()=>return Ok(()),result=run_batch(&pool,clock())=>result};
            match result {
                Ok(50) => tokio::task::yield_now().await, // Drain full batches without waiting for another hint.
                Ok(_) => break,
                Err(_) => {
                    tracing::warn!("derivation batch failed; retrying next tick");
                    break;
                }
            }
        }
    }
}
