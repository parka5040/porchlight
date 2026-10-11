//! Persisted, reusable, single-runner jobs. Job effects and success state commit together.
use chrono::{DateTime, Duration, NaiveTime, Utc};
use chrono_tz::Tz;
use futures_util::future::BoxFuture;
use porchlight_core::{items as core, time};
use porchlight_db::{
    derive,
    tx::{Actor, begin_as},
};
use sqlx::{Acquire, PgConnection, PgPool};
use std::{future::Future, sync::Arc};

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}
/// Adapter for the shared injected item_clock; no separate mutable clock in AppState.
pub struct CallbackClock(pub Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>);
impl Clock for CallbackClock {
    fn now(&self) -> DateTime<Utc> {
        (self.0)()
    }
}
pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
#[derive(Clone, Debug)]
pub enum Schedule {
    DailyAt(NaiveTime, Tz),
    /// Resolve the current live household zone after every run; UTC before initialization.
    DailyInHouseholdAt(NaiveTime),
    Every(std::time::Duration),
}
impl Schedule {
    pub fn next_after(&self, now: DateTime<Utc>) -> anyhow::Result<DateTime<Utc>> {
        match self {
            Self::DailyInHouseholdAt(t) => Self::DailyAt(*t, chrono_tz::UTC).next_after(now),
            Self::Every(d) => {
                anyhow::ensure!(!d.is_zero(), "job interval must be positive");
                now.checked_add_signed(Duration::from_std(*d)?)
                    .ok_or_else(|| anyhow::anyhow!("schedule overflow"))
            }
            Self::DailyAt(t, z) => {
                let local = time::to_local(now, *z);
                let today = time::resolve_local(local.date().and_time(*t), *z);
                if today > now {
                    Ok(today)
                } else {
                    Ok(time::resolve_local(
                        local
                            .date()
                            .succ_opt()
                            .ok_or_else(|| anyhow::anyhow!("schedule overflow"))?
                            .and_time(*t),
                        *z,
                    ))
                }
            }
        }
    }
}
type Run = dyn for<'a> Fn(&'a mut PgConnection, DateTime<Utc>) -> BoxFuture<'a, anyhow::Result<()>>
    + Send
    + Sync;
