use porchlight_db::{
    derive,
    testing::{Role, insert_appointment, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn capped_history_preserves_live_state_and_reports_warning(pool: PgPool) {
    let household = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &household).await.unwrap();
    sqlx::query("update porchlight.items set starts_at='1990-01-01T14:00:00Z',ends_at='1990-01-01T15:00:00Z',tzid='UTC',recurrence_mode='calendar',rrule='FREQ=DAILY' where id=$1")
        .bind(item).execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,starts_at,ends_at,status,board_position,board_moved) select household_id,id,s,'2026-10-11T14:00:00Z'::timestamptz,'2026-10-11T15:00:00Z'::timestamptz,case s when 13432 then 'doing' when 13433 then 'backlog' else 'todo' end,'8180',true from porchlight.items cross join generate_series(13432,13434) s where id=$1")
        .bind(item).execute(&pool).await.unwrap();
    let before: Vec<serde_json::Value> = sqlx::query_scalar(
        "select to_jsonb(o) from porchlight.item_occurrences o where item_id=$1 order by seq",
    )
    .bind(item)
    .fetch_all(&pool)
    .await
    .unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    for error in [None, Some("existing real failure")] {
        sqlx::query("update porchlight.item_derivation set last_error=$2 where item_id=$1")
            .bind(item)
            .bind(error)
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        sqlx::query("select id from porchlight.items where id=$1 for update")
            .bind(item)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        derive::derive_item(&mut tx, item, "2026-10-11T00:00:00Z".parse().unwrap())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let after: Vec<serde_json::Value> = sqlx::query_scalar("select to_jsonb(o) from porchlight.item_occurrences o where item_id=$1 and seq>=10000 order by seq")
            .bind(item).fetch_all(&pool).await.unwrap();
        assert_eq!(
            after, before,
            "capped expansion must preserve live occurrence state"
        );
        let warning: Option<String> = sqlx::query_scalar(
            "select last_error from porchlight.item_derivation where item_id=$1",
        )
        .bind(item)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(warning.is_some(), "capped derivation must expose a warning");
        if let Some(error) = error {
            assert_eq!(warning.as_deref(), Some(error));
        }
    }
}
