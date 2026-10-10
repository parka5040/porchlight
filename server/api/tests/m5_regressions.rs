//! Adversarial M5 regressions: injected clocks and lock-graph barriers, never timing sleeps.
use chrono::{DateTime, Duration, Utc};
use porchlight_api::{notifier, testing::wait_blocked_behind};
use porchlight_db::testing::{
    Role, connect_options_as, insert_appointment, pool_as, seed_household,
};
use sqlx::PgPool;
use uuid::Uuid;

fn at(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}
async fn emit(pool: &PgPool, h: Uuid, m: Uuid, item: Option<Uuid>, t: DateTime<Utc>) {
    sqlx::query("insert into porchlight.notification_events(household_id,recipient_member_id,kind,item_id,entity_type,entity_id,created_at) values($1,$2,'completed',$3,'item',$4,$5)")
        .bind(h).bind(m).bind(item).bind(item.unwrap_or(m)).bind(t).execute(pool).await.unwrap();
}
async fn derive(pool: &PgPool, item: Uuid, t: DateTime<Utc>) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("select id from porchlight.items where id=$1 for update")
        .bind(item)
        .execute(&mut *tx)
        .await
        .unwrap();
    porchlight_db::derive::derive_item(&mut tx, item, t)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}
async fn reminders(pool: &PgPool, t: DateTime<Utc>) {
    let mut tx = pool.begin().await.unwrap();
    notifier::reminders(&mut tx, t).await.unwrap();
    tx.commit().await.unwrap();
}
async fn drain(pool: &PgPool, t: DateTime<Utc>) -> anyhow::Result<usize> {
    let app = pool_as(pool, Role::App, Some("m5-notifier-app")).await?;
    notifier::drain(&app, t).await
}
async fn quiet(pool: &PgPool, m: Uuid) {
    sqlx::query("update porchlight.notification_prefs set quiet_start='22:00',quiet_end='07:00' where member_id=$1").bind(m).execute(pool).await.unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn n1_rederive_retains_already_due_unchanged_reminder(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let start = at("2026-10-05T12:00:00Z");
    sqlx::query(
        "update porchlight.items set starts_at=$2,ends_at=$2+interval '1 hour' where id=$1",
    )
    .bind(item)
    .bind(start)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,30)").bind(h.id).bind(item).execute(&pool).await.unwrap();
    derive(&pool, item, start - Duration::hours(1)).await;
    reminders(&pool, start - Duration::minutes(30) - Duration::seconds(10)).await;
    sqlx::query("update porchlight.items set buffer_before_min=5 where id=$1")
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    derive(
        &pool,
        item,
        start - Duration::minutes(30) + Duration::seconds(1),
    )
    .await;
    reminders(&pool, start - Duration::minutes(29)).await;
    reminders(&pool, start - Duration::minutes(28)).await;
    let count: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.notification_events where kind='reminder' and item_id=$1",
    )
    .bind(item)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "due reminder survives unrelated projection replacement, exactly once"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn n2_disabled_digest_suppresses_already_held_alert(pool: PgPool) {
    let app = pool_as(&pool, Role::App, Some("m5-release-app"))
        .await
        .unwrap();
    let h = seed_household(&pool).await.unwrap();
    quiet(&pool, h.super_admin).await;
    let night = at("2026-10-06T03:00:00Z"); // New York 23:00
    let morning = at("2026-10-06T11:00:00Z");
    emit(&pool, h.id, h.super_admin, None, night).await;
    assert_eq!(notifier::drain(&app, night).await.unwrap(), 1);
    sqlx::query("update porchlight.notification_prefs set digest=false where member_id=$1")
        .bind(h.super_admin)
        .execute(&pool)
        .await
        .unwrap();
    notifier::drain(&app, morning).await.unwrap();
    let alerts: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.outbox where kind='notification.created'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        alerts, 0,
        "disabling Digest before delivery must suppress its created tickle"
    );
    let held: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.notifications where released_at is null",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        held, 0,
        "production-role suppression must delete held digest, not back off on 42501"
    );
    let released: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.notifications where released_at is not null",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        released, 0,
        "suppressed held alert is not released inbox history"
    );
    let retained: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.notification_events where processed_at is not null",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        retained, 1,
        "suppression does not erase immutable event history"
    );
    sqlx::query("update porchlight.notification_prefs set digest=true where member_id=$1")
        .bind(h.super_admin)
        .execute(&pool)
        .await
        .unwrap();
    notifier::drain(&app, morning + Duration::seconds(1))
        .await
        .unwrap();
    let held: i64 = sqlx::query_scalar("select count(*) from porchlight.notifications")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(held, 0, "re-enabling does not resurrect a suppressed alert");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn n3_late_producer_commit_reuses_released_digest_identity(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    quiet(&pool, h.super_admin).await;
    let night = at("2026-10-06T03:00:00Z");
    let morning = at("2026-10-06T11:00:00Z");
    emit(&pool, h.id, h.super_admin, None, night).await;
    drain(&pool, night).await.unwrap();
    // Freeze producer transaction time just before quiet-end; commit only after release.
    let mut producer = pool.begin().await.unwrap();
    sqlx::query("select set_config('porchlight.notification_now',$1,true)")
        .bind((morning - Duration::seconds(1)).to_rfc3339())
        .execute(&mut *producer)
        .await
        .unwrap();
    sqlx::query("insert into porchlight.notification_events(household_id,recipient_member_id,kind,entity_type,entity_id) values($1,$2,'completed','item',$2)").bind(h.id).bind(h.super_admin).execute(&mut *producer).await.unwrap();
    drain(&pool, morning).await.unwrap();
    let original: Uuid =
        sqlx::query_scalar("select id from porchlight.notifications where released_at is not null")
            .fetch_one(&pool)
            .await
            .unwrap();
    producer.commit().await.unwrap();
    drain(&pool, morning + Duration::seconds(1)).await.unwrap();
    let rows: Vec<(Uuid, i32)> =
        sqlx::query_as("select id,count from porchlight.notifications where category='digest'")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        rows,
        vec![(original, 2)],
        "late committed source must not split the morning identity"
    );
    let created: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.outbox where kind='notification.created'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(created, 1);
    // Dismissed digest identity also remains stable; no unread reopen/new alert.
    sqlx::query("update porchlight.notifications set read_at=$2 where id=$1")
        .bind(original)
        .bind(morning)
        .execute(&pool)
        .await
        .unwrap();
    let tickles_before: Vec<(String, i64)> = sqlx::query_as("select kind,count(*) from porchlight.outbox where kind in ('notification.created','notification.dismissed') group by kind order by kind")
        .fetch_all(&pool).await.unwrap();
    assert_eq!(
        tickles_before,
        vec![
            ("notification.created".into(), 1),
            ("notification.dismissed".into(), 1)
        ]
    );
    emit(
        &pool,
        h.id,
        h.super_admin,
        None,
        morning - Duration::seconds(2),
    )
    .await;
    drain(&pool, morning + Duration::seconds(2)).await.unwrap();
    let rows: Vec<(Uuid, i32, Option<DateTime<Utc>>)> = sqlx::query_as(
        "select id,count,read_at from porchlight.notifications where category='digest'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows, vec![(original, 3, Some(morning))]);
    let tickles_after: Vec<(String, i64)> = sqlx::query_as("select kind,count(*) from porchlight.outbox where kind in ('notification.created','notification.dismissed') group by kind order by kind")
        .fetch_all(&pool).await.unwrap();
    assert_eq!(
        tickles_after, tickles_before,
        "late merge into read digest must not create or dismiss again"
    );
}

