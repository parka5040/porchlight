use porchlight_api::scheduler::{CallbackClock, Job, Schedule, Scheduler};
use porchlight_db::testing::{Role, pool_as};
use sqlx::PgPool;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn idle_election_retains_exclusivity_and_persists_success(pool: PgPool) {
    let role = pool_as(&pool, Role::App, None).await.unwrap();
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect_with(
            (*role.connect_options())
                .clone()
                .options(porchlight_api::db::APP_SETTINGS.iter().copied())
                .options([("idle_in_transaction_session_timeout", "100ms")]),
        )
        .await
        .unwrap();
    let now = "2026-11-13T12:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let clock = Arc::new(CallbackClock(Arc::new(move || now)));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let job = Job::new(
        "idle-election",
        Schedule::Every(Duration::from_secs(300)),
        {
            let (entered, release, calls) = (entered.clone(), release.clone(), calls.clone());
            move |_, _| {
                let (entered, release, calls) = (entered.clone(), release.clone(), calls.clone());
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    entered.notify_one();
                    release.notified().await;
                    Ok(())
                })
            }
        },
    );
    let first = Arc::new(Scheduler::new(app.clone(), clock.clone(), vec![job.clone()]).unwrap());
    let second = Scheduler::new(app.clone(), clock, vec![job]).unwrap();
    let runner = tokio::spawn({
        let first = first.clone();
        async move { first.tick().await }
    });
    entered.notified().await;
    tokio::time::sleep(Duration::from_millis(350)).await;
    let competing = tokio::time::timeout(Duration::from_millis(150), second.tick()).await;
    release.notify_waiters();
    let result = runner.await.unwrap();
    assert!(
        matches!(competing, Ok(Ok(0))),
        "election lock lost during callback: {competing:?}"
    );
    assert_eq!(result.unwrap(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(first.tick().await.unwrap(), 0);
    let success: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT last_success_at FROM porchlight.scheduler_jobs WHERE name='idle-election'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(success, Some(now));
    // The worker exemption must never leak back into request-serving connections.
    let timeout: String = sqlx::query_scalar("SHOW idle_in_transaction_session_timeout")
        .fetch_one(&app)
        .await
        .unwrap();
    assert_eq!(timeout, "100ms");
}
