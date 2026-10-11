use chrono::{DateTime, Utc};
use porchlight_core::items as core;
use porchlight_db::{
    derive, items,
    testing::{Role, insert_appointment, pool_as, seed_household},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
use uuid::Uuid;
fn now() -> DateTime<Utc> {
    "2026-10-05T12:00:00Z".parse().unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn thousand_cards_local_tie_does_not_rewrite_column(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let keys = porchlight_core::ordering::rebalance(1000);
    for (seq, key) in keys.iter().enumerate() {
        let key = if (498..=500).contains(&seq) {
            &keys[499]
        } else {
            key
        };
        sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,board_position,status,completed_at) values($1,$2,$3,$4,'done',now())").bind(h.id).bind(id).bind(seq as i32).bind(key).execute(&pool).await.unwrap();
    }
    let rows:Vec<(Uuid,i32)>=sqlx::query_as("select id,version from porchlight.item_occurrences where item_id=$1 and seq between 498 and 500 order by board_position collate \"C\",id").bind(id).fetch_all(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::move_on_board(
        &mut tx,
        h.id,
        rows[2].0,
        rows[2].1,
        Some(rows[0].0),
        Some(rows[1].0),
        now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let changed: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.item_occurrences where item_id=$1 and version>1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        changed <= 3,
        "whole-column rebalance touched {changed} rows instead of local run"
    );
    let ordered:Vec<Uuid>=sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1 and seq between 498 and 500 order by board_position collate \"C\",id").bind(id).fetch_all(&pool).await.unwrap();
    assert_eq!(ordered, vec![rows[0].0, rows[2].0, rows[1].0]);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn thousand_equal_80_cards_order_exactly_and_updates_bounded_by_run(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,status,completed_at) select $1,$2,n,'done',now() from generate_series(0,999) n").bind(h.id).bind(id).execute(&pool).await.unwrap();
    let rows:Vec<(Uuid,i32)>=sqlx::query_as("select id,version from porchlight.item_occurrences where item_id=$1 order by board_position collate \"C\",id").bind(id).fetch_all(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    items::move_on_board(
        &mut tx,
        h.id,
        rows[999].0,
        rows[999].1,
        Some(rows[498].0),
        Some(rows[499].0),
        now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut expected: Vec<_> = rows.iter().map(|r| r.0).collect();
    let mover = expected.pop().unwrap();
    expected.insert(499, mover);
    let ordered:Vec<Uuid>=sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1 order by board_position collate \"C\",id").bind(id).fetch_all(&pool).await.unwrap();
    assert_eq!(ordered, expected);
    let changed: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.item_occurrences where item_id=$1 and version>1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        changed, 999,
        "only changed keys and the mover are updated; the unchanged middle bystander stays version 1"
    );
    let events:i64=sqlx::query_scalar("select count(*) from porchlight.outbox where kind='occurrence.changed' and entity_id=any($1)").bind(expected).fetch_one(&pool).await.unwrap();
    let key_changes:i64=sqlx::query_scalar("select count(*) from porchlight.item_occurrences where item_id=$1 and board_position<>'80'").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(events, key_changes, "one event per actual key change");
    assert_eq!(events, changed, "no silent bystander version bumps");
    let moved: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.item_occurrences where item_id=$1 and board_moved",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(moved, 1, "only the explicit mover is touched for R31");
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn column_drag_finishes_status_and_position_together(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let fields = items::ItemWrite {
        owner_id: h.user,
        title: "card".into(),
        ..Default::default()
    };
    let mut rows = Vec::new();
    for _ in 0..3 {
        let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
        let made = items::create(&mut tx, h.id, &fields, &Default::default(), now())
            .await
            .unwrap();
        rows.push(derive::occurrences(&mut tx, made.item_id).await.unwrap()[0].clone());
        tx.commit().await.unwrap();
    }
    rows.sort_by_key(|o| o.id);
    sqlx::query("update porchlight.item_occurrences set status='doing' where id=any($1)")
        .bind(vec![rows[0].id, rows[1].id])
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let mut scope = items::WriteScope::begin(
        &mut tx,
        h.id,
        items::ScopeRequest {
            occurrences: vec![rows[2].id],
            board_moves: vec![items::BoardMove {
                occurrence: rows[2].id,
                before: Some(rows[0].id),
                after: Some(rows[1].id),
                status: core::Status::Doing,
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    scope
        .set_status(
            rows[2].id,
            rows[2].version,
            core::Status::Doing,
            None,
            now(),
        )
        .await
        .unwrap();
    let version = scope.occurrence(rows[2].id).await.unwrap().version;
    scope
        .move_on_board(rows[2].id, version, Some(rows[0].id), Some(rows[1].id))
        .await
        .unwrap();
    assert_eq!(scope.finish(now()).await.unwrap().len(), 1);
    tx.commit().await.unwrap();
    let ordered:Vec<Uuid>=sqlx::query_scalar("select id from porchlight.item_occurrences where household_id=$1 and status='doing' order by board_position collate \"C\",id").bind(h.id).fetch_all(&pool).await.unwrap();
    assert_eq!(ordered, vec![rows[0].id, rows[2].id, rows[1].id]);
    let pending: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.item_derivation where applied_gen<requested_gen",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pending, 0);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn composed_move_revalidates_bounds_after_status_edit(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let mut ids = vec![id];
    for _ in 0..2 {
        ids.push(insert_appointment(&pool, &h).await.unwrap());
    }
    sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq) select $1,unnest($2::uuid[]),0").bind(h.id).bind(&ids).execute(&pool).await.unwrap();
    let rows: Vec<(Uuid, i32)> = sqlx::query_as(
        "select id,version from porchlight.item_occurrences where item_id=any($1) order by id",
    )
    .bind(ids)
    .fetch_all(&pool)
    .await
    .unwrap();
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut tx = begin_as(&app, &Actor::system()).await.unwrap();
    let mut scope = items::WriteScope::begin(
        &mut tx,
        h.id,
        items::ScopeRequest {
            occurrences: vec![rows[0].0, rows[2].0],
            board_moves: vec![items::BoardMove {
                occurrence: rows[2].0,
                before: Some(rows[0].0),
                after: Some(rows[1].0),
                status: core::Status::Todo,
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    scope
        .set_status(rows[0].0, rows[0].1, core::Status::Doing, None, now())
        .await
        .unwrap();
    assert!(
        matches!(
            scope
                .move_on_board(rows[2].0, rows[2].1, Some(rows[0].0), Some(rows[1].0))
                .await,
            Err(derive::Error::RetryableScope)
        ),
        "changed preplanned bound must be retryable"
    );
    drop(scope);
    tx.rollback().await.unwrap();
}
