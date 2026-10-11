use porchlight_db::outbox::assign_stream_seqs;
use porchlight_db::testing::{
    Household, Role, connect_as, insert_appointment, seed_household as seed_with_settings,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use std::time::Duration;
use uuid::Uuid;

// These low-level sequence/rollback tests intentionally start with an empty outbox.
// S4b tests separately verify default-settings insertion and private stream delivery.
// S5 admin_domain tests separately verify member INSERT/lifecycle tickles.
async fn seed_household(pool: &PgPool) -> anyhow::Result<Household> {
    let h = seed_with_settings(pool).await?;
    sqlx::query("delete from porchlight.outbox where household_id=$1 and kind in ('settings.changed','member.changed')")
        .bind(h.id)
        .execute(pool)
        .await?;
    Ok(h)
}

async fn rows(pool: &PgPool, kind: &'static str, entity: Uuid) -> Vec<(Value, Option<i64>)> {
    sqlx::query_as(
        "select payload, stream_seq from porchlight.outbox where kind = $1 and entity_id = $2 order by id",
    )
    .bind(kind)
    .bind(entity)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn occurrence(pool: &PgPool, h: &Household, item: Uuid) -> Uuid {
    sqlx::query_scalar(
        "insert into porchlight.item_occurrences (household_id, item_id, seq, status) \
         values ($1, $2, 0, 'todo') returning id",
    )
    .bind(h.id)
    .bind(item)
    .fetch_one(pool)
    .await
    .unwrap()
}

const INSERT_ITEM: &str = "insert into porchlight.items (household_id, owner_id, title, kind, time_mode, tzid, starts_at, ends_at) \
     values ($1, $2, 'X', 'appointment', 'fixed', 'America/New_York', now(), now() + interval '1 hour') returning id";

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn item_insert_writes_one_unprojected_row(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let r = rows(&pool, "item.changed", id).await;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].1, None);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn rolled_back_insert_leaves_no_row(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(INSERT_ITEM)
        .bind(h.id)
        .bind(h.super_admin)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let n: i64 = sqlx::query_scalar("select count(*) from porchlight.outbox")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn assign_numbers_pending_rows_once(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    assert_eq!(assign_stream_seqs(&pool, 100).await.unwrap(), Some(1));
    assert_eq!(rows(&pool, "item.changed", id).await[0].1, Some(1));
    assert_eq!(assign_stream_seqs(&pool, 100).await.unwrap(), None);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn late_commit_gets_later_sequence(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut a = pool.begin().await.unwrap();
    let a_id: Uuid = sqlx::query_scalar(INSERT_ITEM)
        .bind(h.id)
        .bind(h.super_admin)
        .fetch_one(&mut *a)
        .await
        .unwrap();
    let mut b = pool.begin().await.unwrap();
    let b_id: Uuid = sqlx::query_scalar(INSERT_ITEM)
        .bind(h.id)
        .bind(h.super_admin)
        .fetch_one(&mut *b)
        .await
        .unwrap();
    b.commit().await.unwrap();
    assert_eq!(assign_stream_seqs(&pool, 100).await.unwrap(), Some(1));
    a.commit().await.unwrap();
    assert_eq!(assign_stream_seqs(&pool, 100).await.unwrap(), Some(2));
    let a_seq = rows(&pool, "item.changed", a_id).await[0].1.unwrap();
    let b_seq = rows(&pool, "item.changed", b_id).await[0].1.unwrap();
    assert_eq!((b_seq, a_seq), (1, 2));
    let a_oid: Uuid = sqlx::query_scalar(
        "select id from porchlight.outbox where entity_id = $1 and kind = 'item.changed'",
    )
    .bind(a_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let b_oid: Uuid = sqlx::query_scalar(
        "select id from porchlight.outbox where entity_id = $1 and kind = 'item.changed'",
    )
    .bind(b_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(a_oid < b_oid, "A's outbox id is the smaller one");
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn listener_notified_on_commit_only(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut l = PgListener::connect_with(&pool).await.unwrap();
    l.listen("porchlight_outbox").await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(INSERT_ITEM)
        .bind(h.id)
        .bind(h.super_admin)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let none = tokio::time::timeout(Duration::from_millis(500), l.recv()).await;
    assert!(none.is_err(), "no notification after rollback");
    insert_appointment(&pool, &h).await.unwrap();
    let n = tokio::time::timeout(Duration::from_secs(2), l.recv())
        .await
        .expect("notification within 2s")
        .unwrap();
    assert_eq!(n.channel(), "porchlight_outbox");
    assert_eq!(n.payload(), "");
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn occurrence_changes_write_events_generated_updates_do_not(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let occ = occurrence(&pool, &h, id).await;
    assert!(rows(&pool, "occurrence.changed", occ).await.is_empty());
    let total = |pool: &PgPool| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("select count(*) from porchlight.outbox")
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    sqlx::query("update porchlight.item_occurrences set status = 'doing' where id = $1")
        .bind(occ)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(rows(&pool, "occurrence.changed", occ).await.len(), 1);
    let before = total(&pool).await;
    sqlx::query(
        "update porchlight.item_occurrences set starts_at = now(), ends_at = now() + interval '1 hour', \
         disposition = 'overdue' where id = $1",
    )
    .bind(occ)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        total(&pool).await,
        before,
        "generated-only update writes no outbox row"
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn access_changes_write_access_events(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set owner_id = $2 where id = $1")
        .bind(id)
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    let r = rows(&pool, "item.access_changed", id).await;
    assert_eq!(r.len(), 1);
    // Dependents cannot hold an app session and are not authoritative readers (S6b).
    let mut former_readers = [h.super_admin, h.user];
    former_readers.sort();
    assert_eq!(
        r[0].0,
        json!({"previous_owner_id": h.super_admin, "recipients": former_readers})
    );

    sqlx::query(
        "insert into porchlight.item_viewers (household_id, item_id, member_id) values ($1, $2, $3)",
    )
    .bind(h.id)
    .bind(id)
    .bind(h.dependent)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(rows(&pool, "item.access_changed", id).await.len(), 1);
    sqlx::query("delete from porchlight.item_viewers where item_id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let r = rows(&pool, "item.access_changed", id).await;
    assert_eq!(r.len(), 1, "household viewers did not lose visibility");

    sqlx::query(
        "insert into porchlight.item_assignees (household_id, item_id, member_id) values ($1, $2, $3)",
    )
    .bind(h.id)
    .bind(id)
    .bind(h.super_admin)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(rows(&pool, "item.access_changed", id).await.len(), 1);
    sqlx::query("delete from porchlight.item_assignees where item_id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let r = rows(&pool, "item.access_changed", id).await;
    assert_eq!(r.len(), 2);
    assert_eq!(r[1].0, json!({"member_id": h.super_admin}));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn member_display_and_tz_changes_write_member_changed(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    sqlx::query("update porchlight.household_members set display_name = 'Alexandra' where id = $1")
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(rows(&pool, "member.changed", h.user).await.len(), 1);
    sqlx::query("update porchlight.household_members set tzid = 'Europe/Paris' where id = $1")
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    let r = rows(&pool, "member.changed", h.user).await;
    assert_eq!(r.len(), 2);
    assert!(r.iter().all(|row| row == &(json!({}), None)));
    let entity_type: String = sqlx::query_scalar(
        "select distinct entity_type from porchlight.outbox where kind = 'member.changed' and entity_id = $1",
    )
    .bind(h.user)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(entity_type, "member");
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn visibility_change_records_previous_visibility(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set visibility = 'private' where id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update porchlight.items set visibility = 'selected', owner_id = $2 where id = $1")
        .bind(id)
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    let r = rows(&pool, "item.access_changed", id).await;
    assert_eq!(r.len(), 2);
    // Dependents cannot hold an app session and are not authoritative readers (S6b).
    let mut former_readers = [h.super_admin, h.user];
    former_readers.sort();
    assert_eq!(
        r[0].0,
        json!({"previous_visibility": "household", "recipients": former_readers})
    );
    assert_eq!(
        r[1].0,
        json!({"previous_visibility": "private", "previous_owner_id": h.super_admin, "recipients": [h.super_admin]})
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn stream_seq_is_assigned_once(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    assert_eq!(assign_stream_seqs(&pool, 100).await.unwrap(), Some(1));
    let mut app = connect_as(&pool, Role::App, None).await.unwrap();
    for sql in [
        "update porchlight.outbox set stream_seq = 99 where entity_id = $1",
        "update porchlight.outbox set stream_seq = null where entity_id = $1",
    ] {
        let err = sqlx::query(sql)
            .bind(id)
            .execute(&mut app)
            .await
            .unwrap_err();
        assert_eq!(
            err.as_database_error().unwrap().code().as_deref(),
            Some("PL422"),
            "{sql}"
        );
    }
    // Rewriting the same value is not a change.
    sqlx::query("update porchlight.outbox set stream_seq = stream_seq where entity_id = $1")
        .bind(id)
        .execute(&mut app)
        .await
        .unwrap();
    assert_eq!(rows(&pool, "item.changed", id).await[0].1, Some(1));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn agent_cannot_stall_assignment(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let mut agent = connect_as(&pool, Role::Agent, Some("agent:test"))
        .await
        .unwrap();
    // PREPARE takes parse-time table locks before any privilege check, so an agent with no grant on the mutex
    // can still hold RowExclusiveLock / RowShareLock on it for its whole transaction.
    sqlx::raw_sql(
        "begin; \
         prepare p as delete from porchlight.stream_assign_mutex; \
         prepare q as select from porchlight.stream_assign_mutex for update",
    )
    .execute(&mut agent)
    .await
    .unwrap();
    let held: Vec<String> = sqlx::query_scalar(
        "select l.mode from pg_locks l join pg_stat_activity a on a.pid = l.pid \
         where l.relation = 'porchlight.stream_assign_mutex'::regclass and a.usename = 'porchlight_agent' \
         order by 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(held, ["RowExclusiveLock", "RowShareLock"]);

    let max = tokio::time::timeout(Duration::from_secs(2), assign_stream_seqs(&pool, 100))
        .await
        .expect("assignment blocked by the agent")
        .unwrap();
    assert_eq!(max, Some(1));
    assert_eq!(rows(&pool, "item.changed", id).await[0].1, Some(1));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn concurrent_assignment_is_serialized(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    for _ in 0..40 {
        insert_appointment(&pool, &h).await.unwrap();
    }
    let total: i64 = sqlx::query_scalar("select count(*) from porchlight.outbox")
        .fetch_one(&pool)
        .await
        .unwrap();
    let batch = total / 2 + 1;
    let (a, b) = tokio::join!(
        assign_stream_seqs(&pool, batch),
        assign_stream_seqs(&pool, batch)
    );
    let (a, b) = (a.unwrap().unwrap(), b.unwrap().unwrap());
    assert_ne!(a, b);
    let (first_max, second_max) = (a.min(b), a.max(b));

    let seqs: Vec<i64> = sqlx::query_scalar(
        "select stream_seq from porchlight.outbox where stream_seq is not null order by stream_seq",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    // Every row assigned exactly once: all rows numbered, no duplicates.
    assert_eq!(seqs.len() as i64, total);
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));
    // Batches do not interleave: the first batch holds the lowest `batch` seqs, the second all greater ones.
    assert_eq!(seqs[batch as usize - 1], first_max);
    assert_eq!(*seqs.last().unwrap(), second_max);
    assert!(seqs[batch as usize] > first_max);
}
