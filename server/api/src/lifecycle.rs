//! Shared production/test composition: no HTTP serving until trust listeners and cleanup are ready.
use crate::state::AppState;
use std::{future::Future, time::Duration};

pub async fn purge(pool: &sqlx::PgPool) -> sqlx::Result<()> {
    let count = porchlight_db::idempotency::purge_expired(pool).await?;
    tracing::info!(count, "expired idempotency responses purged");
    Ok(())
}
async fn maintenance(pool: sqlx::PgPool, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = shutdown.as_mut() => return Ok(()),
            () = tokio::time::sleep(Duration::from_secs(3600)) => {}
        }
        if purge(&pool).await.is_err() {
            tracing::warn!("idempotency purge failed; retrying next hour");
        }
        crate::admin_recovery::warn(&pool).await;
    }
}
pub async fn run(
    state: AppState,
    shutdown: impl Future<Output = ()> + Clone,
    watch_revocations: bool,
    #[cfg(feature = "testing")] fail_seed_once: bool,
    serve: impl Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<()> {
    let (trust_ready, trust_rx) = tokio::sync::oneshot::channel();
    let (hub_ready, hub_rx) = tokio::sync::oneshot::channel();
    let startup = async {
        let ready = async {
            trust_rx.await?;
            hub_rx.await?;
            if purge(&state.pool).await.is_err() {
                tracing::warn!("idempotency startup purge failed; retrying next hour");
            }
            crate::admin_recovery::warn(&state.pool).await;
            Ok::<_, anyhow::Error>(())
        };
        // Watchers drop their ready senders on shutdown. Cancellation wins over that expected close.
        tokio::select! { biased; () = shutdown.clone() => return Ok(()), result = ready => result? }
        tokio::try_join!(serve, async {
            crate::release::initialize(&state).await;
            Ok::<_, anyhow::Error>(())
        })?;
        Ok(())
    };
    tokio::try_join!(
        startup,
        state
            .hub
            .run_ready(state.pool.clone(), shutdown.clone(), Some(hub_ready)),
        async {
            if watch_revocations {
                crate::revocation::run_ready(
                    state.pool.clone(),
                    state.revoked.clone(),
                    state.revocations.clone(),
                    shutdown.clone(),
                    trust_ready,
                    #[cfg(feature = "testing")]
                    fail_seed_once,
                )
                .await
            } else {
                let _ = trust_ready.send(());
                Ok(())
            }
        },
        crate::projector::run(state.pool.clone(), shutdown.clone()),
        async {
            #[cfg(feature = "testing")]
            if !state.background_derivation {
                return Ok(());
            }
            crate::derive_worker::run_with_clock(
                state.pool.clone(),
                state.item_clock.clone(),
                shutdown.clone(),
            )
            .await
        },
        async {
            #[cfg(feature = "testing")]
            if !state.background_derivation {
                return Ok(());
            }
            let scheduler = crate::scheduler::Scheduler::from_state(
                &state,
                std::sync::Arc::new(crate::scheduler::CallbackClock(state.item_clock.clone())),
            )
            .await?;
            scheduler.run(shutdown.clone()).await
        },
        async {
            #[cfg(feature = "testing")]
            if !state.background_derivation {
                return Ok(());
            }
            crate::notifier::run(
                state.pool.clone(),
                state.item_clock.clone(),
                shutdown.clone(),
            )
            .await
        },
        maintenance(state.pool.clone(), shutdown.clone()),
        state.release_watch.run(shutdown.clone()),
    )?;
    Ok(())
}

/// Shared listener/watcher/projector/maintenance composition for production and tests.
pub async fn serve_all(
    state: AppState,
    listeners: crate::server::Listeners,
    policy: crate::server::NetPolicy,
    shutdown: impl Future<Output = ()> + Clone,
    watch_revocations: bool,
    #[cfg(feature = "testing")] fail_seed_once: bool,
) -> anyhow::Result<()> {
    run(
        state.clone(),
        shutdown.clone(),
        watch_revocations,
        #[cfg(feature = "testing")]
        fail_seed_once,
        crate::server::run(state, listeners, policy, shutdown),
    )
    .await
}
