use porchlight_db::testing::{insert_appointment, seed_household};
use sqlx::PgPool;
use uuid::Uuid;

async fn code(pool: &PgPool, sql: &'static str, id: Uuid) -> Option<String> {
    sqlx::query(sql)
        .bind(id)
        .execute(pool)
        .await
        .err()
        .and_then(|e| {
            e.as_database_error()
                .and_then(|d| d.code().map(|c| c.into_owned()))
        })
}

async fn version(pool: &PgPool, id: Uuid) -> i32 {
    sqlx::query_scalar("select version from porchlight.items where id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn blind_update_bumps_version_and_updated_at(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let before: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("select updated_at from porchlight.items where id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("update porchlight.items set title = 'x' where id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let (v, after): (i32, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("select version, updated_at from porchlight.items where id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(v, 2);
    assert!(after > before);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn explicit_version_change_is_pl409(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let r = code(
        &pool,
        "update porchlight.items set version = 7 where id = $1",
        id,
    )
    .await;
    assert_eq!(r.as_deref(), Some("PL409"));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn unchanged_version_value_is_blind_write(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let r = code(
        &pool,
        "update porchlight.items set version = version where id = $1",
        id,
    )
    .await;
    assert_eq!(r, None);
    assert_eq!(version(&pool, id).await, 2);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn changing_household_id_is_pl422(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let other: Uuid = sqlx::query_scalar(
        "insert into porchlight.households (name, tzid) values ('Other', 'America/New_York') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let sql = "update porchlight.items set household_id = $2 where id = $1";
    let e = sqlx::query(sql)
        .bind(id)
        .bind(other)
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(
        e.as_database_error().unwrap().code().as_deref(),
        Some("PL422")
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn stale_conditional_update_affects_no_rows(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set title = 'x' where id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let r = sqlx::query("update porchlight.items set title = 'y' where id = $1 and version = 1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(r.rows_affected(), 0);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn insert_forces_version_one(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id: Uuid = sqlx::query_scalar(
        "insert into porchlight.items (household_id, owner_id, title, kind, time_mode, tzid, starts_at, ends_at, version) \
         values ($1, $2, 'T', 'appointment', 'fixed', 'America/New_York', now(), now() + interval '1 hour', 99) returning id",
    )
    .bind(h.id)
    .bind(h.super_admin)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(version(&pool, id).await, 1);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn non_id_key_column_is_immutable(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    sqlx::query("update porchlight.member_settings set settings='{}' where member_id=$1 and household_id=$2")
        .bind(h.user)
        .bind(h.id)
        .execute(&pool)
        .await
        .unwrap();
    let e =
        sqlx::query("update porchlight.member_settings set member_id = $2 where member_id = $1")
            .bind(h.user)
            .bind(h.dependent)
            .execute(&pool)
            .await
            .unwrap_err();
    assert_eq!(
        e.as_database_error().unwrap().code().as_deref(),
        Some("PL422")
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn household_update_bumps_version(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    sqlx::query("update porchlight.households set name = 'n' where id = $1")
        .bind(h.id)
        .execute(&pool)
        .await
        .unwrap();
    let v: i32 = sqlx::query_scalar("select version from porchlight.households where id = $1")
        .bind(h.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(v, 2);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn guard_triggers_cover_every_versioned_table_with_real_columns(pool: PgPool) {
    // Every table with a version column has the guard (BEFORE INSERT OR UPDATE, per row) and no other table does.
    let tables: Vec<(String, bool, bool)> = sqlx::query_as(
        "select c.relname::text, \
                exists (select 1 from pg_attribute a where a.attrelid = c.oid and a.attname = 'version' \
                        and a.attnum > 0 and not a.attisdropped), \
                exists (select 1 from pg_trigger t where t.tgrelid = c.oid and t.tgname = 'aa_version_guard' \
                        and not t.tgisinternal and t.tgfoid = 'porchlight.pl_version_guard'::regproc \
                        and t.tgtype = 1 | 2 | 4 | 16) \
         from pg_class c where c.relnamespace = 'porchlight'::regnamespace and c.relkind in ('r', 'p') \
         order by 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let mismatched: Vec<_> = tables.iter().filter(|(_, v, g)| v != g).collect();
    assert!(
        mismatched.is_empty(),
        "(table, has version, has guard): {mismatched:?}"
    );
    let guarded = tables.iter().filter(|(_, v, _)| *v).count();
    assert!(guarded >= 12, "only {guarded} versioned tables found");

    // Every trigger argument must name an existing column of its table.
    let bad: Vec<(String, String)> = sqlx::query_as(
        "select c.relname::text, a.arg \
         from pg_trigger t \
         join pg_class c on c.oid = t.tgrelid \
         join pg_namespace n on n.oid = c.relnamespace, \
         lateral unnest(string_to_array(encode(t.tgargs, 'escape'), '\\000')) as a(arg) \
         where n.nspname = 'porchlight' and t.tgname = 'aa_version_guard' and a.arg <> '' \
           and not exists (select 1 from pg_attribute at \
                           where at.attrelid = c.oid and at.attname = a.arg \
                             and at.attnum > 0 and not at.attisdropped)",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(bad.is_empty(), "unknown trigger argument columns: {bad:?}");

    let no_args: Vec<String> = sqlx::query_scalar(
        "select c.relname::text from pg_trigger t join pg_class c on c.oid = t.tgrelid \
         where t.tgname = 'aa_version_guard' and t.tgnargs = 0",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        no_args.is_empty(),
        "triggers without key columns: {no_args:?}"
    );
}