#[derive(Clone)]
pub struct Job {
    pub name: String,
    pub schedule: Schedule,
    run: Arc<Run>,
}
impl Job {
    /// Callback borrows the actor-attributed job transaction; never commit it yourself.
    pub fn new<F>(name: impl Into<String>, schedule: Schedule, run: F) -> Self
    where
        F: for<'a> Fn(&'a mut PgConnection, DateTime<Utc>) -> BoxFuture<'a, anyhow::Result<()>>
            + Send
            + Sync
            + 'static,
    {
        Self {
            name: name.into(),
            schedule,
            run: Arc::new(run),
        }
    }
}
pub struct Scheduler {
    pool: PgPool,
    clock: Arc<dyn Clock>,
    jobs: Vec<Job>,
    registered: tokio::sync::OnceCell<()>,
}
impl Scheduler {
    pub fn new(pool: PgPool, clock: Arc<dyn Clock>, jobs: Vec<Job>) -> anyhow::Result<Self> {
        let mut names = std::collections::BTreeSet::new();
        for job in &jobs {
            anyhow::ensure!(
                !job.name.is_empty() && job.name.len() <= 100,
                "invalid job name"
            );
            anyhow::ensure!(names.insert(job.name.clone()), "duplicate job name");
            job.schedule.next_after(clock.now())?;
        }
        Ok(Self {
            pool,
            clock,
            jobs,
            registered: tokio::sync::OnceCell::new(),
        })
    }
    /// First registration is due immediately; subsequent registrations preserve the persisted cursor.
    /// Overdue jobs run ONCE, not once per missed interval. Failures use capped exponential backoff.
    pub async fn tick(&self) -> anyhow::Result<usize> {
        let now = self.clock.now();
        self.registered.get_or_try_init(|| async {
            for job in &self.jobs {
                // Registration is once per runner, outside election transactions.
                // Existing locked rows are read, never conflict-updated.
                let exists: bool = sqlx::query_scalar(
                    "select exists(select 1 from porchlight.scheduler_jobs where name=$1)",
                ).bind(&job.name).fetch_one(&self.pool).await?;
                if !exists {
                    sqlx::query("insert into porchlight.scheduler_jobs(name,next_due_at) values($1,$2) on conflict do nothing").bind(&job.name).bind(now).execute(&self.pool).await?;
                }
            }
            Ok::<(), sqlx::Error>(())
        }).await?;
        // This unlocked hint is the only idle-tick read. The election below must
        // still recheck due time under SKIP LOCKED against other runners.
        let names: Vec<_> = self.jobs.iter().map(|job| job.name.as_str()).collect();
        let due: std::collections::BTreeSet<String> = sqlx::query_scalar(
            "select name from porchlight.scheduler_jobs where name=any($1) and next_due_at<=$2",
        )
        .bind(&names)
        .bind(now)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .collect();
        let mut count = 0;
        for job in self.jobs.iter().filter(|job| due.contains(&job.name)) {
            let mut tx = begin_as(&self.pool, &Actor::system()).await?;
            let due:Option<i32>=sqlx::query_scalar("select attempts from porchlight.scheduler_jobs where name=$1 and next_due_at<=$2 for update skip locked").bind(&job.name).bind(now).fetch_optional(&mut *tx).await?;
            let Some(attempts) = due else {
                tx.rollback().await?;
                continue;
            };
            sqlx::query("update porchlight.scheduler_jobs set last_run_at=$2 where name=$1")
                .bind(&job.name)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            let mut savepoint = tx.begin().await?;
            let result = (job.run)(&mut savepoint, now).await;
            let success = result.is_ok();
            if success {
                savepoint.commit().await?;
            } else {
                savepoint.rollback().await?;
            }
            // Error strings from external jobs can contain credentials. Store a safe category only.
            let next = if success {
                match &job.schedule {
                    Schedule::DailyInHouseholdAt(t) => {
                        let zone: Option<String> = sqlx::query_scalar("select tzid from porchlight.households where deleted_at is null order by id limit 1").fetch_optional(&mut *tx).await?;
                        Schedule::DailyAt(*t, derive::parse_zone(zone.as_deref().unwrap_or("UTC"))?)
                            .next_after(now)?
                    }
                    schedule => schedule.next_after(now)?,
                }
            } else {
                now + Duration::seconds((5_i64 * (1_i64 << (attempts + 1).min(20))).min(900))
            };
            sqlx::query("update porchlight.scheduler_jobs set last_success_at=case when $3 then $2 else last_success_at end,next_due_at=$4,last_error=case when $3 then null else 'job failed; exponential backoff' end,attempts=case when $3 then 0 else least(attempts+1,20) end where name=$1").bind(&job.name).bind(now).bind(success).bind(next).execute(&mut *tx).await?;
            tx.commit().await?;
            count += 1;
        }
        Ok(count)
    }
    pub async fn run(&self, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
        tokio::pin!(shutdown);
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {()=shutdown.as_mut()=>return Ok(()),_=tick.tick()=>{}}
            tokio::select! {()=shutdown.as_mut()=>return Ok(()),result=self.tick()=>{if result.is_err(){tracing::warn!("scheduler tick failed; retrying next tick");}}}
        }
    }
    fn with_blobs(mut self, root: std::path::PathBuf, budget: crate::blobs::DiskBudget) -> Self {
        let pool = &self.pool;
        let gc_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy_with(
                (*pool.connect_options())
                    .clone()
                    .application_name("porchlight-scheduler-blobs")
                    .options(crate::db::APP_SETTINGS.iter().copied()),
            );
        let metadata_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy_with(
                (*pool.connect_options())
                    .clone()
                    .application_name("porchlight-gc-metadata")
                    .options(crate::db::APP_SETTINGS.iter().copied()),
            );
        self.jobs.push(Job::new(
            "blob_gc",
            Schedule::DailyInHouseholdAt(NaiveTime::from_hms_opt(3, 30, 0).unwrap()),
            move |_, now| {
                let root = root.clone();
                let pool = gc_pool.clone();
                let metadata_pool = metadata_pool.clone();
                let budget = budget.clone();
                Box::pin(async move {
                    budget.gc(&pool, &metadata_pool, &root, now).await?;
                    Ok(())
                })
            },
        ));
        self
    }
    pub async fn from_state(
        state: &crate::state::AppState,
        clock: Arc<dyn Clock>,
    ) -> anyhow::Result<Self> {
        Self::derivation_with_fetcher(state.pool.clone(), clock, state.calendar_fetcher.clone())
            .await
            .map(|scheduler| {
                scheduler.with_blobs(state.settings.data_dir.clone(), state.disk_budget.clone())
            })
    }
    pub async fn derivation(pool: PgPool, clock: Arc<dyn Clock>) -> anyhow::Result<Self> {
        Self::derivation_with_fetcher(pool, clock, crate::calendar::fetch::Fetcher::default()).await
    }
    pub async fn derivation_with_fetcher(
        pool: PgPool,
        clock: Arc<dyn Clock>,
        calendar_fetcher: crate::calendar::fetch::Fetcher,
    ) -> anyhow::Result<Self> {
        // Independent item commits need a slot while the scheduler election transaction
        // retains its job row. A separate one-slot operational pool avoids starving a
        // small request pool (and simultaneous runners of different built-in jobs).
        let item_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy_with(
                (*pool.connect_options())
                    .clone()
                    .application_name("porchlight-scheduler-items")
                    .options(crate::db::APP_SETTINGS.iter().copied()),
            );
        let horizon_pool = item_pool.clone();
        let feed_pool = item_pool.clone();
        let planning_pool = item_pool.clone();
        let refresh_pool = item_pool;
        let feed_running = Arc::new(std::sync::atomic::AtomicBool::new(false));

        Self::new(
            pool,
            clock,
            [
                vec![
                    // S8a: polling honors each source's daily-by-default due timestamp.
                    Job::new(
                        "refresh_ics_sources",
                        Schedule::Every(std::time::Duration::from_secs(60)),
                        move |_, now| {
                            let pool = feed_pool.clone();
                            let fetcher = calendar_fetcher.clone();
                            let running = feed_running.clone();
                            Box::pin(async move {
                                if !running.swap(true, std::sync::atomic::Ordering::AcqRel) {
                                    tokio::spawn(async move {
                                        struct Reset(Arc<std::sync::atomic::AtomicBool>);
                                        impl Drop for Reset {
                                            fn drop(&mut self) {
                                                self.0.store(
                                                    false,
                                                    std::sync::atomic::Ordering::Release,
                                                );
                                            }
                                        }
                                        let _reset = Reset(running);
                                        if crate::calendar::jobs::run_due(&pool, now, &fetcher)
                                            .await
                                            .is_err()
                                        {
                                            tracing::warn!("calendar polling pass failed");
                                        }
                                    });
                                }
                                Ok(())
                            })
                        },
                    ),
                    // s8b: EDF proposals; nothing moves without acceptance.
                    Job::new(
                        "schedule_suggestions",
                        Schedule::DailyInHouseholdAt(NaiveTime::from_hms_opt(3, 0, 0).unwrap()),
                        move |_, now| {
                            let pool = planning_pool.clone();
                            Box::pin(async move { crate::planning::nightly(&pool, now).await })
                        },
                    ),
                    Job::new(
                        "extend_horizon",
                        Schedule::DailyInHouseholdAt(NaiveTime::from_hms_opt(2, 30, 0).unwrap()),
                        move |_, now| {
                            let pool = horizon_pool.clone();
                            Box::pin(async move { extend_horizon(&pool, now).await })
                        },
                    ),
                    Job::new(
                        "refresh_dispositions",
                        Schedule::Every(std::time::Duration::from_secs(300)),
                        move |_, now| {
                            let pool = refresh_pool.clone();
                            Box::pin(async move { refresh_dispositions(&pool, now).await })
                        },
                    ),
                ],
                crate::notifier::jobs(),
                vec![
                    Job::new(
                        "backup_stale",
                        Schedule::Every(std::time::Duration::from_secs(3600)),
                        |conn, now| {
                            Box::pin(
                                async move { crate::routes::backups::emit_stale(conn, now).await },
                            )
                        },
                    ),
                    Job::new(
                        "retention",
                        Schedule::DailyInHouseholdAt(NaiveTime::from_hms_opt(3, 45, 0).unwrap()),
                        |conn, now| {
                            Box::pin(async move {
                                sqlx::query("select porchlight.pl_retention($1)")
                                    .bind(now)
                                    .execute(conn)
                                    .await?;
                                Ok(())
                            })
                        },
                    ),
                ],
            ]
            .concat(),
        )
    }
}
/// Built-in sweeps commit independent items, not a household-wide transaction.
/// The scheduler job lock elects the runner; only each item's short transaction holds domain locks.
pub async fn extend_horizon(pool: &PgPool, now: DateTime<Utc>) -> anyhow::Result<()> {
    let ids: Vec<uuid::Uuid> = sqlx::query_scalar(
        "select i.id from porchlight.items i join porchlight.households h on h.id=i.household_id join porchlight.item_derivation d on d.item_id=i.id where i.deleted_at is null and i.recurrence_mode='calendar' and d.applied_gen=d.requested_gen and d.attempts=0 and (d.calendar_horizon is null or d.calendar_horizon < (($1 at time zone coalesce(i.tzid,h.tzid))+interval '18 months') at time zone coalesce(i.tzid,h.tzid)) and coalesce((select max(coalesce(o.local_start,o.local_due,o.due_date::timestamp)) from porchlight.item_occurrences o where o.item_id=i.id and not o.is_detached),'-infinity'::timestamp) < ($1 at time zone coalesce(i.tzid,h.tzid))+interval '18 months' order by i.id"
    ).bind(now).fetch_all(pool).await?;
    for id in ids {
        let mut tx = begin_as(pool, &Actor::system()).await?;
        let mut sp = tx.begin().await?;
        let result = extend_one(&mut sp, id, now).await;
        match result {
            Ok(()) => sp.commit().await?,
            Err(_) => {
                sp.rollback().await?;
                tracing::warn!(%id, "horizon candidate failed; left for worker");
            }
        }
        tx.commit().await?;
    }
    Ok(())
}
async fn extend_one(
    conn: &mut PgConnection,
    id: uuid::Uuid,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    let locked: Option<uuid::Uuid> = sqlx::query_scalar("select id from porchlight.items where id=$1 and deleted_at is null and recurrence_mode='calendar' for no key update skip locked").bind(id).fetch_optional(&mut *conn).await?;
    if locked.is_none() {
        return Ok(());
    }
    let row = derive::load_item(conn, id).await?;
    let spec = row.spec()?;
    let zone: String = sqlx::query_scalar("select tzid from porchlight.households where id=$1")
        .bind(row.household_id)
        .fetch_one(&mut *conn)
        .await?;
    let zone = spec.zone().unwrap_or(derive::parse_zone(&zone)?);
    let horizon = core::horizon_end(now, zone)?;
    let state: (bool, Option<DateTime<Utc>>) = sqlx::query_as("select applied_gen<requested_gen or attempts>0,calendar_horizon from porchlight.item_derivation where item_id=$1").bind(id).fetch_one(&mut *conn).await?;
    if state.0 || state.1.is_some_and(|end| end >= horizon) {
        return Ok(());
    }
    let existing: std::collections::BTreeSet<i32> = sqlx::query_scalar::<_, i32>(
        "select seq from porchlight.item_occurrences where item_id=$1",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .collect();
    let desired = core::expand(
        &spec,
        chrono::NaiveDate::from_ymd_opt(1, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap(),
        time::to_local(horizon, zone),
    )?;
    // COUNT/UNTIL/split heads that have no more occurrences must not enqueue forever.
    if desired.iter().any(|a| !existing.contains(&(a.seq as i32))) {
        enqueue_if_current(conn, id).await?;
    }
    Ok(())
}
async fn enqueue_if_current(conn: &mut PgConnection, id: uuid::Uuid) -> anyhow::Result<()> {
    let current: Option<bool> = sqlx::query_scalar("select applied_gen=requested_gen and attempts=0 from porchlight.item_derivation where item_id=$1 for update nowait").bind(id).fetch_optional(&mut *conn).await?;
    if current == Some(true) {
        sqlx::query("select porchlight.pl_request_derivation($1)")
            .bind(id)
            .execute(conn)
            .await?;
    }
    Ok(())
}
/// Compute core dispositions in a per-item SAVEPOINT under item-first SKIP LOCKED.
/// Invalid overrides are logged by ID, left pending for the worker's bounded backoff,
/// and never reset worker attempts/retry_at. Locks release at EVERY item commit.
pub async fn refresh_dispositions(pool: &PgPool, now: DateTime<Utc>) -> anyhow::Result<()> {
    let ids: Vec<uuid::Uuid> = sqlx::query_scalar("select i.id from porchlight.items i where deleted_at is null and exists(select 1 from porchlight.item_occurrences o where o.item_id=i.id and o.status not in ('done','skipped')) order by i.id").fetch_all(pool).await?;
    for id in ids {
        let mut tx = begin_as(pool, &Actor::system()).await?;
        let mut sp = tx.begin().await?;
        let result = refresh_one(&mut sp, id, now).await;
        match result {
            Ok(()) => sp.commit().await?,
            Err(_) => {
                sp.rollback().await?;
                tracing::warn!(%id, "disposition candidate failed; left for worker backoff");
            }
        }
        tx.commit().await?;
    }
    Ok(())
}
async fn refresh_one(
    conn: &mut PgConnection,
    id: uuid::Uuid,
    now: DateTime<Utc>,
) -> anyhow::Result<()> {
    let locked: Option<uuid::Uuid> = sqlx::query_scalar("select id from porchlight.items where id=$1 and deleted_at is null for no key update skip locked").bind(id).fetch_optional(&mut *conn).await?;
    if locked.is_none() {
        return Ok(());
    }
    let row = derive::load_item(conn, id).await?;
    let spec = row.spec()?;
    let z: String = sqlx::query_scalar("select porchlight.pl_effective_member_zone(m.role,m.tzid,h.tzid) from porchlight.household_members m join porchlight.households h on h.id=m.household_id where m.id=$1").bind(row.subject_member_id.unwrap_or(row.owner_id)).fetch_one(&mut *conn).await?;
    let rows = derive::occurrences(conn, id).await?;
    let effective: Vec<core::OpenOccurrence> = rows
        .iter()
        .map(|o| {
            Ok(core::OpenOccurrence {
                anchor: derive::effective_anchor(o, &spec)?,
                status: o.status,
                disposition: o.disposition,
                closed_at: o.closed_at,
                generated_anchor: o.is_detached.then(|| o.anchor.clone()),
            })
        })
        .collect::<derive::Result<_>>()?;
    let expected = core::dispositions(
        &effective,
        now,
        spec.input().missed_policy,
        spec.zone().unwrap_or(derive::parse_zone(&z)?),
    )?;
    if rows
        .iter()
        .zip(expected)
        .any(|(o, d)| o.status.is_open() && o.disposition != d)
    {
        enqueue_if_current(conn, id).await?;
        return Ok(());
    }
    if spec.input().time_mode == core::TimeMode::Floating {
        // The board may already be expired while a later-zone recipient is still
        // waiting. Compare recipient eligibility too, without taking member locks.
        let profiles: Vec<(String, Vec<uuid::Uuid>)> = sqlx::query_as("select porchlight.pl_effective_member_zone(m.role,m.tzid,h.tzid),array(select a.occurrence_id from porchlight.member_agenda a where a.item_id=$1 and a.member_id=m.id order by a.occurrence_id) from porchlight.household_members m join porchlight.households h on h.id=m.household_id where m.deleted_at is null and (m.id=$2 or exists(select 1 from porchlight.item_assignees a where a.item_id=$1 and a.member_id=m.id) or ($2::uuid is null and m.id=$3 and not exists(select 1 from porchlight.item_assignees a where a.item_id=$1)))")
            .bind(id).bind(row.subject_member_id).bind(row.owner_id).fetch_all(&mut *conn).await?;
        for (zone, actual) in profiles {
            let expected = core::dispositions(
                &effective,
                now,
                spec.input().missed_policy,
                derive::parse_zone(&zone)?,
            )?;
            let mut eligible: Vec<_> = rows
                .iter()
                .zip(expected)
                .filter(|(o, d)| o.status.is_open() && *d != core::Disposition::Expired)
                .map(|(o, _)| o.id)
                .collect();
            eligible.sort();
            if actual != eligible {
                enqueue_if_current(conn, id).await?;
                break;
            }
        }
    }
    Ok(())
}
