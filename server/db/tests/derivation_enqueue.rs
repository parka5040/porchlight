use porchlight_db::testing::{Household, insert_appointment, seed_household};
use sqlx::PgPool;
use uuid::Uuid;

async fn gens(pool: &PgPool, item: Uuid) -> (i64, i64) {
    sqlx::query_as(
        "select requested_gen, applied_gen from porchlight.item_derivation where item_id = $1",
    )
    .bind(item)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn requested(pool: &PgPool, item: Uuid) -> i64 {
    gens(pool, item).await.0
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

async fn exec(pool: &PgPool, sql: &'static str, id: Uuid) {
    sqlx::query(sql).bind(id).execute(pool).await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn new_item_is_enqueued(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    assert_eq!(gens(&pool, id).await, (1, 0));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn title_update_does_not_enqueue_but_time_update_does(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    exec(
        &pool,
        "update porchlight.items set title = 'New' where id = $1",
        id,
    )
    .await;
    assert_eq!(requested(&pool, id).await, 1);
    exec(
        &pool,
        "update porchlight.items set starts_at = starts_at + interval '1 hour', \
         ends_at = ends_at + interval '1 hour' where id = $1",
        id,
    )
    .await;
    assert_eq!(requested(&pool, id).await, 2);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn two_assignees_one_statement_enqueue_once_viewer_never(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query(
        "insert into porchlight.item_assignees (household_id, item_id, member_id) \
         select $1, $2, m from unnest($3::uuid[]) m",
    )
    .bind(h.id)
    .bind(id)
    .bind([h.user, h.dependent].as_slice())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(requested(&pool, id).await, 2);
    sqlx::query(
        "insert into porchlight.item_viewers (household_id, item_id, member_id) values ($1, $2, $3)",
    )
    .bind(h.id)
    .bind(id)
    .bind(h.super_admin)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(requested(&pool, id).await, 2);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn member_tz_change_enqueues_related_items_only(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let assigned = insert_appointment(&pool, &h).await.unwrap(); // owned by super_admin
    let owned: Uuid = sqlx::query_scalar(
        "insert into porchlight.items (household_id, owner_id, title, kind, time_mode, tzid, starts_at, ends_at) \
         values ($1, $2, 'Mine', 'appointment', 'fixed', 'America/New_York', now(), now() + interval '1 hour') returning id",
    )
    .bind(h.id)
    .bind(h.user)
    .fetch_one(&pool)
    .await
    .unwrap();
    // `assigned` is owned by super_admin and assigned to user; `owned` is owned by user;
    // `unrelated` is owned by super_admin only.
    let unrelated: Uuid = sqlx::query_scalar(
        "insert into porchlight.items (household_id, owner_id, title, kind, time_mode, tzid, starts_at, ends_at) \
         values ($1, $2, 'Other', 'appointment', 'fixed', 'America/New_York', now(), now() + interval '1 hour') returning id",
    )
    .bind(h.id)
    .bind(h.super_admin)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into porchlight.item_assignees (household_id, item_id, member_id) values ($1, $2, $3)",
    )
    .bind(h.id)
    .bind(assigned)
    .bind(h.user)
    .execute(&pool)
    .await
    .unwrap();
    let before = (
        requested(&pool, assigned).await,
        requested(&pool, owned).await,
        requested(&pool, unrelated).await,
    );
    sqlx::query("update porchlight.household_members set tzid = 'Europe/Paris' where id = $1")
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(requested(&pool, assigned).await, before.0 + 1);
    assert_eq!(requested(&pool, owned).await, before.1 + 1);
    assert_eq!(requested(&pool, unrelated).await, before.2);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn occurrence_authoritative_changes_enqueue(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let occ = occurrence(&pool, &h, id).await;
    assert_eq!(requested(&pool, id).await, 1);
    exec(
        &pool,
        "update porchlight.item_occurrences set status = 'done', completed_at = now() where id = $1",
        occ,
    )
    .await;
    assert_eq!(requested(&pool, id).await, 2);
    exec(
        &pool,
        "update porchlight.item_occurrences set completed_at = now() - interval '1 day' where id = $1",
        occ,
    )
    .await;
    assert_eq!(requested(&pool, id).await, 3);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn overrides_enqueue_generated_columns_do_not(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let occ = occurrence(&pool, &h, id).await;
    exec(
        &pool,
        "update porchlight.item_occurrences set starts_at = now(), ends_at = now() + interval '1 hour', \
         disposition = 'overdue' where id = $1",
        occ,
    )
    .await;
    assert_eq!(requested(&pool, id).await, 1);
    exec(
        &pool,
        "update porchlight.item_occurrences set override_local_start = '2026-10-12 10:00', \
         override_local_end = '2026-10-12 11:00' where id = $1",
        occ,
    )
    .await;
    assert_eq!(requested(&pool, id).await, 2);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn conditional_ack_loses_to_a_newer_request(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    exec(
        &pool,
        "update porchlight.items set starts_at = starts_at + interval '1 hour', \
         ends_at = ends_at + interval '1 hour' where id = $1",
        id,
    )
    .await;
    let res = sqlx::query(
        "update porchlight.item_derivation set applied_gen = 1 where item_id = $1 and requested_gen = 1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(res.rows_affected(), 0);
    assert_eq!(gens(&pool, id).await, (2, 0));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn owner_and_subject_changes_enqueue_visibility_does_not(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set visibility = 'private' where id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(requested(&pool, id).await, 1);
    sqlx::query("update porchlight.items set owner_id = $2 where id = $1")
        .bind(id)
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(requested(&pool, id).await, 2);
    sqlx::query("update porchlight.items set subject_member_id = $2 where id = $1")
        .bind(id)
        .bind(h.dependent)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(requested(&pool, id).await, 3);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn reminder_insert_and_delete_each_enqueue(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    assert_eq!(requested(&pool, id).await, 1);
    sqlx::query(
        "insert into porchlight.item_reminders (household_id, item_id, offset_minutes) values ($1, $2, 15)",
    )
    .bind(h.id)
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(requested(&pool, id).await, 2);
    exec(
        &pool,
        "delete from porchlight.item_reminders where item_id = $1",
        id,
    )
    .await;
    assert_eq!(requested(&pool, id).await, 3);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn soft_delete_enqueues(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    exec(
        &pool,
        "update porchlight.items set deleted_at = now() where id = $1",
        id,
    )
    .await;
    assert_eq!(requested(&pool, id).await, 2);
}
