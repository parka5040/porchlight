use porchlight_db::testing::{insert_appointment, seed_household};
use sqlx::PgPool;
use std::{
    borrow::Cow,
    time::{Duration, Instant},
};
use uuid::Uuid;

async fn snapshot(pool: &PgPool) -> serde_json::Value {
    sqlx::query_scalar("select jsonb_build_object('board',(select jsonb_agg(jsonb_build_array(id,board_position,version) order by id) from porchlight.item_occurrences),'list',(select jsonb_agg(jsonb_build_array(id,position,version) order by id) from porchlight.list_entries),'outbox',(select count(*) from porchlight.outbox))")
        .fetch_one(pool).await.unwrap()
}

async fn ordering_migration_writer(pool: PgPool, board: bool) {
    let h = seed_household(&pool).await.unwrap();
    sqlx::raw_sql("alter table porchlight.item_occurrences drop constraint item_occurrences_board_position_canonical; alter table porchlight.list_entries drop constraint list_entries_position_canonical; delete from public._sqlx_migrations where version=67;")
        .execute(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let occurrence: Uuid = sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq,board_position) values($1,$2,0,'a0') returning id")
        .bind(h.id).bind(item).fetch_one(&pool).await.unwrap();
    let list: Uuid = sqlx::query_scalar("insert into porchlight.lists(household_id,owner_id,name) values($1,$2,'Legacy list') returning id")
        .bind(h.id).bind(h.super_admin).fetch_one(&pool).await.unwrap();
    let entry: Uuid = sqlx::query_scalar("insert into porchlight.list_entries(household_id,list_id,label,position) values($1,$2,'Legacy entry','a0') returning id")
        .bind(h.id).bind(list).fetch_one(&pool).await.unwrap();
    // A canonical collection must not be respread or acquire revisions/outbox writes.
    let other = seed_household(&pool).await.unwrap();
    let valid_item = insert_appointment(&pool, &other).await.unwrap();
    let valid_occurrence: Uuid = sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq,board_position) values($1,$2,0,'80') returning id")
        .bind(other.id).bind(valid_item).fetch_one(&pool).await.unwrap();
    let before = snapshot(&pool).await;
    let migration = porchlight_db::MIGRATOR
        .iter()
        .find(|m| m.version == 67)
        .unwrap()
        .clone();
    let migrator = sqlx::migrate::Migrator {
        migrations: Cow::Owned(vec![migration]),
        ignore_missing: true,
        ..sqlx::migrate::Migrator::DEFAULT
    };
    let mut writer = pool.begin().await.unwrap();
    sqlx::query("set local role porchlight_agent")
        .execute(&mut *writer)
        .await
        .unwrap();
    if board {
        sqlx::query("update porchlight.item_occurrences set board_position='4080' where id=$1")
            .bind(occurrence)
            .execute(&mut *writer)
            .await
            .unwrap();
    } else {
        sqlx::query("update porchlight.list_entries set position='4080' where id=$1")
            .bind(entry)
            .execute(&mut *writer)
            .await
            .unwrap();
    }
    let holder: i32 = sqlx::query_scalar("select pg_backend_pid()")
        .fetch_one(&mut *writer)
        .await
        .unwrap();
    let migration_pool = pool.clone();
    let started = Instant::now();
    let task = async move {
        let mut conn = migration_pool.acquire().await.unwrap();
        let result = migrator.run(&mut *conn).await;
        // The no-transaction migration owns its BEGIN; discard an aborted session
        // and its migration advisory lock just as a failed one-shot migrator exits.
        conn.close().await.unwrap();
        (result, migrator)
    };
    let waiting = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let mode: Option<String> = sqlx::query_scalar("select l.mode from pg_locks l where not l.granted and $1=any(pg_blocking_pids(l.pid)) order by l.mode limit 1")
                .bind(holder).fetch_optional(&pool).await.unwrap();
            if let Some(mode) = mode {
                break mode;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });
    let (task, waiting) = tokio::join!(tokio::time::timeout(Duration::from_secs(8), task), waiting);
    let waiting_mode = waiting.expect("migration must wait behind the concurrent writer");
    let (result, migrator) = task.expect("migration must fail fast");
    let error = result.expect_err("held writer must abort migration");
    match error {
        sqlx::migrate::MigrateError::ExecuteMigration(error, 67) => {
            let db = error.as_database_error().unwrap();
            assert_eq!(db.code().as_deref(), Some("55P03"));
            assert!(
                db.message()
                    .contains("pause agent SQL writers and retry upgrade")
            );
        }
        other => panic!("unexpected migration error: {other}"),
    }
    assert!(started.elapsed() < Duration::from_secs(8));
    assert_eq!(
        snapshot(&pool).await,
        before,
        "timed-out repair must be atomic"
    );
    writer.rollback().await.unwrap();
    migrator.run(&pool).await.unwrap();
    let repaired: bool = sqlx::query_scalar(
        "select bool_and(board_position ~ '^([0-9a-f]{2})*80$') from porchlight.item_occurrences",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(repaired);
    let canonical: (String, i32) = sqlx::query_as(
        "select board_position,version from porchlight.item_occurrences where id=$1",
    )
    .bind(valid_occurrence)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        canonical,
        ("80".into(), 1),
        "canonical collection must be untouched"
    );
    migrator.run(&pool).await.unwrap();
    assert_eq!(
        waiting_mode, "ShareRowExclusiveLock",
        "repair must gate writers BEFORE reading the ordering snapshot"
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn ordering_migration_locks_before_snapshot_and_retries(pool: PgPool) {
    ordering_migration_writer(pool, true).await;
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn ordering_migration_list_writer_fails_fast_and_retries(pool: PgPool) {
    ordering_migration_writer(pool, false).await;
}
