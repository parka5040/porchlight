use chrono::{DateTime, Duration, NaiveTime, Utc};
use porchlight_api::scheduler::{Clock, Job, Schedule, Scheduler};
use porchlight_db::testing::{Role, insert_appointment, pool_as, seed_household};
use sqlx::PgPool;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
struct TestClock(Mutex<DateTime<Utc>>);
impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}
fn now() -> DateTime<Utc> {
    "2026-10-05T12:00:00Z".parse().unwrap()
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn persisted_catchup_single_runner_and_retry(pool: PgPool) {
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let clock = Arc::new(TestClock(Mutex::new(now())));
    let calls = Arc::new(AtomicUsize::new(0));
    let counts = calls.clone();
    let job = Job::new(
        "test",
        Schedule::Every(std::time::Duration::from_secs(300)),
        move |_, _| {
            let counts = counts.clone();
            Box::pin(async move {
                counts.fetch_add(1, Ordering::SeqCst);
                tokio::task::yield_now().await;
                Ok(())
            })
        },
    );
    let s = Scheduler::new(app.clone(), clock.clone(), vec![job]).unwrap();
    let (a, b) = tokio::join!(s.tick(), s.tick());
    assert!(a.is_ok() && b.is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(s.tick().await.unwrap(), 0);
    *clock.0.lock().unwrap() = now() + Duration::days(2);
    assert_eq!(s.tick().await.unwrap(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(s.tick().await.unwrap(), 0);
    let failed = Job::new(
        "failed",
        Schedule::Every(std::time::Duration::from_secs(60)),
        |conn, _| {
            Box::pin(async move {
                sqlx::query("select 1/0").execute(conn).await?;
                Ok(())
            })
        },
    );
    let f = Scheduler::new(app.clone(), clock.clone(), vec![failed]).unwrap();
    assert_eq!(f.tick().await.unwrap(), 1);
    assert_eq!(f.tick().await.unwrap(), 0);
    *clock.0.lock().unwrap() += Duration::seconds(10);
    assert_eq!(f.tick().await.unwrap(), 1);
    assert_eq!(f.tick().await.unwrap(), 0);
    let error: Option<String> =
        sqlx::query_scalar("select last_error from porchlight.scheduler_jobs where name='failed'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(error.is_some());
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn builtins_leave_pending_generations_unchanged_and_daily_dst_schedule(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query(
        "update porchlight.items set recurrence_mode='calendar',rrule='FREQ=DAILY' where id=$1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let s = Scheduler::derivation(app, Arc::new(TestClock(Mutex::new(now()))))
        .await
        .unwrap();
    assert_eq!(s.tick().await.unwrap(), 8); // Horizon, dispositions, calendar, planning and both notification jobs (blob GC is added by from_state).
    let requested: i64 =
        sqlx::query_scalar("select requested_gen from porchlight.item_derivation where item_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(requested, 2); // INSERT + recurrence edit; already-pending items are never re-enqueued.
    let schedule = Schedule::DailyAt(
        NaiveTime::from_hms_opt(2, 30, 0).unwrap(),
        chrono_tz::America::New_York,
    );
    assert_eq!(
        schedule
            .next_after("2026-03-08T06:00:00Z".parse().unwrap())
            .unwrap(),
        "2026-03-08T07:00:00Z".parse::<DateTime<Utc>>().unwrap()
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn locked_job_is_skipped_and_refresh_enqueues_elapsed_boundary(pool: PgPool) {
    use porchlight_db::{
        items,
        tx::{Actor, begin_as},
    };
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let clock = Arc::new(TestClock(Mutex::new(now())));
    sqlx::query("insert into porchlight.scheduler_jobs(name,next_due_at) values('locked',$1)")
        .bind(now())
        .execute(&pool)
        .await
        .unwrap();
    let mut held = pool.begin().await.unwrap();
    sqlx::query("select name from porchlight.scheduler_jobs where name='locked' for update")
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let job = Job::new(
        "locked",
        Schedule::Every(std::time::Duration::from_secs(60)),
        |_, _| Box::pin(async { panic!("locked callback must not run") }),
    );
    let s = Scheduler::new(app.clone(), clock.clone(), vec![job]).unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(1), s.tick())
            .await
            .unwrap()
            .unwrap(),
        0
    );
    held.rollback().await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let fields = items::ItemWrite {
        owner_id: h.user,
        title: "Boundary".into(),
        timing: porchlight_core::items::SeriesInput {
            due_date: Some("2026-10-04".parse().unwrap()),
            ..Default::default()
        },
        ..Default::default()
    };
    let out = items::create(
        &mut tx,
        h.id,
        &fields,
        &Default::default(),
        now() - Duration::days(2),
    )
    .await
    .unwrap();
    let initial: String =
        sqlx::query_scalar("select disposition from porchlight.item_occurrences where item_id=$1")
            .bind(out.item_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    assert_eq!(initial, "open");
    tx.commit().await.unwrap();
    let s = Scheduler::derivation(app.clone(), clock.clone())
        .await
        .unwrap();
    assert_eq!(s.tick().await.unwrap(), 8); // Horizon, dispositions, calendar, planning and both notification jobs (blob GC is added by from_state).
    let pending: bool = sqlx::query_scalar(
        "select applied_gen<requested_gen from porchlight.item_derivation where item_id=$1",
    )
    .bind(out.item_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(pending);
    // Enqueue trigger uses database now(); align its retry cursor with the injected worker clock.
    sqlx::query("update porchlight.item_derivation set retry_at=$2 where item_id=$1")
        .bind(out.item_id)
        .bind(now())
        .execute(&pool)
        .await
        .unwrap();
    porchlight_api::derive_worker::run_batch(&app, now())
        .await
        .unwrap();
    let disposition: String =
        sqlx::query_scalar("select disposition from porchlight.item_occurrences where item_id=$1")
            .bind(out.item_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(disposition, "overdue");
    *clock.0.lock().unwrap() = now() + Duration::minutes(5);
    // New instance retains persisted due state; no duplicate enqueue when already current.
    let restarted = Scheduler::derivation(app, clock).await.unwrap();
    assert_eq!(restarted.tick().await.unwrap(), 4); // Dispositions, calendar polling and both notification jobs.
    let pending: bool = sqlx::query_scalar(
        "select applied_gen<requested_gen from porchlight.item_derivation where item_id=$1",
    )
    .bind(out.item_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!pending);
}
