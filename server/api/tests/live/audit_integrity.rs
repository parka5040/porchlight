use chrono::{DateTime, Duration, Utc};
use porchlight_api::notifier;
use porchlight_db::{
    derive,
    items::{ItemWrite, ScopeRequest, WriteScope},
    testing::{insert_appointment, seed_household},
};
use sqlx::PgPool;
use uuid::Uuid;

fn at(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

async fn produce(pool: &PgPool, now: DateTime<Utc>) {
    let mut tx = pool.begin().await.unwrap();
    notifier::reminders(&mut tx, now).await.unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn split_preserves_pending_reminders_without_replaying_emitted(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    sqlx::query("update porchlight.notification_prefs set default_appointment_offsets=array[30] where member_id=$1")
        .bind(h.super_admin).execute(&pool).await.unwrap();
    for explicit in [false, true] {
        for emitted in [false, true] {
            let item = insert_appointment(&pool, &h).await.unwrap();
            sqlx::query("update porchlight.items set starts_at='2026-10-05 12:00Z',ends_at='2026-10-05 13:00Z',rrule='FREQ=DAILY;COUNT=3',recurrence_mode='calendar' where id=$1")
                .bind(item).execute(&pool).await.unwrap();
            if explicit {
                sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,30)")
                    .bind(h.id).bind(item).execute(&pool).await.unwrap();
            }
            let mut tx = pool.begin().await.unwrap();
            let mut scope = WriteScope::begin(
                &mut tx,
                h.id,
                ScopeRequest {
                    items: vec![item],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            let row = scope.item(item).await.unwrap();
            scope
                .update(item, row.version, &ItemWrite::from(&row))
                .await
                .unwrap();
            scope.finish(at("2026-10-05T10:00:00Z")).await.unwrap();
            tx.commit().await.unwrap();
            let occurrence: Uuid = sqlx::query_scalar(
                "select id from porchlight.item_occurrences where item_id=$1 and seq=1",
            )
            .bind(item)
            .fetch_one(&pool)
            .await
            .unwrap();
            let now = at("2026-10-06T11:31:00Z");
            if emitted {
                produce(&pool, now).await;
            }
            let mut tx = pool.begin().await.unwrap();
            let mut scope = WriteScope::begin(
                &mut tx,
                h.id,
                ScopeRequest {
                    items: vec![item],
                    splits: vec![(item, 1)],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            let row = scope.item(item).await.unwrap();
            let (_, tail) = scope.split(item, row.version, 1).await.unwrap();
            let row = scope.item(tail).await.unwrap();
            let mut fields = ItemWrite::from(&row);
            fields.title = "Renamed series".into();
            scope.update(tail, row.version, &fields).await.unwrap();
            scope.finish(now).await.unwrap();
            tx.commit().await.unwrap();
            produce(&pool, now).await;
            produce(&pool, now).await;
            let count: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_events where kind='reminder' and occurrence_id=$1")
                .bind(occurrence).fetch_one(&pool).await.unwrap();
            assert_eq!(
                count, 1,
                "split reminder: explicit={explicit}, emitted={emitted}"
            );
            // A subsequent derivation must also retire the transferred obligation.
            let mut tx = pool.begin().await.unwrap();
            derive::derive_item(&mut tx, tail, now).await.unwrap();
            tx.commit().await.unwrap();
            let pending: i64 = sqlx::query_scalar(
                "select count(*) from porchlight.scheduled_reminders where occurrence_id=$1",
            )
            .bind(occurrence)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(pending, 0);
        }
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn floating_recipients_expire_at_their_own_boundary(pool: PgPool) {
    use porchlight_core::{items as core, time};
    use porchlight_db::items;
    for (owner_zone, recipient_zone, local, multiple) in [
        (
            "America/New_York",
            "America/Los_Angeles",
            "2026-10-10T08:00:00",
            false,
        ),
        (
            "Pacific/Auckland",
            "Australia/Sydney",
            "2026-10-04T08:00:00",
            true,
        ),
    ] {
        let h = seed_household(&pool).await.unwrap();
        for (member, zone) in [(h.super_admin, owner_zone), (h.user, recipient_zone)] {
            sqlx::query("update porchlight.household_members set tzid=$2 where id=$1")
                .bind(member)
                .bind(zone)
                .execute(&pool)
                .await
                .unwrap();
        }
        let local = local.parse().unwrap();
        let owner_due = time::resolve_local(local, time::parse_tz(owner_zone).unwrap());
        let recipient_due = time::resolve_local(local, time::parse_tz(recipient_zone).unwrap());
        assert!(owner_due < recipient_due);
        let mut tx = pool.begin().await.unwrap();
        let item = items::create(
            &mut tx,
            h.id,
            &ItemWrite {
                title: "Floating work".into(),
                owner_id: h.super_admin,
                timing: core::SeriesInput {
                    kind: core::Kind::Deadline,
                    time_mode: core::TimeMode::Floating,
                    local_due: Some(local),
                    missed_policy: core::MissedPolicy::Skip,
                    rrule: Some("FREQ=DAILY;COUNT=3".into()),
                    recurrence_mode: Some(core::RecurrenceMode::Calendar),
                    ..Default::default()
                },
                ..Default::default()
            },
            &items::Relationships {
                assignees: if multiple {
                    vec![h.super_admin, h.user]
                } else {
                    vec![h.user]
                },
                reminder_offsets: vec![0],
                ..Default::default()
            },
            owner_due - Duration::hours(1),
        )
        .await
        .unwrap()
        .item_id;
        tx.commit().await.unwrap();
        let occurrence: Uuid = sqlx::query_scalar(
            "select id from porchlight.item_occurrences where item_id=$1 and seq=0",
        )
        .bind(item)
        .fetch_one(&pool)
        .await
        .unwrap();
        let middle = owner_due + Duration::minutes(5);
        porchlight_api::scheduler::refresh_dispositions(&pool, middle)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        derive::derive_item(&mut tx, item, middle).await.unwrap();
        tx.commit().await.unwrap();
        let disposition: String =
            sqlx::query_scalar("select disposition from porchlight.item_occurrences where id=$1")
                .bind(occurrence)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            disposition, "expired",
            "board keeps the owner-zone disposition"
        );
        let agenda: Vec<Uuid> = sqlx::query_scalar(
            "select member_id from porchlight.member_agenda where occurrence_id=$1",
        )
        .bind(occurrence)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            agenda,
            vec![h.user],
            "recipient must keep floating agenda until own boundary"
        );
        let fire:DateTime<Utc> = sqlx::query_scalar("select fire_at from porchlight.scheduled_reminders where occurrence_id=$1 and member_id=$2")
            .bind(occurrence).bind(h.user).fetch_one(&pool).await.unwrap();
        assert_eq!(fire, recipient_due);
        produce(&pool, recipient_due).await;
        let count:i64 = sqlx::query_scalar("select count(*) from porchlight.notification_events where occurrence_id=$1 and recipient_member_id=$2 and kind='reminder'")
            .bind(occurrence).bind(h.user).fetch_one(&pool).await.unwrap();
        assert_eq!(
            count, 1,
            "aggregate board expiry must not suppress recipient reminder"
        );
        let late = recipient_due + Duration::minutes(5);
        porchlight_api::scheduler::refresh_dispositions(&pool, late)
            .await
            .unwrap();
        let pending: bool = sqlx::query_scalar(
            "select requested_gen>applied_gen from porchlight.item_derivation where item_id=$1",
        )
        .bind(item)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            pending,
            "scheduler must refresh a recipient boundary even if board is already expired"
        );
        let mut tx = pool.begin().await.unwrap();
        derive::derive_item(&mut tx, item, late).await.unwrap();
        tx.commit().await.unwrap();
        let remaining: i64 = sqlx::query_scalar(
            "select count(*) from porchlight.member_agenda where occurrence_id=$1",
        )
        .bind(occurrence)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining, 0);
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn idle_scheduler_uses_one_read_and_no_job_transactions(pool: PgPool) {
    use porchlight_api::scheduler::{Clock, Job, Schedule, Scheduler};
    use porchlight_db::testing::{Role, connect_options_as};
    use sqlx::ConnectOptions;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use tracing::Instrument;
    use tracing_subscriber::{Layer, layer::SubscriberExt};
    struct TestClock(Mutex<DateTime<Utc>>);
    impl Clock for TestClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap()
        }
    }
    struct Counter(Arc<AtomicUsize>);
    impl<S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>> Layer<S>
        for Counter
    {
        fn on_event(&self, e: &tracing::Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
            if e.metadata().target() == "sqlx::query"
                && ctx
                    .event_scope(e)
                    .is_some_and(|s| s.from_root().any(|s| s.name() == "audit_idle_scheduler"))
            {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    let options = connect_options_as(&pool, Role::App, None)
        .unwrap()
        .log_statements(tracing::log::LevelFilter::Debug);
    let app = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    let clock = Arc::new(TestClock(Mutex::new(at("2026-10-10T12:00:00Z"))));
    let jobs = (0..9)
        .map(|n| {
            Job::new(
                format!("idle_{n}"),
                Schedule::Every(std::time::Duration::from_secs(86400)),
                |_, _| Box::pin(async { Ok(()) }),
            )
        })
        .collect();
    let scheduler = Scheduler::new(app, clock.clone(), jobs).unwrap();
    assert_eq!(
        scheduler.tick().await.unwrap(),
        9,
        "startup catches up once"
    );
    let count = Arc::new(AtomicUsize::new(0));
    tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(Counter(count.clone())),
    )
    .unwrap();
    async {
        for _ in 0..12 {
            *clock.0.lock().unwrap() += Duration::seconds(5);
            assert_eq!(scheduler.tick().await.unwrap(), 0);
        }
    }
    .instrument(tracing::info_span!("audit_idle_scheduler"))
    .await;
    assert_eq!(
        count.load(Ordering::SeqCst),
        12,
        "idle minute must use only one due read per tick, no attributed election transactions"
    );
}
