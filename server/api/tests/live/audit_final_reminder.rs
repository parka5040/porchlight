use chrono::{DateTime, Utc};
use porchlight_api::notifier;
use porchlight_db::{
    derive,
    items::{ItemWrite, ScopeRequest, WriteScope},
    testing::{Household, insert_appointment, seed_household},
    tx,
};
use sqlx::PgPool;
use uuid::Uuid;

fn at(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct Reminder {
    id: Uuid,
    member_id: Uuid,
    occurrence_id: Uuid,
    item_id: Uuid,
    reminder_id: Uuid,
    fire_at: DateTime<Utc>,
}

async fn scheduled(pool: &PgPool, item: Uuid) -> Vec<Reminder> {
    sqlx::query_as("select id,member_id,occurrence_id,item_id,reminder_id,fire_at from porchlight.scheduled_reminders where item_id=$1 order by member_id,occurrence_id,reminder_id")
        .bind(item).fetch_all(pool).await.unwrap()
}

async fn regenerate(pool: &PgPool, item: Uuid, now: DateTime<Utc>) {
    let mut tx = tx::begin(pool).await.unwrap();
    derive::derive_item(&mut tx, item, now).await.unwrap();
    tx.commit().await.unwrap();
}

async fn fixture(pool: &PgPool, explicit: bool) -> (Household, Uuid) {
    let h = seed_household(pool).await.unwrap();
    let item = insert_appointment(pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set starts_at='2026-10-05 12:00Z',ends_at='2026-10-05 13:00Z',rrule='FREQ=DAILY;COUNT=3',recurrence_mode='calendar' where id=$1")
        .bind(item).execute(pool).await.unwrap();
    if explicit {
        sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,15),($1,$2,30)")
            .bind(h.id).bind(item).execute(pool).await.unwrap();
    } else {
        sqlx::query("update porchlight.notification_prefs set default_appointment_offsets=array[15,30] where member_id=$1")
            .bind(h.super_admin).execute(pool).await.unwrap();
    }
    regenerate(pool, item, at("2026-10-05T10:00:00Z")).await;
    assert_eq!(scheduled(pool, item).await.len(), 6);
    (h, item)
}

async fn edit_title(pool: &PgPool, h: Household, item: Uuid, now: DateTime<Utc>) {
    let mut tx = tx::begin(pool).await.unwrap();
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
    let mut fields = ItemWrite::from(&row);
    fields.title = "Renamed appointment".into();
    scope.update(item, row.version, &fields).await.unwrap();
    scope.finish(now).await.unwrap();
    tx.commit().await.unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unrelated_edit_keeps_future_row_ids(pool: PgPool) {
    for explicit in [false, true] {
        let (h, item) = fixture(&pool, explicit).await;
        let before = scheduled(&pool, item).await;
        edit_title(&pool, h, item, at("2026-10-05T10:01:00Z")).await;
        assert_eq!(
            scheduled(&pool, item).await,
            before,
            "unchanged future reminders must retain their row IDs"
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn due_unemitted_rows_keep_ids_across_regeneration(pool: PgPool) {
    for explicit in [false, true] {
        let (h, item) = fixture(&pool, explicit).await;
        let before = scheduled(&pool, item).await;
        let now = at("2026-10-05T11:46:00Z");
        assert_eq!(before.iter().filter(|r| r.fire_at <= now).count(), 2);
        let emitted: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_events where kind='reminder' and item_id=$1")
            .bind(item).fetch_one(&pool).await.unwrap();
        assert_eq!(emitted, 0);
        edit_title(&pool, h, item, now).await;
        assert_eq!(
            scheduled(&pool, item).await,
            before,
            "due unemitted reminders must retain their row IDs"
        );
        regenerate(&pool, item, now).await;
        assert_eq!(scheduled(&pool, item).await, before);
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn changed_fire_at_and_new_tuple_get_new_ids(pool: PgPool) {
    let (_, item) = fixture(&pool, true).await;
    let before = scheduled(&pool, item).await;
    // Explicit rows are immutable: replace the offset while keeping its delivery identity.
    let mut tx = tx::begin(&pool).await.unwrap();
    let changed: Uuid = sqlx::query_scalar(
        "delete from porchlight.item_reminders where item_id=$1 and offset_minutes=30 returning id",
    )
    .bind(item)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let changed_row: Uuid = sqlx::query_scalar("insert into porchlight.item_reminders(household_id,item_id,offset_minutes,delivery_id) select household_id,id,45,$2 from porchlight.items where id=$1 returning id")
        .bind(item).bind(changed).fetch_one(&mut *tx).await.unwrap();
    derive::derive_item(&mut tx, item, at("2026-10-05T10:01:00Z"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let after = scheduled(&pool, item).await;
    assert_eq!(after.iter().filter(|r| r.reminder_id == changed).count(), 3);
    for row in after.iter().filter(|r| r.reminder_id == changed) {
        let old = before
            .iter()
            .find(|r| r.occurrence_id == row.occurrence_id && r.reminder_id == row.reminder_id)
            .unwrap();
        assert_ne!(row.fire_at, old.fire_at);
        assert_ne!(row.id, old.id, "changed fire_at must get a new row ID");
    }
    // Replacing an offset with the same fire time but a new delivery identity is not reuse.
    let mut tx = tx::begin(&pool).await.unwrap();
    sqlx::query("delete from porchlight.item_reminders where id=$1")
        .bind(changed_row)
        .execute(&mut *tx)
        .await
        .unwrap();
    let replacement: Uuid = sqlx::query_scalar("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) select household_id,id,45 from porchlight.items where id=$1 returning id")
        .bind(item).fetch_one(&mut *tx).await.unwrap();
    derive::derive_item(&mut tx, item, at("2026-10-05T10:02:00Z"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let replaced = scheduled(&pool, item).await;
    assert_eq!(
        replaced
            .iter()
            .filter(|r| r.reminder_id == replacement)
            .count(),
        3
    );
    for row in replaced.iter().filter(|r| r.reminder_id == replacement) {
        let old = after
            .iter()
            .find(|r| r.occurrence_id == row.occurrence_id && r.reminder_id == changed)
            .unwrap();
        assert_eq!(row.fire_at, old.fire_at);
        assert_ne!(row.id, old.id, "new tuple must get a new row ID");
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn split_keeps_future_and_due_ids_and_retires_emitted_rows(pool: PgPool) {
    for explicit in [false, true] {
        for due in [false, true] {
            let (h, item) = fixture(&pool, explicit).await;
            let occurrences: Vec<Uuid> = sqlx::query_scalar(
                "select id from porchlight.item_occurrences where item_id=$1 and seq>=1",
            )
            .bind(item)
            .fetch_all(&pool)
            .await
            .unwrap();
            let mut before: Vec<_> = scheduled(&pool, item)
                .await
                .into_iter()
                .filter(|r| occurrences.contains(&r.occurrence_id))
                .collect();
            assert_eq!(before.len(), 4);
            let now = at(if due {
                "2026-10-06T11:46:00Z"
            } else {
                "2026-10-05T10:01:00Z"
            });
            assert_eq!(
                before.iter().filter(|r| r.fire_at <= now).count(),
                if due { 2 } else { 0 }
            );
            let mut tx = tx::begin(&pool).await.unwrap();
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
            scope.finish(now).await.unwrap();
            tx.commit().await.unwrap();
            for row in &mut before {
                row.item_id = tail;
            }
            assert_eq!(
                scheduled(&pool, tail).await,
                before,
                "split must reparent without changing reminder row IDs"
            );
            regenerate(&pool, tail, now).await;
            assert_eq!(scheduled(&pool, tail).await, before);

            for _ in 0..2 {
                let mut tx = tx::begin(&pool).await.unwrap();
                notifier::reminders(&mut tx, now).await.unwrap();
                tx.commit().await.unwrap();
            }
            let emitted: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_events where item_id=$1 and kind='reminder'")
                .bind(tail).fetch_one(&pool).await.unwrap();
            assert_eq!(
                emitted,
                if due { 2 } else { 0 },
                "split must retain the dedupe witness"
            );
            regenerate(&pool, tail, now).await;
            before.retain(|r| r.fire_at > now);
            assert_eq!(
                scheduled(&pool, tail).await,
                before,
                "emitted due rows must be retired; future rows stay stable"
            );
        }
    }
}
