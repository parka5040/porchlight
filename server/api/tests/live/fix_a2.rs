use chrono::{DateTime, Utc};
use porchlight_api::derive_worker;
use porchlight_db::testing::{Role, insert_appointment, pool_as, seed_household};
use sqlx::PgPool;
use uuid::Uuid;
fn now() -> DateTime<Utc> {
    "2026-10-05T12:00:00Z".parse().unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn worker_does_not_lock_unrelated_members_or_closed_history(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let closed:Uuid=sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq,status,completed_at) values($1,$2,10,'done',$3) returning id").bind(h.id).bind(id).bind(now()).fetch_one(&pool).await.unwrap();
    sqlx::query("update porchlight.item_derivation set retry_at=$1")
        .bind(now())
        .execute(&pool)
        .await
        .unwrap();
    let mut held = pool.begin().await.unwrap();
    sqlx::query(
        "select id from porchlight.household_members where id=any($1) order by id for update",
    )
    .bind(vec![h.user, h.dependent])
    .fetch_all(&mut *held)
    .await
    .unwrap();
    sqlx::query("select id from porchlight.item_occurrences where id=$1 for update")
        .bind(closed)
        .fetch_one(&mut *held)
        .await
        .unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    assert_eq!(
        derive_worker::run_batch(&app, now()).await.unwrap(),
        1,
        "unrelated member and closed-history locks must not make candidate unavailable"
    );
    let state: (i64, i64) = sqlx::query_as(
        "select requested_gen,applied_gen from porchlight.item_derivation where item_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state.0, state.1);
}
