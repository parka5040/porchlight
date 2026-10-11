use porchlight_db::{
    derive,
    testing::{Role, insert_appointment, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn audit_limits_legacy_series_derives_bounded_history_and_acks(pool: PgPool) {
    let household = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &household).await.unwrap();
    sqlx::query("update porchlight.items set starts_at='0001-01-01T14:00:00Z',ends_at='0001-01-01T15:00:00Z',tzid='UTC',recurrence_mode='calendar',rrule='FREQ=DAILY' where id=$1")
        .bind(id).execute(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    sqlx::query("select id from porchlight.items where id=$1 for update")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    derive::derive_item(&mut tx, id, "2026-10-11T00:00:00Z".parse().unwrap())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (count, first, last): (i64, i32, i32) = sqlx::query_as(
        "select count(*),min(seq),max(seq) from porchlight.item_occurrences where item_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((count, first, last), (10000, 0, 9999));
    let ack: bool = sqlx::query_scalar(
        "select requested_gen=applied_gen from porchlight.item_derivation where item_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(ack);
}