#[test]
fn n4_fold_start_quiet_membership_is_an_instant_interval() {
    let zone = chrono_tz::America::New_York;
    let start = Some("01:30".parse().unwrap());
    let end = Some("03:00".parse().unwrap());
    let release = Some(at("2026-11-01T08:00:00Z"));
    assert_eq!(
        notifier::quiet_release(at("2026-11-01T05:29:59Z"), zone, start, end),
        None
    );
    assert_eq!(
        notifier::quiet_release(at("2026-11-01T05:30:00Z"), zone, start, end),
        release
    );
    assert_eq!(
        notifier::quiet_release(at("2026-11-01T06:15:00Z"), zone, start, end),
        release,
        "second 01:15 is after the earlier 01:30 start"
    );
    assert_eq!(
        notifier::quiet_release(at("2026-11-01T08:00:00Z"), zone, start, end),
        None
    );
    // Overnight start in the repeated hour uses the preceding-date candidate too.
    assert_eq!(
        notifier::quiet_release(
            at("2026-11-01T06:15:00Z"),
            zone,
            start,
            Some("00:30".parse().unwrap())
        ),
        Some(at("2026-11-02T05:30:00Z"))
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn n5_consumer_batch_event_locks_precede_notification_and_purge(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let x = insert_appointment(&pool, &h).await.unwrap();
    let y = insert_appointment(&pool, &h).await.unwrap();
    let t = at("2026-10-05T12:00:00Z");
    emit(&pool, h.id, h.super_admin, Some(x), t).await;
    assert_eq!(drain(&pool, t).await.unwrap(), 1);
    let n: Uuid = sqlx::query_scalar("select id from porchlight.notifications")
        .fetch_one(&pool)
        .await
        .unwrap();
    emit(
        &pool,
        h.id,
        h.super_admin,
        Some(y),
        t + Duration::seconds(1),
    )
    .await;
    emit(
        &pool,
        h.id,
        h.super_admin,
        Some(x),
        t + Duration::seconds(2),
    )
    .await;
    sqlx::query("update porchlight.items set deleted_at=now() where id=$1")
        .bind(x)
        .execute(&pool)
        .await
        .unwrap();
    // Test-only trigger pauses C after it has locked N, before advancing to E_x.
    sqlx::raw_sql("create table porchlight.m5_gate(id int primary key); insert into porchlight.m5_gate values(1); create function porchlight.m5_pause() returns trigger language plpgsql security definer set search_path=pg_catalog,porchlight,pg_temp as $$ begin perform id from porchlight.m5_gate where id=1 for update; return new; end $$; create trigger m5_pause before update on porchlight.notifications for each row execute function porchlight.m5_pause();").execute(&pool).await.unwrap();
    let mut gate = pool.begin().await.unwrap();
    let gate_pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
        .fetch_one(&mut *gate)
        .await
        .unwrap();
    sqlx::query("select id from porchlight.m5_gate for update")
        .execute(&mut *gate)
        .await
        .unwrap();
    let consumer_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(connect_options_as(&pool, Role::App, Some("m5-consumer")).unwrap())
        .await
        .unwrap();
    let consumer_pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
        .fetch_one(&consumer_pool)
        .await
        .unwrap();
    let consumer =
        tokio::spawn(
            async move { notifier::drain(&consumer_pool, t + Duration::seconds(3)).await },
        );
    let bound = std::time::Duration::from_secs(10);
    wait_blocked_behind(&pool, gate_pid, 1, bound)
        .await
        .unwrap();
    let p = pool.clone();
    let purge = tokio::spawn(async move {
        let mut tx = p.begin().await.unwrap();
        let result = sqlx::query("select porchlight.pl_purge_items($1,$2)")
            .bind(h.id)
            .bind(vec![x])
            .execute(&mut *tx)
            .await;
        if result.is_ok() {
            tx.commit().await.unwrap();
        } else {
            tx.rollback().await.unwrap();
        }
        result
    });
    // Only this consumer's lock graph counts, not an arbitrary waiter in pg_locks.
    wait_blocked_behind(&pool, consumer_pid, 1, bound)
        .await
        .unwrap();
    gate.rollback().await.unwrap();
    let (consumed, purged) = tokio::time::timeout(bound, async {
        (consumer.await.unwrap(), purge.await.unwrap())
    })
    .await
    .unwrap();
    assert!(
        purged.is_ok(),
        "purge must not deadlock with consumer: {purged:?}"
    );
    assert_eq!(
        consumed.unwrap(),
        2,
        "both events processed without a deadlock-induced retry"
    );
    let surviving: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.notification_sources where notification_id=$1",
    )
    .bind(n)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        surviving, 1,
        "purge preserves unrelated Y source in the merged row"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn n1_changed_or_cancelled_due_reminders_do_not_resurrect(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let start = at("2026-10-05T12:00:00Z");
    let now = start - Duration::minutes(29);
    for change in [
        "anchor",
        "offset",
        "remove",
        "done",
        "deleted",
        "new-overdue",
    ] {
        let item = insert_appointment(&pool, &h).await.unwrap();
        sqlx::query(
            "update porchlight.items set starts_at=$2,ends_at=$2+interval '1 hour' where id=$1",
        )
        .bind(item)
        .bind(start)
        .execute(&pool)
        .await
        .unwrap();
        if change != "new-overdue" {
            sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,30)")
                .bind(h.id).bind(item).execute(&pool).await.unwrap();
        }
        derive(&pool, item, start - Duration::hours(1)).await;
        match change {
            "anchor" => {
                sqlx::query("update porchlight.items set starts_at=starts_at+interval '1 minute',ends_at=ends_at+interval '1 minute' where id=$1").bind(item).execute(&pool).await.unwrap();
            }
            "offset" => {
                // Reminder definitions are immutable: replacement is delete/insert.
                sqlx::query("delete from porchlight.item_reminders where item_id=$1")
                    .bind(item)
                    .execute(&pool)
                    .await
                    .unwrap();
                sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,31)")
                    .bind(h.id).bind(item).execute(&pool).await.unwrap();
            }
            "remove" => {
                sqlx::query("delete from porchlight.item_reminders where item_id=$1")
                    .bind(item)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            "done" => {
                sqlx::query("update porchlight.item_occurrences set status='done',completed_at=$2 where item_id=$1").bind(item).bind(now).execute(&pool).await.unwrap();
            }
            "deleted" => {
                sqlx::query("update porchlight.items set deleted_at=$2 where id=$1")
                    .bind(item)
                    .bind(now)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            "new-overdue" => {
                sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,30)").bind(h.id).bind(item).execute(&pool).await.unwrap();
            }
            _ => unreachable!(),
        }
        derive(&pool, item, now).await;
        reminders(&pool, now).await;
        let count: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_events where kind='reminder' and item_id=$1").bind(item).fetch_one(&pool).await.unwrap();
        assert_eq!(
            count, 0,
            "obsolete/new overdue reminder must not fire: {change}"
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn n1_synthetic_default_due_identity_survives_repeated_recovery(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let start = at("2026-10-05T12:00:00Z");
    sqlx::query(
        "update porchlight.items set starts_at=$2,ends_at=$2+interval '1 hour' where id=$1",
    )
    .bind(item)
    .bind(start)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("update porchlight.notification_prefs set default_appointment_offsets='{30}' where member_id=$1").bind(h.super_admin).execute(&pool).await.unwrap();
    derive(&pool, item, start - Duration::hours(1)).await;
    for seconds in [1, 2, 3] {
        derive(
            &pool,
            item,
            start - Duration::minutes(30) + Duration::seconds(seconds),
        )
        .await;
    }
    reminders(&pool, start - Duration::minutes(29)).await;
    derive(&pool, item, start - Duration::minutes(28)).await;
    reminders(&pool, start - Duration::minutes(27)).await;
    let count: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.notification_events where kind='reminder' and item_id=$1",
    )
    .bind(item)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "synthetic default UUID remains a durable exactly-once obligation"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn n4_fold_start_event_is_held_until_resolved_end(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    sqlx::query("update porchlight.notification_prefs set quiet_start='01:30',quiet_end='03:00' where member_id=$1").bind(h.super_admin).execute(&pool).await.unwrap();
    let t = at("2026-11-01T06:15:00Z");
    emit(&pool, h.id, h.super_admin, None, t).await;
    assert_eq!(drain(&pool, t).await.unwrap(), 1);
    let row: (String, DateTime<Utc>, Option<DateTime<Utc>>) =
        sqlx::query_as("select category,release_at,released_at from porchlight.notifications")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row, ("digest".into(), at("2026-11-01T08:00:00Z"), None));
    drain(&pool, at("2026-11-01T08:00:00Z")).await.unwrap();
    let released: DateTime<Utc> =
        sqlx::query_scalar("select released_at from porchlight.notifications")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(released, at("2026-11-01T08:00:00Z"));
}

#[sqlx::test(migrations = "../db/migrations")]
async fn n5_busy_item_reservation_defers_without_waiting_with_prefs(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let t = at("2026-10-05T12:00:00Z");
    emit(&pool, h.id, h.super_admin, Some(item), t).await;
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("select id from porchlight.items where id=$1 for update")
        .bind(item)
        .execute(&mut *holder)
        .await
        .unwrap();
    let consumed = tokio::time::timeout(std::time::Duration::from_secs(10), drain(&pool, t))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(consumed, 0);
    // This test proves prompt whole-recipient deferral only. Prefs NOWAIT after
    // drain returned cannot prove anything about locks while drain is in flight;
    // the i2 barrier test above checks prefs serialization during consumption.
    holder.rollback().await.unwrap();
    assert_eq!(drain(&pool, t).await.unwrap(), 1);
}

// A recurring CarryOver occurrence remains overdue beyond the event retention horizon.
#[sqlx::test(migrations = "../db/migrations")]
async fn i1_fired_reminder_is_not_carried_past_event_retention(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let start = at("2026-10-05T12:00:00Z");
    sqlx::query("update porchlight.items set starts_at=$2,ends_at=$2+interval '1 hour',rrule='FREQ=DAILY',recurrence_mode='calendar',missed_policy='carry_over' where id=$1")
        .bind(item).bind(start).execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,30)")
        .bind(h.id).bind(item).execute(&pool).await.unwrap();
    derive(&pool, item, start - Duration::hours(1)).await;
    let occurrence: Uuid = sqlx::query_scalar("select occurrence_id from porchlight.scheduled_reminders where item_id=$1 order by fire_at limit 1")
        .bind(item).fetch_one(&pool).await.unwrap();
    reminders(&pool, start).await;
    let fired: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_events where kind='reminder' and occurrence_id=$1")
        .bind(occurrence).fetch_one(&pool).await.unwrap();
    assert_eq!(fired, 1);
    // Routine horizon/edit regeneration must retire the emitted obligation while
    // its dedupe witness still exists, before the 90-day retention removes it.
    derive(&pool, item, start + Duration::days(1)).await;
    let later = start + Duration::days(92);
    sqlx::query("select porchlight.pl_retention($1)")
        .bind(later)
        .execute(&pool)
        .await
        .unwrap();
    let retained: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_events where kind='reminder' and occurrence_id=$1")
        .bind(occurrence).fetch_one(&pool).await.unwrap();
    assert_eq!(
        retained, 0,
        "real retention removes the expired dedupe witness"
    );
    derive(&pool, item, later).await;
    reminders(&pool, later).await;
    let replay: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_events where kind='reminder' and occurrence_id=$1")
        .bind(occurrence).fetch_one(&pool).await.unwrap();
    assert_eq!(
        replay, 0,
        "already-fired CarryOver obligation must not survive rederive and fire again after retention"
    );
}

// R1/R74: a never-rederived overdue CarryOver occurrence keeps its projection row; once
// retention removes the dedupe witness the 60-s scan must not fire it again.
#[sqlx::test(migrations = "../db/migrations")]
async fn r1_overdue_reminder_does_not_refire_after_retention(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let start = at("2026-10-05T12:00:00Z");
    sqlx::query("update porchlight.items set starts_at=$2,ends_at=$2+interval '1 hour',missed_policy='carry_over' where id=$1")
        .bind(item).bind(start).execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,30)")
        .bind(h.id).bind(item).execute(&pool).await.unwrap();
    derive(&pool, item, start - Duration::hours(1)).await;
    let occurrence: Uuid = sqlx::query_scalar("select occurrence_id from porchlight.scheduled_reminders where item_id=$1 order by fire_at limit 1")
        .bind(item).fetch_one(&pool).await.unwrap();
    let count = |pool: PgPool| async move {
        sqlx::query_scalar::<_, i64>("select count(*) from porchlight.notification_events where kind='reminder' and occurrence_id=$1")
            .bind(occurrence).fetch_one(&pool).await.unwrap()
    };
    reminders(&pool, start).await;
    assert_eq!(count(pool.clone()).await, 1);
    let later = start + Duration::days(92);
    sqlx::query("select porchlight.pl_retention($1)")
        .bind(later)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        count(pool.clone()).await,
        0,
        "retention removed the witness"
    );
    let status: (String, String) = sqlx::query_as(
        "select status::text,disposition::text from porchlight.item_occurrences where id=$1",
    )
    .bind(occurrence)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_ne!(status.1, "expired", "occurrence stays open: {status:?}");
    let rows: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.scheduled_reminders where occurrence_id=$1",
    )
    .bind(occurrence)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, 1, "projection row is still present");
    // (a) scan alone, no rederivation
    reminders(&pool, later).await;
    assert_eq!(
        count(pool.clone()).await,
        0,
        "scan re-fired a reminder outside the retention horizon"
    );
    // (b) rederive, then scan
    derive(&pool, item, later).await;
    reminders(&pool, later).await;
    assert_eq!(
        count(pool.clone()).await,
        0,
        "rederive re-armed a reminder outside the retention horizon"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn i2_consumer_notification_locks_match_purge_id_order(pool: PgPool) {
    let app = pool_as(&pool, Role::App, Some("m5-setup-app"))
        .await
        .unwrap();
    let h = seed_household(&pool).await.unwrap();
    let x = insert_appointment(&pool, &h).await.unwrap();
    let y = insert_appointment(&pool, &h).await.unwrap();
    let t = at("2026-10-05T12:00:00Z");
    emit(&pool, h.id, h.super_admin, Some(x), t).await;
    assert_eq!(notifier::drain(&app, t).await.unwrap(), 1);
    // Separate reminder/business windows, with UUID order opposite batch order.
    sqlx::query("insert into porchlight.notification_events(household_id,recipient_member_id,kind,item_id,entity_type,entity_id,created_at) values($1,$2,'reminder',$3,'item',$3,$4)")
        .bind(h.id).bind(h.super_admin).bind(x).bind(t).execute(&pool).await.unwrap();
    assert_eq!(notifier::drain(&app, t).await.unwrap(), 1);
    let ids: Vec<(Uuid, String)> =
        sqlx::query_as("select id,category from porchlight.notifications order by id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0].1, "activity");
    assert_eq!(ids[1].1, "reminders");
    sqlx::query("insert into porchlight.notification_events(household_id,recipient_member_id,kind,item_id,entity_type,entity_id,created_at) values($1,$2,'reminder',$3,'item',$3,$4)")
        .bind(h.id).bind(h.super_admin).bind(y).bind(t+Duration::seconds(1)).execute(&pool).await.unwrap();
    emit(
        &pool,
        h.id,
        h.super_admin,
        Some(y),
        t + Duration::seconds(2),
    )
    .await;
    sqlx::query("update porchlight.items set deleted_at=now() where id=$1")
        .bind(x)
        .execute(&pool)
        .await
        .unwrap();
    // Test-only barrier: pause C at its first merge. Old C holds only N_R;
    // fixed C already holds N_A then N_R. P walks real purge's ordered locks.
    sqlx::raw_sql("create table porchlight.m5_gate(id int primary key); insert into porchlight.m5_gate values(1); create function porchlight.m5_pause() returns trigger language plpgsql security definer set search_path=pg_catalog,porchlight,pg_temp as $$ begin perform id from porchlight.m5_gate where id=1 for update; return new; end $$; create trigger m5_pause before update on porchlight.notifications for each row execute function porchlight.m5_pause();")
        .execute(&pool).await.unwrap();
    let mut gate = pool.begin().await.unwrap();
    let gate_pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
        .fetch_one(&mut *gate)
        .await
        .unwrap();
    sqlx::query("select id from porchlight.m5_gate for update")
        .execute(&mut *gate)
        .await
        .unwrap();
    let consumer_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with(connect_options_as(&pool, Role::App, Some("m5-consumer")).unwrap())
        .await
        .unwrap();
    let consumer_pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
        .fetch_one(&consumer_pool)
        .await
        .unwrap();
    let consumer =
        tokio::spawn(
            async move { notifier::drain(&consumer_pool, t + Duration::seconds(3)).await },
        );
    let bound = std::time::Duration::from_secs(10);
    wait_blocked_behind(&pool, gate_pid, 1, bound)
        .await
        .unwrap();
    // This NOWAIT probe happens while C is suspended, not after it finishes.
    let mut prefs_probe = pool.begin().await.unwrap();
    let prefs = sqlx::query(
        "select member_id from porchlight.notification_prefs where member_id=$1 for update nowait",
    )
    .bind(h.super_admin)
    .execute(&mut *prefs_probe)
    .await
    .unwrap_err();
    assert_eq!(
        prefs.as_database_error().unwrap().code().as_deref(),
        Some("55P03")
    );
    prefs_probe.rollback().await.unwrap();
    let p = pool.clone();
    let purge = tokio::spawn(async move {
        let mut tx = p.begin().await.unwrap();
        let result = sqlx::query("select porchlight.pl_purge_items($1,$2)")
            .bind(h.id)
            .bind(vec![x])
            .execute(&mut *tx)
            .await;
        if result.is_ok() {
            tx.commit().await.unwrap();
        } else {
            tx.rollback().await.unwrap();
        }
        result
    });
    wait_blocked_behind(&pool, consumer_pid, 1, bound)
        .await
        .unwrap();
    gate.rollback().await.unwrap();
    let (consumed, purged) = tokio::time::timeout(bound, async {
        (consumer.await.unwrap(), purge.await.unwrap())
    })
    .await
    .unwrap();
    assert!(
        purged.is_ok(),
        "purge and reverse-processing consumer must not deadlock: {purged:?}"
    );
    assert_eq!(
        consumed.unwrap(),
        2,
        "both Y sources processed without deadlock retry"
    );
    let surviving: Vec<i64> = sqlx::query_scalar("select count(*) from porchlight.notification_sources where notification_id=any($1) group by notification_id order by notification_id")
        .bind(ids.iter().map(|(id,_)| *id).collect::<Vec<_>>()).fetch_all(&pool).await.unwrap();
    assert_eq!(
        surviving,
        vec![1, 1],
        "purge retains Y links in both coalesced windows"
    );
}
