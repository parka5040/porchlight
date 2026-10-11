use porchlight_db::testing::{insert_appointment, seed_household};
use sqlx::PgPool;

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn prefs_seed_and_task_routing(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let seeded: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.notification_prefs where household_id=$1",
    )
    .bind(h.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(seeded, 3);
    let item = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query(
        "insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)",
    )
    .bind(h.id)
    .bind(item)
    .bind(h.user)
    .execute(&pool)
    .await
    .unwrap();
    let recipients: Vec<uuid::Uuid> = sqlx::query_scalar("select recipient_member_id from porchlight.notification_events where item_id=$1 and kind='assigned'")
        .bind(item).fetch_all(&pool).await.unwrap();
    assert_eq!(recipients, vec![h.user]);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn self_assignment_agent_status_completed_and_terminal(pool: PgPool) {
    use porchlight_db::{
        testing::{Role, pool_as},
        tx::{Actor, begin_as},
    };
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(
        &app,
        &Actor {
            member_id: Some(h.user),
            device_id: None,
            request_id: uuid::Uuid::now_v7(),
            agent_label: None,
            token_id: None,
        },
    )
    .await
    .unwrap();
    sqlx::query(
        "insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)",
    )
    .bind(h.id)
    .bind(item)
    .bind(h.user)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "select count(*) from porchlight.notification_events where kind='assigned'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    let occ:uuid::Uuid=sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq) values($1,$2,0) returning id").bind(h.id).bind(item).fetch_one(&pool).await.unwrap();
    let agent = pool_as(&pool, Role::Agent, Some("agent:garden-helper"))
        .await
        .unwrap();
    sqlx::query("update porchlight.item_occurrences set status='doing' where id=$1")
        .bind(occ)
        .execute(&agent)
        .await
        .unwrap();
    sqlx::query(
        "update porchlight.item_occurrences set status='done',completed_at=now() where id=$1",
    )
    .bind(occ)
    .execute(&agent)
    .await
    .unwrap();
    let events:Vec<(String,uuid::Uuid,Option<String>)>=sqlx::query_as("select kind,recipient_member_id,actor_label from porchlight.notification_events order by created_at,id").fetch_all(&pool).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].0, "status_changed");
    assert_eq!(events[1].0, "completed");
    for (_, recipient, label) in events {
        assert_eq!(recipient, h.super_admin);
        assert_eq!(label.as_deref(), Some("agent:garden-helper"));
    }
    sqlx::query("update porchlight.item_derivation set attempts=5,last_error='not included' where item_id=$1").bind(item).execute(&pool).await.unwrap();
    sqlx::query("update porchlight.item_derivation set attempts=5 where item_id=$1")
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    let alerts:Vec<(uuid::Uuid,Option<uuid::Uuid>,serde_json::Value)>=sqlx::query_as("select recipient_member_id,item_id,payload from porchlight.notification_events where kind='derivation_error'").fetch_all(&pool).await.unwrap();
    assert_eq!(alerts, vec![(h.super_admin, None, serde_json::json!({}))]);
}
async fn apply(pool: &PgPool, id: uuid::Uuid, now: chrono::DateTime<chrono::Utc>) {
    let mut tx = porchlight_db::tx::begin_as(pool, &porchlight_db::tx::Actor::system())
        .await
        .unwrap();
    sqlx::query("select id from porchlight.items where id=$1 for update")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    porchlight_db::derive::derive_item(&mut tx, id, now)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn default_offsets_enqueue_and_explicit_override_conflict_dedupe(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let before: i64 =
        sqlx::query_scalar("select requested_gen from porchlight.item_derivation where item_id=$1")
            .bind(item)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("update porchlight.notification_prefs set default_appointment_offsets='{15,60}' where member_id=$1").bind(h.super_admin).execute(&pool).await.unwrap();
    let after: i64 =
        sqlx::query_scalar("select requested_gen from porchlight.item_derivation where item_id=$1")
            .bind(item)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(after, before + 1);
    let now = "2026-10-05T12:00:00Z".parse().unwrap();
    apply(&pool, item, now).await;
    let reminders:Vec<uuid::Uuid>=sqlx::query_scalar("select reminder_id from porchlight.scheduled_reminders where item_id=$1 order by reminder_id").bind(item).fetch_all(&pool).await.unwrap();
    assert_eq!(reminders.len(), 2);
    apply(&pool, item, now).await;
    let again:Vec<uuid::Uuid>=sqlx::query_scalar("select reminder_id from porchlight.scheduled_reminders where item_id=$1 order by reminder_id").bind(item).fetch_all(&pool).await.unwrap();
    assert_eq!(again, reminders);
    let explicit:uuid::Uuid=sqlx::query_scalar("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,5) returning id").bind(h.id).bind(item).fetch_one(&pool).await.unwrap();
    apply(&pool, item, now).await;
    assert_eq!(
        sqlx::query_scalar::<_, uuid::Uuid>(
            "select reminder_id from porchlight.scheduled_reminders where item_id=$1"
        )
        .bind(item)
        .fetch_one(&pool)
        .await
        .unwrap(),
        explicit
    );
    let second = insert_appointment(&pool, &h).await.unwrap();
    apply(&pool, second, now).await;
    apply(&pool, item, now).await;
    apply(&pool, second, now).await;
    let conflicts: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.notification_events where kind='conflict'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(conflicts, 1);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn pending_counterpart_cannot_emit_a_phantom_conflict(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let a = insert_appointment(&pool, &h).await.unwrap();
    let b = insert_appointment(&pool, &h).await.unwrap();
    let now = "2026-10-05T12:00:00Z".parse().unwrap();
    sqlx::query("update porchlight.items set starts_at=starts_at+interval '2 hours',ends_at=ends_at+interval '2 hours' where id=$1").bind(b).execute(&pool).await.unwrap();
    apply(&pool, a, now).await;
    apply(&pool, b, now).await;
    // Both inputs changed before projection: A moves into B's OLD slot; B moved out.
    sqlx::query("update porchlight.items set starts_at=starts_at+interval '2 hours',ends_at=ends_at+interval '2 hours' where id=any($1)").bind(vec![a,b]).execute(&pool).await.unwrap();
    apply(&pool, a, now).await;
    apply(&pool, b, now).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "select count(*) from porchlight.notification_events where kind='conflict'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn reopened_maps_to_status_and_agent_cannot_forge_event_clock(pool: PgPool) {
    use porchlight_db::testing::{Role, pool_as};
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let occ:uuid::Uuid=sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq) values($1,$2,0) returning id").bind(h.id).bind(item).fetch_one(&pool).await.unwrap();
    let agent = pool_as(&pool, Role::Agent, Some("agent:clock-test"))
        .await
        .unwrap();
    let mut tx = agent.begin().await.unwrap();
    sqlx::query("select set_config('porchlight.notification_now','1999-01-01T00:00:00Z',true)")
        .execute(&mut *tx)
        .await
        .unwrap();
    let transaction_now: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("select now()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "update porchlight.item_occurrences set status='done',completed_at=now() where id=$1",
    )
    .bind(occ)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "update porchlight.item_occurrences set status='todo',completed_at=null where id=$1",
    )
    .bind(occ)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let kinds: Vec<String> = sqlx::query_scalar(
        "select kind from porchlight.notification_events where item_id=$1 order by id",
    )
    .bind(item)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(kinds, vec!["completed", "status_changed"]);
    let dates: Vec<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "select created_at from porchlight.notification_events where item_id=$1",
    )
    .bind(item)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(dates.iter().all(|date| *date == transaction_now));
    sqlx::query("update porchlight.item_derivation set attempts=5 where item_id=$1")
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    assert!(sqlx::query_scalar::<_,bool>("select entity_id<>$1 and item_id is null and occurrence_id is null from porchlight.notification_events where kind='derivation_error'").bind(item).fetch_one(&pool).await.unwrap());
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn same_item_recurrence_overlaps_emit_conflicts_while_deriving(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set recurrence_mode='calendar',rrule='FREQ=DAILY;COUNT=3',ends_at=starts_at+interval '25 hours' where id=$1")
        .bind(item).execute(&pool).await.unwrap();
    let now = "2026-10-05T12:00:00Z".parse().unwrap();
    apply(&pool, item, now).await;
    assert_eq!(sqlx::query_scalar::<_, i64>("select count(*) from porchlight.notification_events where item_id=$1 and kind='conflict'")
        .bind(item).fetch_one(&pool).await.unwrap(), 2);
    apply(&pool, item, now).await;
    assert_eq!(sqlx::query_scalar::<_, i64>("select count(*) from porchlight.notification_events where item_id=$1 and kind='conflict'")
        .bind(item).fetch_one(&pool).await.unwrap(), 2);
}
