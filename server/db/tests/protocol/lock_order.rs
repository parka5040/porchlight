//! Lock order: items -> item_occurrences -> item_derivation (see 0007_outbox_derivation.sql).
use porchlight_db::testing::{Role, connect_as, insert_appointment, seed_household};
use serde_json::{Value, json};
use sqlx::{Connection, PgPool};
use std::time::Duration;
use tokio::time::timeout;
use uuid::Uuid;

fn sqlstate(e: &sqlx::Error) -> Option<String> {
    e.as_database_error()
        .and_then(|d| d.code().map(|c| c.into_owned()))
}

/// (revision count, requested_gen, latest revision snapshot)
async fn state(pool: &PgPool, item: Uuid) -> (i64, i64, Value) {
    sqlx::query_as(
        "select (select count(*) from porchlight.item_revisions where item_id = $1), \
                (select requested_gen from porchlight.item_derivation where item_id = $1), \
                (select snapshot from porchlight.item_revisions where item_id = $1 order by rev desc limit 1)",
    )
    .bind(item)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn child_delete_locks_the_item_before_the_derivation_row(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query(
        "insert into porchlight.item_assignees (household_id, item_id, member_id) values ($1, $2, $3)",
    )
    .bind(h.id)
    .bind(id)
    .bind(h.user)
    .execute(&pool)
    .await
    .unwrap();
    let (revs, gen_before, _) = state(&pool, id).await;

    let mut a = connect_as(&pool, Role::App, None).await.unwrap();
    let mut b = connect_as(&pool, Role::Agent, Some("agent:test"))
        .await
        .unwrap();
    let mut a_tx = a.begin().await.unwrap();
    sqlx::query("update porchlight.items set title = title where id = $1")
        .bind(id)
        .execute(&mut *a_tx)
        .await
        .unwrap();

    // 0061 replaces the former waiting-parent protocol with an immediate
    // rejection: a raw DELETE already holds the child tuple before its trigger.
    let mut b_tx = b.begin().await.unwrap();
    let error = timeout(
        Duration::from_secs(15),
        sqlx::query("delete from porchlight.item_assignees where item_id = $1")
            .bind(id)
            .execute(&mut *b_tx),
    )
    .await
    .expect("raw guard did not respond")
    .unwrap_err();
    assert_eq!(sqlstate(&error).as_deref(), Some("55P03"));
    b_tx.rollback().await.unwrap();

    // A can still acquire derivation; B's rejected deletion left no effects.
    timeout(
        Duration::from_secs(2),
        sqlx::query("update porchlight.items set est_minutes = 1 where id = $1")
            .bind(id)
            .execute(&mut *a_tx),
    )
    .await
    .expect("A blocked behind B: lock-order inversion")
    .unwrap_or_else(|e| panic!("A failed ({:?}): {e}", sqlstate(&e)));
    a_tx.commit().await.unwrap();
    // Retry the whole raw transaction once the parent is available.
    let mut b_tx = b.begin().await.unwrap();
    sqlx::query("delete from porchlight.item_assignees where item_id = $1")
        .bind(id)
        .execute(&mut *b_tx)
        .await
        .unwrap();
    b_tx.commit().await.unwrap();

    // A wrote two revisions and B's parent bump one; A's est_minutes change and B's delete each enqueue.
    let (revs_after, gen_after, snapshot) = state(&pool, id).await;
    assert_eq!(
        (revs_after, gen_after),
        (revs + 3, gen_before + 2),
        "{snapshot}"
    );
    assert_eq!(snapshot["assignees"], json!([]));
    assert_eq!(snapshot["est_minutes"], json!(1));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn concurrent_child_inserts_on_one_item_do_not_deadlock(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let insert_occurrence =
        "insert into porchlight.item_occurrences (household_id, item_id, seq) values ($1, $2, $3)";
    let insert_reminder = "insert into porchlight.item_reminders (household_id, item_id, offset_minutes) values ($1, $2, $3)";

    // Both transactions hold FOR KEY SHARE on the item (foreign-key checks) before touching a child table, so the
    // parent bump must take a lock that KEY SHARE does not block.
    let mut a = connect_as(&pool, Role::App, None).await.unwrap();
    let mut b = connect_as(&pool, Role::App, None).await.unwrap();
    for (conn, seq) in [(&mut a, 0), (&mut b, 1)] {
        sqlx::query("begin").execute(&mut *conn).await.unwrap();
        sqlx::query(insert_occurrence)
            .bind(h.id)
            .bind(id)
            .bind(seq)
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let a_task = tokio::spawn(async move {
        let result = async {
            sqlx::query(insert_reminder)
                .bind(h.id)
                .bind(id)
                .bind(15)
                .execute(&mut a)
                .await?;
            sqlx::query("commit").execute(&mut a).await.map(drop)
        }
        .await;
        if result.is_err() {
            sqlx::query("rollback").execute(&mut a).await.unwrap();
        }
        (a, result)
    });
    let b_result = timeout(Duration::from_secs(5), async {
        sqlx::query(insert_reminder)
            .bind(h.id)
            .bind(id)
            .bind(30)
            .execute(&mut b)
            .await?;
        sqlx::query("commit").execute(&mut b).await.map(drop)
    })
    .await
    .expect("B did not finish");
    if b_result.is_err() {
        sqlx::query("rollback").execute(&mut b).await.unwrap();
    }
    let (a, a_result) = timeout(Duration::from_secs(15), a_task)
        .await
        .expect("A did not finish")
        .unwrap();
    // NKU remains compatible with both occurrence FK KEY SHARE locks. Only
    // overlapping relationship writes may now fail with 55P03, never 40P01;
    // raw clients must retry their whole transaction (including its occurrence).
    for (who, mut conn, result, seq, offset) in
        [("A", a, a_result, 0, 15), ("B", b, b_result, 1, 30)]
    {
        if let Err(e) = result {
            assert_eq!(
                sqlstate(&e).as_deref(),
                Some("55P03"),
                "{who}: unexpected SQLSTATE"
            );
            let mut tx = conn.begin().await.unwrap();
            sqlx::query(insert_occurrence)
                .bind(h.id)
                .bind(id)
                .bind(seq)
                .execute(&mut *tx)
                .await
                .unwrap();
            sqlx::query(insert_reminder)
                .bind(h.id)
                .bind(id)
                .bind(offset)
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    }
    let reminders: Value = sqlx::query_scalar(
        "select snapshot -> 'reminders' from porchlight.item_revisions where item_id = $1 order by rev desc limit 1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(reminders, json!([15, 30]));
}
