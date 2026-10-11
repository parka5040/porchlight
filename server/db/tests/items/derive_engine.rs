use chrono::{DateTime, Utc};
use porchlight_db::{
    derive,
    testing::{Role, insert_appointment, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
fn now() -> DateTime<Utc> {
    "2026-10-05T12:00:00Z".parse().unwrap()
}
async fn apply(pool: &PgPool, id: uuid::Uuid) {
    let app = pool_as(pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    sqlx::query("select id from porchlight.items where id=$1 for update")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    derive::derive_item(&mut tx, id, now()).await.unwrap();
    tx.commit().await.unwrap();
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn generation_agenda_ack_and_event(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    apply(&pool, id).await;
    let state: (i64, i64) = sqlx::query_as(
        "select requested_gen,applied_gen from porchlight.item_derivation where item_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state.0, state.1);
    let count: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.member_agenda where item_id=$1 and busy is not null",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let event: serde_json::Value = sqlx::query_scalar(
        "select payload from porchlight.outbox where entity_id=$1 and kind='derived'",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(event["applied_gen"], state.1);
    assert_eq!(event["item_id"], id.to_string());
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn regeneration_preserves_closed_detached_and_deletes_rule_removed(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set rrule='FREQ=DAILY;COUNT=5',recurrence_mode='calendar' where id=$1").bind(id).execute(&pool).await.unwrap();
    apply(&pool, id).await;
    sqlx::query("update porchlight.item_occurrences set status='done',completed_at='2026-10-12T19:00Z' where item_id=$1 and seq=1").bind(id).execute(&pool).await.unwrap();
    sqlx::query("update porchlight.item_occurrences set override_local_start='2026-10-18 14:00',override_local_end='2026-10-18 15:00' where item_id=$1 and seq=2").bind(id).execute(&pool).await.unwrap();
    let kept_before:Vec<serde_json::Value>=sqlx::query_scalar("select to_jsonb(o) from porchlight.item_occurrences o where item_id=$1 and seq in (1,2) order by seq").bind(id).fetch_all(&pool).await.unwrap();
    sqlx::query("update porchlight.items set rrule='FREQ=DAILY;COUNT=1' where id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    apply(&pool, id).await;
    let seqs: Vec<i32> = sqlx::query_scalar(
        "select seq from porchlight.item_occurrences where item_id=$1 order by seq",
    )
    .bind(id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(seqs, vec![0, 1, 2]);
    let kept_after:Vec<serde_json::Value>=sqlx::query_scalar("select to_jsonb(o) from porchlight.item_occurrences o where item_id=$1 and seq in (1,2) order by seq").bind(id).fetch_all(&pool).await.unwrap();
    assert_eq!(
        kept_after, kept_before,
        "kept closed/detached anchors and all state must be untouched"
    );
    sqlx::query("update porchlight.items set deleted_at=now() where id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    apply(&pool, id).await;
    let count: i64 =
        sqlx::query_scalar("select count(*) from porchlight.member_agenda where item_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn floating_zones_reminders_and_gap_override(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    sqlx::query("update porchlight.household_members set tzid='America/Los_Angeles' where id=$1")
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    let id:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,time_mode,local_start,local_end) values($1,$2,'Floating','appointment','floating','2026-10-12 14:00','2026-10-12 15:00') returning id").bind(h.id).bind(h.super_admin).fetch_one(&pool).await.unwrap();
    for m in [h.super_admin, h.user] {
        sqlx::query("insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)").bind(h.id).bind(id).bind(m).execute(&pool).await.unwrap();
    }
    sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,30)").bind(h.id).bind(id).execute(&pool).await.unwrap();
    apply(&pool, id).await;
    let times: Vec<DateTime<Utc>> = sqlx::query_scalar(
        "select starts_at from porchlight.member_agenda where item_id=$1 order by starts_at",
    )
    .bind(id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        times,
        vec![
            "2026-10-12T18:00:00Z".parse::<DateTime<Utc>>().unwrap(),
            "2026-10-12T21:00:00Z".parse::<DateTime<Utc>>().unwrap()
        ]
    );
    let count: i64 =
        sqlx::query_scalar("select count(*) from porchlight.scheduled_reminders where item_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 2);
    sqlx::query("update porchlight.household_members set tzid='America/New_York' where id=$1")
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    apply(&pool, id).await;
    let fires: Vec<DateTime<Utc>> =
        sqlx::query_scalar("select fire_at from porchlight.scheduled_reminders where item_id=$1")
            .bind(id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(
        fires
            .iter()
            .all(|t| *t == "2026-10-12T17:30:00Z".parse::<DateTime<Utc>>().unwrap())
    );
    // Stored local overrides use the same R10 correction as core expansion and agendas.
    sqlx::query("update porchlight.item_occurrences set override_local_start='2026-03-08 02:10',override_local_end='2026-03-08 02:50' where item_id=$1").bind(id).execute(&pool).await.unwrap();
    apply(&pool, id).await;
    let (s, e): (DateTime<Utc>, DateTime<Utc>) = sqlx::query_as(
        "select starts_at,ends_at from porchlight.member_agenda where item_id=$1 limit 1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(e - s, chrono::Duration::minutes(40));
}
