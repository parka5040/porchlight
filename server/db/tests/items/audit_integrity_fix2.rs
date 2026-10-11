use chrono::{DateTime, Utc};
use porchlight_db::{
    derive,
    items::{ItemWrite, ScopeRequest, WriteScope},
    testing::{Role, insert_appointment, pool_as, seed_household},
};
use sqlx::PgPool;
use uuid::Uuid;

async fn snapshot(pool: &PgPool, item: Uuid) -> serde_json::Value {
    sqlx::query_scalar("select jsonb_build_object('item',(select to_jsonb(i) from porchlight.items i where id=$1),'reminders',(select jsonb_agg(to_jsonb(r) order by id) from porchlight.item_reminders r where item_id=$1),'derivation',(select to_jsonb(d) from porchlight.item_derivation d where item_id=$1),'outbox',(select count(*) from porchlight.outbox))")
        .bind(item).fetch_one(pool).await.unwrap()
}

async fn reject_collision(pool: PgPool, first_null: bool, second_null: bool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let agent = pool_as(&pool, Role::Agent, Some("agent:audit-integrity-fix2"))
        .await
        .unwrap();
    let identity = Uuid::new_v4();
    let first = if first_null { identity } else { Uuid::new_v4() };
    let second = if second_null {
        identity
    } else {
        Uuid::new_v4()
    };
    // Distinct physical IDs and offsets must not hide an effective-ID collision.
    sqlx::query("insert into porchlight.item_reminders(id,household_id,item_id,offset_minutes,delivery_id) values($1,$2,$3,15,$4)")
        .bind(first).bind(h.id).bind(item).bind((!first_null).then_some(identity))
        .execute(&agent).await.unwrap();
    let before = snapshot(&pool, item).await;
    let result = sqlx::query("insert into porchlight.item_reminders(id,household_id,item_id,offset_minutes,delivery_id) values($1,$2,$3,30,$4)")
        .bind(second).bind(h.id).bind(item).bind((!second_null).then_some(identity))
        .execute(&agent).await;
    assert!(
        result.is_err(),
        "agent accepted duplicate effective reminder identity"
    );
    let error = result.unwrap_err();
    let db = error.as_database_error().unwrap();
    assert_eq!(db.code().as_deref(), Some("23505"));
    assert_eq!(db.constraint(), Some("item_reminders_delivery_identity"));
    assert_eq!(
        snapshot(&pool, item).await,
        before,
        "rejection must preserve parent version, reminders, derivation and outbox"
    );
    agent.close().await;
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn agent_duplicate_delivery_id_is_atomic(pool: PgPool) {
    reject_collision(pool, false, false).await;
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn agent_null_to_explicit_alias_is_atomic(pool: PgPool) {
    reject_collision(pool, true, false).await;
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn agent_explicit_to_null_alias_is_atomic(pool: PgPool) {
    reject_collision(pool, false, true).await;
}

async fn offsets(pool: &PgPool, item: Uuid, member: Uuid) -> Vec<(Uuid, i32)> {
    sqlx::query_as("select id,offset_minutes from porchlight.pl_reminder_offsets($1,$2) order by offset_minutes")
        .bind(item).bind(member).fetch_all(pool).await.unwrap()
}

async fn scheduled(pool: &PgPool) -> Vec<(Uuid, Uuid, Uuid, DateTime<Utc>)> {
    sqlx::query_as("select member_id,occurrence_id,reminder_id,fire_at from porchlight.scheduled_reminders order by member_id,occurrence_id,reminder_id")
        .fetch_all(pool).await.unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn multi_offset_split_and_regeneration_keep_delivery_identities(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set starts_at='2026-10-05 12:00Z',ends_at='2026-10-05 13:00Z',rrule='FREQ=DAILY;COUNT=3',recurrence_mode='calendar' where id=$1")
        .bind(item).execute(&pool).await.unwrap();
    let agent = pool_as(&pool, Role::Agent, Some("agent:audit-integrity-fix2"))
        .await
        .unwrap();
    // Exercise both legacy NULL identities and explicit aliases in the same split.
    for (offset, identity) in [(15, None), (30, Some(Uuid::new_v4()))] {
        sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes,delivery_id) values($1,$2,$3,$4)")
            .bind(h.id).bind(item).bind(offset).bind(identity).execute(&agent).await.unwrap();
    }
    let now: DateTime<Utc> = "2026-10-05T10:00:00Z".parse().unwrap();
    let mut tx = pool.begin().await.unwrap();
    derive::derive_item(&mut tx, item, now).await.unwrap();
    tx.commit().await.unwrap();
    let identities = offsets(&pool, item, h.super_admin).await;
    assert_eq!(identities.len(), 2);
    assert_ne!(identities[0].0, identities[1].0);
    let before = scheduled(&pool).await;
    assert_eq!(before.len(), 6, "each of three occurrences has two offsets");

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

    for id in [item, tail] {
        assert_eq!(offsets(&pool, id, h.super_admin).await, identities);
    }
    assert_eq!(scheduled(&pool).await, before);
    // Rebuild both projections again rather than only testing transferred rows.
    let mut tx = pool.begin().await.unwrap();
    for id in [item, tail] {
        derive::derive_item(&mut tx, id, now).await.unwrap();
    }
    tx.commit().await.unwrap();
    assert_eq!(scheduled(&pool).await, before);
    let counts: Vec<(Uuid, i64)> = sqlx::query_as("select item_id,count(*) from porchlight.scheduled_reminders group by item_id order by item_id")
        .fetch_all(&pool).await.unwrap();
    assert_eq!(counts.iter().find(|(id, _)| *id == item).unwrap().1, 2);
    assert_eq!(counts.iter().find(|(id, _)| *id == tail).unwrap().1, 4);
    let pending: i64 = sqlx::query_scalar("select count(*) from porchlight.item_derivation where item_id=any($1) and applied_gen<requested_gen")
        .bind(vec![item, tail]).fetch_one(&pool).await.unwrap();
    assert_eq!(pending, 0);
    agent.close().await;
}
