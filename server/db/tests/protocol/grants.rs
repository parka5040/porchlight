use porchlight_db::testing::{Role, connect_as, insert_appointment, pool_as, seed_household};
use sqlx::{Connection, PgConnection, PgPool};

async fn code(conn: &mut PgConnection, sql: &'static str) -> Option<String> {
    sqlx::query(sql).execute(conn).await.err().and_then(|e| {
        e.as_database_error()
            .and_then(|d| d.code().map(|c| c.into_owned()))
    })
}

const INSERT_TODO: &str = "insert into porchlight.items (household_id, owner_id, title, kind, time_mode, tzid, due_date) \
     values ($1, $2, 'Agent todo', 'todo', 'floating', 'America/New_York', '2026-10-20') returning id";

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn agent_allowed_operations(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut agent = connect_as(&pool, Role::Agent, Some("agent:test"))
        .await
        .unwrap();

    let id: uuid::Uuid = sqlx::query_scalar(INSERT_TODO)
        .bind(h.id)
        .bind(h.super_admin)
        .fetch_one(&mut agent)
        .await
        .unwrap();

    let revs: Vec<(serde_json::Value,)> =
        sqlx::query_as("select actor from porchlight.item_revisions where item_id = $1")
            .bind(id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(revs.len(), 1);
    assert_eq!(revs[0].0["application_name"], "agent:test");
    assert_eq!(revs[0].0["db_user"], "porchlight_agent");
    let outbox: i64 =
        sqlx::query_scalar("select count(*) from porchlight.outbox where entity_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(outbox, 1);
    let deriv: i64 =
        sqlx::query_scalar("select count(*) from porchlight.item_derivation where item_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(deriv, 1);

    let n = sqlx::query("update porchlight.items set title = 'Renamed' where id = $1")
        .bind(id)
        .execute(&mut agent)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(n, 1);

    sqlx::query(
        "insert into porchlight.item_assignees (household_id, item_id, member_id) values ($1, $2, $3)",
    )
    .bind(h.id)
    .bind(id)
    .bind(h.user)
    .execute(&mut agent)
    .await
    .unwrap();
    let n = sqlx::query("delete from porchlight.item_assignees where item_id = $1")
        .bind(id)
        .execute(&mut agent)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(n, 1);

    let item = insert_appointment(&pool, &h).await.unwrap();
    let occ: uuid::Uuid = sqlx::query_scalar(
        "insert into porchlight.item_occurrences (household_id, item_id, seq, status) \
         values ($1, $2, 0, 'todo') returning id",
    )
    .bind(h.id)
    .bind(item)
    .fetch_one(&pool)
    .await
    .unwrap();
    let n = sqlx::query(
        "update porchlight.item_occurrences set status = 'done', completed_at = now() where id = $1",
    )
    .bind(occ)
    .execute(&mut agent)
    .await
    .unwrap()
    .rows_affected();
    assert_eq!(n, 1);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn forbidden_operations_are_42501(pool: PgPool) {
    let mut agent = connect_as(&pool, Role::Agent, Some("agent:test"))
        .await
        .unwrap();
    let mut app = connect_as(&pool, Role::App, None).await.unwrap();

    let cases: Vec<(&str, bool, &'static str)> = vec![
        ("agent", true, "delete from porchlight.items"),
        ("agent", true, "select * from porchlight.user_credentials"),
        ("agent", true, "select * from porchlight.sessions"),
        ("agent", true, "select * from porchlight.devices"),
        ("agent", true, "select * from porchlight.api_tokens"),
        (
            "agent",
            true,
            "insert into porchlight.item_revisions (household_id, item_id, rev, snapshot, actor) \
             values (gen_random_uuid(), gen_random_uuid(), 1, '{}', '{}')",
        ),
        (
            "agent",
            true,
            "update porchlight.item_occurrences set seq = 5",
        ),
        (
            "agent",
            true,
            "update porchlight.item_occurrences set starts_at = now()",
        ),
        (
            "agent",
            true,
            "insert into porchlight.item_occurrences (household_id, item_id, seq) \
             values (gen_random_uuid(), gen_random_uuid(), 0)",
        ),
        ("agent", true, "select * from porchlight.outbox"),
        ("agent", true, "create table x (y int)"),
        ("agent", true, "create temp table t (x int)"),
        ("agent", true, "truncate porchlight.items"),
        ("agent", true, "select pg_advisory_lock(1)"),
        ("agent", true, "select pg_try_advisory_xact_lock(1, 2)"),
        (
            "agent",
            true,
            "select from porchlight.stream_assign_mutex for update",
        ),
        ("app", false, "select pg_advisory_lock(1)"),
        (
            "app",
            false,
            "insert into porchlight.item_revisions (household_id, item_id, rev, snapshot, actor) \
             values (gen_random_uuid(), gen_random_uuid(), 1, '{}', '{}')",
        ),
        ("app", false, "update porchlight.outbox set kind = 'x'"),
        ("app", false, "create temp table t (x int)"),
        // Domain rows are soft-deleted or revoked, never deleted by the app.
        ("app", false, "delete from porchlight.households"),
        ("app", false, "delete from porchlight.items"),
        ("app", false, "delete from porchlight.household_members"),
        ("app", false, "delete from porchlight.devices"),
        ("app", false, "delete from porchlight.device_certificates"),
        (
            "app",
            false,
            "update porchlight.device_certificates set serial = 'x'",
        ),
    ];
    for (who, is_agent, sql) in cases {
        let conn = if is_agent { &mut agent } else { &mut app };
        assert_eq!(
            code(conn, sql).await.as_deref(),
            Some("42501"),
            "{who}: {sql}"
        );
    }
    // LOCK TABLE outside a transaction block fails with 25P01 before privileges are checked.
    let mut tx = agent.begin().await.unwrap();
    assert_eq!(
        code(&mut tx, "lock table porchlight.stream_assign_mutex")
            .await
            .as_deref(),
        Some("42501")
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn app_allowed_operations(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let mut app = connect_as(&pool, Role::App, None).await.unwrap();

    let v: Option<i64> = sqlx::query_scalar("select max(version) from public._sqlx_migrations")
        .fetch_one(&mut app)
        .await
        .unwrap();
    assert!(v.is_some());

    let n = sqlx::query(
        "update porchlight.outbox set stream_seq = nextval('porchlight.outbox_stream_seq') \
         where entity_id = $1",
    )
    .bind(id)
    .execute(&mut app)
    .await
    .unwrap()
    .rows_affected();
    assert_eq!(n, 1);

    // The projector includes three seeded tickles each for S4b settings,
    // S5 members and S6a notification preferences.
    let second = insert_appointment(&pool, &h).await.unwrap();
    let app_pool = pool_as(&pool, Role::App, None).await.unwrap();
    assert_eq!(
        porchlight_db::outbox::assign_stream_seqs(&app_pool, 10)
            .await
            .unwrap(),
        Some(11)
    );
    let seq: Option<i64> =
        sqlx::query_scalar("select stream_seq from porchlight.outbox where entity_id = $1")
            .bind(second)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(seq, Some(11));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn certificate_history_is_immutable(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let device: uuid::Uuid = sqlx::query_scalar(
        "insert into porchlight.devices (household_id, member_id, label, platform) \
         values ($1, $2, 'Phone', 'android') returning id",
    )
    .bind(h.id)
    .bind(h.user)
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut app = connect_as(&pool, Role::App, None).await.unwrap();
    let insert = "insert into porchlight.device_certificates \
                  (household_id, device_id, fingerprint_sha256, serial, not_before, not_after) \
                  values ($1, $2, decode(repeat('ab', 32), 'hex'), '01', now(), now() + interval '1 year')";
    sqlx::query(insert)
        .bind(h.id)
        .bind(device)
        .execute(&mut app)
        .await
        .unwrap();
    let dup = sqlx::query(insert)
        .bind(h.id)
        .bind(device)
        .execute(&mut app)
        .await
        .unwrap_err();
    assert_eq!(
        dup.as_database_error().unwrap().code().as_deref(),
        Some("23505")
    );

    // Even the owner cannot change history; only the maintenance role may (M3 purge).
    let mut su = pool.acquire().await.unwrap();
    for sql in [
        "update porchlight.device_certificates set serial = '02'",
        "delete from porchlight.device_certificates",
    ] {
        assert_eq!(code(&mut su, sql).await.as_deref(), Some("PL422"), "{sql}");
    }
    let mut tx = pool.begin().await.unwrap();
    for sql in [
        // Rolled back below; the maintenance role gets its real grants in M3.
        "grant usage on schema porchlight to porchlight_maintenance",
        "grant delete on porchlight.device_certificates to porchlight_maintenance",
        "set local role porchlight_maintenance",
    ] {
        sqlx::query(sql).execute(&mut *tx).await.unwrap();
    }
    let n = sqlx::query("delete from porchlight.device_certificates")
        .execute(&mut *tx)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(n, 1);
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn only_the_migrator_takes_advisory_locks(pool: PgPool) {
    // sqlx's migration lock is an advisory lock; bootstrap-roles.sql grants the functions to the migrator only.
    let mut migrator = connect_as(&pool, Role::Migrator, None).await.unwrap();
    sqlx::query("select pg_advisory_lock(1), pg_advisory_unlock(1)")
        .execute(&mut migrator)
        .await
        .unwrap();
    let public: Vec<String> = sqlx::query_scalar(
        "select p.oid::regprocedure::text from pg_proc p \
         where p.pronamespace = 'pg_catalog'::regnamespace and p.proname ~ '^pg_(try_)?advisory_' \
           and has_function_privilege('public', p.oid, 'EXECUTE')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(public.is_empty(), "PUBLIC can execute: {public:?}");
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn agent_sessions_have_timeouts(pool: PgPool) {
    let mut agent = connect_as(&pool, Role::Agent, Some("agent:test"))
        .await
        .unwrap();
    let settings: (String, String, String) = sqlx::query_as(
        "select current_setting('idle_in_transaction_session_timeout'), \
                current_setting('lock_timeout'), current_setting('statement_timeout')",
    )
    .fetch_one(&mut agent)
    .await
    .unwrap();
    assert_eq!(
        settings,
        ("1min".to_owned(), "10s".to_owned(), "1min".to_owned())
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn no_function_is_executable_by_public(pool: PgPool) {
    let bad: Vec<String> = sqlx::query_scalar(
        "select p.oid::regprocedure::text from pg_proc p \
         where p.pronamespace = 'porchlight'::regnamespace \
           and not exists (select 1 from pg_depend d where d.classid = 'pg_proc'::regclass \
                           and d.objid = p.oid and d.deptype = 'e') \
           and has_function_privilege('public', p.oid, 'EXECUTE')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(bad.is_empty(), "PUBLIC can execute: {bad:?}");
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn new_functions_default_to_no_public_execute(pool: PgPool) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("set local role porchlight_owner")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("create function porchlight.pl_throwaway() returns int language sql as 'select 1'")
        .execute(&mut *tx)
        .await
        .unwrap();
    let ok: bool = sqlx::query_scalar(
        "select has_function_privilege('porchlight_agent', 'porchlight.pl_throwaway()', 'EXECUTE')",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(!ok);
    tx.rollback().await.unwrap();
}
