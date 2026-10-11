use chrono::{DateTime, Duration, Utc};
use porchlight_api::{
    derive_worker,
    scheduler::{Clock, Scheduler},
};
use porchlight_db::{
    derive,
    testing::{Role, insert_appointment, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
use std::sync::{Arc, Mutex};
struct TestClock(Mutex<DateTime<Utc>>);
impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}
fn now() -> DateTime<Utc> {
    "2026-10-05T12:00:00Z".parse().unwrap()
}
async fn apply(app: &PgPool, id: uuid::Uuid, at: DateTime<Utc>) {
    let mut tx = begin_as(app, &Actor::system()).await.unwrap();
    sqlx::query("select id from porchlight.items where id=$1 for update")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    derive::derive_item(&mut tx, id, at).await.unwrap();
    tx.commit().await.unwrap();
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn scheduler_isolates_bad_override_and_releases_each_item(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let bad = insert_appointment(&pool, &h).await.unwrap();
    let good = insert_appointment(&pool, &h).await.unwrap();
    for id in [bad, good] {
        apply(&app, id, now() - Duration::days(10)).await;
    }
    sqlx::query("update porchlight.item_occurrences set override_local_start='2026-10-14 14:00' where item_id=$1").bind(bad).execute(&pool).await.unwrap();
    let clock = Arc::new(TestClock(Mutex::new(now() + Duration::days(20))));
    let s = Scheduler::derivation(app.clone(), clock.clone())
        .await
        .unwrap();
    assert_eq!(s.tick().await.unwrap(), 8); // Includes calendar, planning and notification jobs.
    let state:(Option<DateTime<Utc>>,Option<String>)=sqlx::query_as("select last_success_at,last_error from porchlight.scheduler_jobs where name='refresh_dispositions'").fetch_one(&pool).await.unwrap();
    assert!(
        state.0.is_some() && state.1.is_none(),
        "one bad override must not fail the sweep"
    );
    let pending: bool = sqlx::query_scalar(
        "select applied_gen<requested_gen from porchlight.item_derivation where item_id=$1",
    )
    .bind(good)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(pending, "healthy item must enqueue");
    sqlx::query("update porchlight.item_derivation set retry_at=$1")
        .bind(clock.now())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        derive_worker::run_batch(&app, clock.now()).await.unwrap(),
        2
    );
    let failure: (i32, Option<String>) = sqlx::query_as(
        "select attempts,last_error from porchlight.item_derivation where item_id=$1",
    )
    .bind(bad)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(failure.0, 1);
    assert!(failure.1.is_some());
    let healthy: bool = sqlx::query_scalar(
        "select applied_gen=requested_gen from porchlight.item_derivation where item_id=$1",
    )
    .bind(good)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(healthy);
    // Scheduler must not reset worker poison backoff on every sweep.
    sqlx::query("update porchlight.item_derivation set attempts=5,last_error='terminal',retry_at=$2 where item_id=$1").bind(bad).bind(now()+Duration::days(30)).execute(&pool).await.unwrap();
    *clock.0.lock().unwrap() = now() + Duration::days(21);
    s.tick().await.unwrap();
    let attempts: i32 =
        sqlx::query_scalar("select attempts from porchlight.item_derivation where item_id=$1")
            .bind(bad)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(attempts, 5);
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn horizon_filters_finished_and_already_covered_series(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let done = insert_appointment(&pool, &h).await.unwrap();
    let daily = insert_appointment(&pool, &h).await.unwrap();
    for (id, rule) in [(done, "FREQ=DAILY;COUNT=2"), (daily, "FREQ=DAILY")] {
        sqlx::query("update porchlight.items set recurrence_mode='calendar',rrule=$2 where id=$1")
            .bind(id)
            .bind(rule)
            .execute(&pool)
            .await
            .unwrap();
        apply(&app, id, now()).await;
    }
    let before: Vec<(uuid::Uuid, i64)> = sqlx::query_as(
        "select item_id,requested_gen from porchlight.item_derivation order by item_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let clock = Arc::new(TestClock(Mutex::new(now())));
    let s = Scheduler::derivation(app.clone(), clock.clone())
        .await
        .unwrap();
    s.tick().await.unwrap();
    let after: Vec<(uuid::Uuid, i64)> = sqlx::query_as(
        "select item_id,requested_gen from porchlight.item_derivation order by item_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(after, before, "no vacuous horizon re-enqueue");
    *clock.0.lock().unwrap() = now() + Duration::days(1);
    s.tick().await.unwrap();
    let generation: i64 =
        sqlx::query_scalar("select requested_gen from porchlight.item_derivation where item_id=$1")
            .bind(done)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        generation,
        before.iter().find(|(id, _)| *id == done).unwrap().1
    );
    let generation: i64 =
        sqlx::query_scalar("select requested_gen from porchlight.item_derivation where item_id=$1")
            .bind(daily)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        generation,
        before.iter().find(|(id, _)| *id == daily).unwrap().1 + 1
    );
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn startup_without_household_and_contended_occurrence_batch(pool: PgPool) {
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    assert!(
        Scheduler::derivation(app.clone(), Arc::new(TestClock(Mutex::new(now()))))
            .await
            .is_ok(),
        "empty startup must work"
    );
    let h = seed_household(&pool).await.unwrap();
    let blocked = insert_appointment(&pool, &h).await.unwrap();
    let good = insert_appointment(&pool, &h).await.unwrap();
    for id in [blocked, good] {
        apply(&app, id, now()).await;
    }
    sqlx::query("update porchlight.items set buffer_before_min=1")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update porchlight.item_derivation set retry_at=$1")
        .bind(now())
        .execute(&pool)
        .await
        .unwrap();
    let mut held = pool.begin().await.unwrap();
    sqlx::query("select id from porchlight.item_occurrences where item_id=$1 for update")
        .bind(blocked)
        .fetch_all(&mut *held)
        .await
        .unwrap();
    // The worker's production lock limits (the contended candidate still fails at once: NOWAIT).
    // A shorter lock_timeout would also bound the good candidate's COMMIT, which waits for
    // PostgreSQL's cluster-wide NOTIFY lock behind concurrently running tests' outbox commits.
    let limited = sqlx::postgres::PgPoolOptions::new()
        .connect_with(
            app.connect_options()
                .as_ref()
                .clone()
                .options(porchlight_api::db::APP_SETTINGS.iter().copied()),
        )
        .await
        .unwrap();
    let holder: i32 = sqlx::query_scalar("select pg_backend_pid()")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let bound = std::time::Duration::from_secs(15);
    let batch = tokio::time::timeout(bound, derive_worker::run_batch(&limited, now()));
    let count = tokio::select! {
        result = batch => result.expect("batch must terminate").unwrap(),
        blocked = porchlight_api::testing::wait_blocked_behind(&pool, holder, 1, bound) => {
            blocked.expect("batch must terminate before watchdog");
            panic!("contended occurrence waited instead of NOWAIT");
        }
    };
    assert_eq!(
        count, 1,
        "contended occurrence must not abort remaining candidates"
    );
    let pending: bool = sqlx::query_scalar(
        "select applied_gen<requested_gen from porchlight.item_derivation where item_id=$1",
    )
    .bind(good)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!pending);
    held.rollback().await.unwrap();
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn daily_statement_measurement(pool: PgPool) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tracing::Instrument;
    use tracing_subscriber::{Layer, layer::SubscriberExt};
    struct Counter(Arc<AtomicUsize>);
    impl<S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>> Layer<S>
        for Counter
    {
        fn on_event(&self, e: &tracing::Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
            if e.metadata().target() == "sqlx::query"
                && ctx.event_scope(e).is_some_and(|scope| {
                    scope
                        .from_root()
                        .any(|span| span.name() == "measured_derive")
                })
            {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    let h = seed_household(&pool).await.unwrap();
    use sqlx::ConnectOptions;
    let options = porchlight_db::testing::connect_options_as(&pool, Role::App, None)
        .unwrap()
        .log_statements(tracing::log::LevelFilter::Debug);
    let app = sqlx::postgres::PgPoolOptions::new()
        .connect_with(options)
        .await
        .unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query(
        "update porchlight.items set recurrence_mode='calendar',rrule='FREQ=DAILY' where id=$1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    for member in [h.super_admin, h.dependent, h.user] {
        sqlx::query("insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)").bind(h.id).bind(id).bind(member).execute(&pool).await.unwrap();
    }
    for offset in [15, 30] {
        sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,$3)").bind(h.id).bind(id).bind(offset).execute(&pool).await.unwrap();
    }
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    sqlx::query("select id from porchlight.items where id=$1 for update")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry().with(Counter(count.clone()));
    tracing::subscriber::set_global_default(subscriber).unwrap();
    derive::derive_item(&mut tx, id, now())
        .instrument(tracing::info_span!("measured_derive"))
        .await
        .unwrap();
    let measured = count.load(Ordering::SeqCst);
    println!("daily derive statements={measured}");
    assert!(
        measured > 0 && measured <= 30,
        "derivation must batch statements, measured {measured}"
    );
    let counts:(i64,i64,i64)=sqlx::query_as("select (select count(*) from porchlight.item_occurrences where item_id=$1),(select count(*) from porchlight.member_agenda where item_id=$1),(select count(*) from porchlight.scheduled_reminders where item_id=$1)").bind(id).fetch_one(&mut *tx).await.unwrap();
    assert!(counts.0 > 500);
    assert_eq!(counts.1, counts.0 * 3);
    assert_eq!(counts.2, counts.0 * 6);
    tx.rollback().await.unwrap();
    let task: uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,time_mode,due_date,est_minutes,recurrence_mode,rrule) values($1,$2,'Daily estimated chore','todo','floating','2026-10-06',90,'calendar','FREQ=DAILY') returning id").bind(h.id).bind(h.super_admin).fetch_one(&pool).await.unwrap();
    for member in [h.super_admin, h.dependent, h.user] {
        sqlx::query("insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)").bind(h.id).bind(task).bind(member).execute(&pool).await.unwrap();
        sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,0,'09:00','10:00')").bind(h.id).bind(member).execute(&pool).await.unwrap();
    }
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    count.store(0, Ordering::SeqCst);
    derive::derive_item(&mut tx, task, now())
        .instrument(tracing::info_span!("measured_derive"))
        .await
        .unwrap();
    let measured = count.load(Ordering::SeqCst);
    println!(
        "daily estimated task derive statements={measured} (batch notification seam included)"
    );
    assert!(
        measured > 0 && measured <= 30,
        "task derive statement cap exceeded: {measured}"
    );
    let rows: i64 = sqlx::query_scalar("select count(*) from porchlight.member_daily_load")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(rows > 1500);
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn builtin_scheduler_with_single_connection_does_not_starve(pool: PgPool) {
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(porchlight_db::testing::connect_options_as(&pool, Role::App, None).unwrap())
        .await
        .unwrap();
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    apply(&app, id, now() - Duration::days(20)).await;
    let s = Scheduler::derivation(
        app,
        Arc::new(TestClock(Mutex::new(now() + Duration::days(20)))),
    )
    .await
    .unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(1), s.tick())
            .await
            .expect("job election must not starve its independent item transactions")
            .unwrap(),
        8 // Includes retention/staleness, calendar, planning and notification jobs.
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn sweep_releases_first_item_while_later_item_is_running(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let first = insert_appointment(&pool, &h).await.unwrap();
    let last = insert_appointment(&pool, &h).await.unwrap();
    for id in [first, last] {
        apply(&app, id, now() - Duration::days(20)).await;
    }
    // Pause only the second candidate's enqueue. An observer must be able to lock the first.
    let ddl = format!(
        "create function porchlight.test_sweep_delay() returns trigger language plpgsql as $$ begin if new.item_id='{}'::uuid and new.requested_gen>old.requested_gen then perform pg_sleep(0.8); end if; return new; end; $$; create trigger test_sweep_delay before update on porchlight.item_derivation for each row execute function porchlight.test_sweep_delay();",
        last
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(ddl))
        .execute(&pool)
        .await
        .unwrap();
    let s = Scheduler::derivation(
        app,
        Arc::new(TestClock(Mutex::new(now() + Duration::days(20)))),
    )
    .await
    .unwrap();
    let running = tokio::spawn(async move { s.tick().await });
    tokio::time::timeout(std::time::Duration::from_secs(2),async{loop {let sleeping:bool=sqlx::query_scalar("select exists(select 1 from pg_stat_activity where datname=current_database() and wait_event='PgSleep')").fetch_one(&pool).await.unwrap();if sleeping{break;}tokio::time::sleep(std::time::Duration::from_millis(5)).await;}}).await.unwrap();
    let mut observer = pool.begin().await.unwrap();
    sqlx::query("select id from porchlight.items where id=$1 for no key update nowait")
        .bind(first)
        .fetch_one(&mut *observer)
        .await
        .expect("sweep retained an earlier item lock");
    observer.rollback().await.unwrap();
    assert_eq!(running.await.unwrap().unwrap(), 8); // Includes calendar, planning and notification jobs.
}
