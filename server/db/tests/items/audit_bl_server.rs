use porchlight_db::{
    items::{self, ScopeRequest, WriteScope},
    testing::{Role, insert_appointment, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "./migrations")]
async fn member_zone_scope_requires_enqueue_only(pool: PgPool) {
    let mut tx = porchlight_db::tx::begin(&pool).await.unwrap();
    let result = WriteScope::begin(
        &mut tx,
        Uuid::now_v7(),
        ScopeRequest {
            member_zones: vec![Uuid::now_v7()],
            ..Default::default()
        },
    )
    .await;
    assert!(
        matches!(result, Err(porchlight_db::derive::Error::Validation(ref error)) if error.field == "scope"),
        "member-zone scopes without enqueue_only must fail scope validation"
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn unchanged_explicit_board_move_is_a_true_noop(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let id: Uuid = sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq,status,completed_at) values($1,$2,0,'done',now()) returning id")
        .bind(h.id).bind(item).fetch_one(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::move_on_board(&mut tx, h.id, id, 1, None, None, chrono::Utc::now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let state: (i32, bool, i64) = sqlx::query_as("select version,board_moved,(select count(*) from porchlight.outbox where kind='occurrence.changed' and entity_id=$1) from porchlight.item_occurrences where id=$1")
        .bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(
        state,
        (1, false, 0),
        "unchanged move must not silently bump version or marker"
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn raw_agent_cannot_spoof_board_respreader(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let id: Uuid = sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq,status,completed_at) values($1,$2,0,'done',now()) returning id")
        .bind(h.id).bind(item).fetch_one(&pool).await.unwrap();
    let agent = pool_as(&pool, Role::Agent, Some("agent:fixture"))
        .await
        .unwrap();
    let mut tx = porchlight_db::tx::begin(&agent).await.unwrap();
    sqlx::query("select set_config('porchlight.board_mover',$1,true)")
        .bind(Uuid::nil().to_string())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("update porchlight.item_occurrences set board_position=$2 where id=$1")
        .bind(id)
        .bind(porchlight_core::ordering::key_after("80").unwrap())
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let moved: bool =
        sqlx::query_scalar("select board_moved from porchlight.item_occurrences where id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        moved,
        "raw agent board writes are explicit moves regardless of caller-settable GUC"
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn meaningful_respread_publishes_mover_even_if_its_key_is_unchanged(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let mut keys: Vec<_> = (1..=4)
        .map(|i| format!("{}{i:02x}80", "00".repeat(30)))
        .collect();
    keys.extend(["80".into(), "f080".into()]);
    let now = chrono::Utc::now();
    let mut ids = Vec::new();
    for (seq, key) in keys.iter().enumerate() {
        let id: Uuid = sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq,status,completed_at,board_position) values($1,$2,$3,'done',$4,$5) returning id")
            .bind(h.id).bind(item).bind(seq as i32).bind(now).bind(key).fetch_one(&pool).await.unwrap();
        ids.push(id);
    }
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::move_on_board(&mut tx, h.id, ids[4], 1, Some(ids[1]), Some(ids[2]), now)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let state: (String, bool, i32, i64) = sqlx::query_as("select board_position,board_moved,version,(select count(*) from porchlight.outbox where kind='occurrence.changed' and entity_id=$1) from porchlight.item_occurrences where id=$1")
        .bind(ids[4]).fetch_one(&pool).await.unwrap();
    assert_eq!(
        state,
        ("80".into(), true, 2, 1),
        "a marker-only change needs exactly one outbox event"
    );
}
