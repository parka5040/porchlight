//! Monotonic revocation propagation. NOTIFY payloads are hints only; a poll repairs missed notifications.
use crate::{state::AppState, tls::RevokedSet};
use sqlx::{PgPool, postgres::PgListener};
use std::{collections::HashSet, future::Future, time::Duration};
use tokio::{
    sync::broadcast,
    time::{Instant, MissedTickBehavior, interval_at, sleep},
};
use uuid::Uuid;
/// Called only after the revoking transaction commits. Extend before notifying open streams.
pub fn publish(state: &AppState, id: Uuid, fingerprints: Vec<[u8; 32]>) {
    state.revoked.extend(fingerprints);
    if state.revoked.claim_device(id) {
        let _ = state.revocations.send(id);
    }
}
/// Public catch-up hook for startup, reconnect and testing. Never replaces the local set.
pub async fn reload(
    pool: &PgPool,
    revoked: &RevokedSet,
    known: &mut HashSet<Uuid>,
    events: &broadcast::Sender<Uuid>,
) -> sqlx::Result<()> {
    // IDs first: every id read here is committed before the fingerprint query, so a broadcast
    // cannot precede adding its fingerprints (even if another revoke commits between the reads).
    let ids = porchlight_db::devices::revoked_device_ids(pool).await?;
    let fps = porchlight_db::devices::revoked_fingerprints(pool).await?;
    revoked.extend(fps);
    for id in ids {
        if known.insert(id) && revoked.claim_device(id) {
            let _ = events.send(id);
        }
    }
    Ok(())
}
async fn connect(pool: &PgPool) -> sqlx::Result<PgListener> {
    let mut l = PgListener::connect_with(pool).await?;
    l.listen("porchlight_revocation").await?;
    Ok(l)
}
pub async fn run(
    pool: PgPool,
    revoked: RevokedSet,
    events: broadcast::Sender<Uuid>,
    shutdown: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    run_inner(
        pool,
        revoked,
        events,
        shutdown,
        std::future::ready(()),
        None,
        #[cfg(feature = "testing")]
        false,
    )
    .await
}

/// Rendezvous after startup history, before LISTEN, for deterministic startup-gap tests.
#[cfg(feature = "testing")]
pub async fn run_after_seed(
    pool: PgPool,
    revoked: RevokedSet,
    events: broadcast::Sender<Uuid>,
    shutdown: impl Future<Output = ()>,
    after_seed: impl Future<Output = ()>,
) -> anyhow::Result<()> {
    run_inner(
        pool,
        revoked,
        events,
        shutdown,
        after_seed,
        None,
        #[cfg(feature = "testing")]
        false,
    )
    .await
}

/// Serving waits for history, LISTEN, and the first broadcasting catch-up.
pub async fn run_ready(
    pool: PgPool,
    revoked: RevokedSet,
    events: broadcast::Sender<Uuid>,
    shutdown: impl Future<Output = ()>,
    ready: tokio::sync::oneshot::Sender<()>,
    #[cfg(feature = "testing")] fail_seed_once: bool,
) -> anyhow::Result<()> {
    run_inner(
        pool,
        revoked,
        events,
        shutdown,
        std::future::ready(()),
        Some(ready),
        #[cfg(feature = "testing")]
        fail_seed_once,
    )
    .await
}

async fn run_inner(
    pool: PgPool,
    revoked: RevokedSet,
    events: broadcast::Sender<Uuid>,
    shutdown: impl Future<Output = ()>,
    after_seed: impl Future<Output = ()>,
    mut ready: Option<tokio::sync::oneshot::Sender<()>>,
    #[cfg(feature = "testing")] mut fail_seed_once: bool,
) -> anyhow::Result<()> {
    tokio::pin!(shutdown);
    let listeners = crate::db::listener_pool(&pool, "porchlight-revocation-listener");
    let mut known = HashSet::new();
    let mut backoff = Duration::from_secs(1);
    // Seed history before LISTEN. Every later catch-up broadcasts, including the first:
    // revocations committed between this read and LISTEN must not become silent history.
    loop {
        let seed = tokio::select! { ()=shutdown.as_mut()=>return Ok(()), r=async {
            #[cfg(feature = "testing")]
            if std::mem::take(&mut fail_seed_once) { return Err(sqlx::Error::RowNotFound); }
            let ids = porchlight_db::devices::revoked_device_ids(&pool).await?;
            let fps = porchlight_db::devices::revoked_fingerprints(&pool).await?;
            revoked.extend(fps);
            // Do not claim publication: concurrent local revokes still publish immediately.
            known.extend(ids);
            Ok::<_, sqlx::Error>(())
        }=>r };
        if seed.is_ok() {
            break;
        }
        tracing::warn!(?backoff, "revocation history load retrying");
        tokio::select! { ()=shutdown.as_mut()=>return Ok(()), ()=sleep(backoff)=>{} }
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
    tokio::select! { ()=shutdown.as_mut()=>return Ok(()), ()=after_seed=>{} }
    loop {
        let connected =
            tokio::select! {()=shutdown.as_mut()=>return Ok(()),r=connect(&listeners)=>r};
        if let Ok(mut listener) = connected {
            let initial = tokio::select! {()=shutdown.as_mut()=>return Ok(()),r=reload(&pool,&revoked,&mut known,&events)=>r};
            if initial.is_ok() {
                if let Some(ready) = ready.take() {
                    let _ = ready.send(());
                }
                backoff = Duration::from_secs(1);
                // The first poll is thirty seconds AFTER catch-up, never an immediate second load.
                let period = Duration::from_secs(30);
                let mut poll = interval_at(Instant::now() + period, period);
                poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
                loop {
                    let hint = tokio::select! {()=shutdown.as_mut()=>return Ok(()),_=poll.tick()=>Ok(()),r=listener.try_recv()=>r.map(|_|())};
                    if hint.is_err() {
                        break;
                    }
                    let loaded = tokio::select! {()=shutdown.as_mut()=>return Ok(()),r=reload(&pool,&revoked,&mut known,&events)=>r};
                    if loaded.is_err() {
                        break;
                    }
                    backoff = Duration::from_secs(1);
                }
            }
        }
        // Errors carry no SQL, credentials, URLs or notification payloads into logs.
        tracing::warn!(?backoff, "revocation watcher reconnecting");
        tokio::select! {()=shutdown.as_mut()=>return Ok(()),()=sleep(backoff)=>{}}
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}
