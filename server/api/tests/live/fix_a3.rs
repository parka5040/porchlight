use chrono::{DateTime, Utc};
use porchlight_api::derive_worker;
use porchlight_db::testing::{Role, insert_appointment, pool_as, seed_household};
use sqlx::PgPool;
use std::sync::{Arc, Mutex};
use tracing::{Level, instrument::WithSubscriber};
use tracing_subscriber::{Layer, layer::SubscriberExt};

#[derive(Clone)]
struct Capture(Arc<Mutex<Vec<(Level, String)>>>);
impl<S: tracing::Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() == "porchlight_api::derive_worker" {
            struct Fields(String);
            impl tracing::field::Visit for Fields {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    self.0.push_str(&format!(" {}={value:?}", field.name()));
                }
            }
            let mut fields = Fields(String::new());
            event.record(&mut fields);
            self.0
                .lock()
                .unwrap()
                .push((*event.metadata().level(), fields.0));
        }
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn only_lock_unavailable_is_debug_other_acquisition_errors_warn_redacted(pool: PgPool) {
    let now: DateTime<Utc> = "2026-10-05T12:00:00Z".parse().unwrap();
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query(
        "insert into porchlight.item_occurrences(household_id,item_id,seq) values($1,$2,0)",
    )
    .bind(h.id)
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("update porchlight.item_derivation set retry_at=$1")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let logs = Capture(Arc::new(Mutex::new(Vec::new())));
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(logs.clone()));
    let mut held = pool.begin().await.unwrap();
    sqlx::query("select id from porchlight.item_occurrences where item_id=$1 for update")
        .bind(id)
        .fetch_all(&mut *held)
        .await
        .unwrap();
    assert_eq!(
        derive_worker::run_batch(&app, now)
            .with_subscriber(dispatch.clone())
            .await
            .unwrap(),
        0
    );
    held.rollback().await.unwrap();
    let events = logs.0.lock().unwrap().clone();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, Level::DEBUG);
    assert!(events[0].1.contains("lock_unavailable"));
    logs.0.lock().unwrap().clear();
    // Force a non-contention acquisition failure without logging PostgreSQL's message.
    sqlx::query("revoke select on porchlight.household_members from porchlight_app")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        derive_worker::run_batch(&app, now)
            .with_subscriber(dispatch)
            .await
            .unwrap(),
        0
    );
    let events = logs.0.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, Level::WARN);
    assert!(events[0].1.contains("database"));
    assert!(!events[0].1.contains("permission") && !events[0].1.contains("household_members"));
}
