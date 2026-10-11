use porchlight_db::testing::{
    Household, Role, connect_as, insert_appointment, pool_as, seed_household,
};
use serde_json::{Value, json};
use sqlx::postgres::PgListener;
use sqlx::{Executor, PgPool, Postgres};
use std::time::Duration;
use uuid::Uuid;

async fn code(pool: &PgPool, sql: &str) -> Option<String> {
    sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(pool)
        .await
        .err()
        .and_then(|e| {
            e.as_database_error()
                .and_then(|d| d.code().map(|c| c.into_owned()))
        })
}

fn assert_code(err: sqlx::Error, expected: &str) {
    assert_eq!(
        err.as_database_error().unwrap().code().as_deref(),
        Some(expected)
    );
}

async fn device(pool: &PgPool, h: &Household) -> Uuid {
    sqlx::query_scalar(
        "insert into porchlight.devices (household_id, member_id, label, platform) \
         values ($1, $2, 'Phone', 'android') returning id",
    )
    .bind(h.id)
    .bind(h.user)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn session(pool: &PgPool, h: &Household, device: Uuid) -> Uuid {
    sqlx::query_scalar(
        "insert into porchlight.sessions (household_id, device_id, token_hash, expires_at) \
         values ($1, $2, decode(repeat('00', 32), 'hex'), now() + interval '1 day') returning id",
    )
    .bind(h.id)
    .bind(device)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn reset(pool: &PgPool, h: &Household) -> Uuid {
    sqlx::query_scalar(
        "insert into porchlight.password_resets (household_id, user_id, code_hash, expires_at, created_by) \
         select $1, user_id, decode(repeat('00', 32), 'hex'), now() + interval '15 minutes', $3 \
         from porchlight.household_members where id = $2 returning id",
    )
    .bind(h.id)
    .bind(h.user)
    .bind(h.super_admin)
    .fetch_one(pool)
    .await
    .unwrap()
}

const STEP_UP: &str = "insert into porchlight.step_ups \
    (household_id, session_id, device_id, action, token_hash, expires_at) \
    values ($1, $2, $3, $4, decode(repeat($5, 32), 'hex'), now() - interval '1 minute') returning id";

async fn step_up<'e>(
    executor: impl Executor<'e, Database = Postgres>,
    h: &Household,
    session: Uuid,
    device: Uuid,
    action: &str,
    hash_byte: &str,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(STEP_UP)
        .bind(h.id)
        .bind(session)
        .bind(device)
        .bind(action)
        .bind(hash_byte)
        .fetch_one(executor)
        .await
}

const IDEM_INSERT: &str = "insert into porchlight.idempotency_keys \
    (household_id, scope_kind, scope_id, idem_key, request_hash, expires_at) \
    values ($1, 'device', $2, 'test-key', decode(repeat('00', 32), 'hex'), now() + interval '1 day') \
    on conflict do nothing";

async fn idem_insert<'e>(
    executor: impl Executor<'e, Database = Postgres>,
    household: Uuid,
    scope: Uuid,
) -> Result<sqlx::postgres::PgQueryResult, sqlx::Error> {
    sqlx::query(IDEM_INSERT)
        .bind(household)
        .bind(scope)
        .execute(executor)
        .await
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn revocation_is_final(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let d = device(&pool, &h).await;
    let s = session(&pool, &h, d).await;
    let pat: Uuid = sqlx::query_scalar(
        "insert into porchlight.api_tokens (household_id, owner_member_id, name, token_hash, scopes, expires_at) \
         values ($1, $2, 'test', decode(repeat('00', 32), 'hex'), '{read}', now() + interval '1 day') returning id",
    )
    .bind(h.id).bind(h.super_admin).fetch_one(&pool).await.unwrap();
    let enrollment: Uuid = sqlx::query_scalar(
        "insert into porchlight.enrollment_tokens \
         (household_id, member_id, device_label, token_hash, expires_at, created_by) \
         values ($1, $2, 'Phone', decode(repeat('00', 32), 'hex'), now() + interval '15 minutes', $3) returning id",
    )
    .bind(h.id).bind(h.user).bind(h.super_admin).fetch_one(&pool).await.unwrap();
    let r = reset(&pool, &h).await;
    // Exercise existing-table guards before touching the new table, so RED proves missing finality too.
    for (table, column, id) in [
        ("devices", "revoked_at", d),
        ("sessions", "revoked_at", s),
        ("api_tokens", "revoked_at", pat),
        ("enrollment_tokens", "used_at", enrollment),
        ("password_resets", "used_at", r),
    ] {
        assert_final(&pool, table, column, id).await;
    }
    let grant = step_up(&pool, &h, s, d, "device.revoke", "00")
        .await
        .unwrap();
    assert_final(&pool, "step_ups", "used_at", grant).await;
}

async fn assert_final(pool: &PgPool, table: &str, column: &str, id: Uuid) {
    let update = format!("update porchlight.{table} set {column} = ");
    let condition = format!(" where id = '{id}'");
    assert_eq!(code(pool, &format!("{update}now(){condition}")).await, None);
    // A no-op is allowed, but neither replacement nor clearing is allowed.
    assert_eq!(
        code(pool, &format!("{update}{column}{condition}")).await,
        None
    );
    for value in [format!("{column} + interval '1 second'"), "null".to_owned()] {
        assert_eq!(
            code(pool, &format!("{update}{value}{condition}"))
                .await
                .as_deref(),
            Some("PL422"),
            "{table}.{column} = {value}",
        );
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn revoking_a_device_notifies(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let d = device(&pool, &h).await;
    let mut listener = PgListener::connect_with(&pool).await.unwrap();
    listener.listen("porchlight_revocation").await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("update porchlight.devices set revoked_at = now() where id = $1")
        .bind(d)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), listener.recv())
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), listener.recv())
            .await
            .is_err()
    );
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("update porchlight.devices set revoked_at = now() where id = $1")
        .bind(d)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let notification = tokio::time::timeout(Duration::from_secs(2), listener.recv())
        .await
        .expect("notification within 2s")
        .unwrap();
    assert_eq!(notification.channel(), "porchlight_revocation");
    assert_eq!(notification.payload(), d.to_string());
    sqlx::query("update porchlight.devices set label = 'Renamed' where id = $1")
        .bind(d)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), listener.recv())
            .await
            .is_err()
    );
    sqlx::query("update porchlight.devices set revoked_at = revoked_at where id = $1")
        .bind(d)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), listener.recv())
            .await
            .is_err()
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn idempotency_key_unique_per_scope(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let insert = |kind: &str, scope: Uuid, key: &str, hash_len: usize| {
        format!(
            "insert into porchlight.idempotency_keys \
         (household_id, scope_kind, scope_id, idem_key, request_hash, expires_at) \
         values ('{}', '{kind}', '{scope}', '{key}', decode(repeat('00', {hash_len}), 'hex'), now() + interval '1 day')",
            h.id,
        )
    };
    let valid = insert("device", h.user, "test-key", 32);
    assert_eq!(code(&pool, &valid).await, None);
    assert_eq!(code(&pool, &valid).await.as_deref(), Some("23505"));
    assert_eq!(
        code(&pool, &insert("device", h.super_admin, "test-key", 32)).await,
        None
    );
    for kind in ["pat", "enrollment"] {
        assert_eq!(
            code(&pool, &insert(kind, h.user, "test-key", 32)).await,
            None
        );
    }
    for (kind, key, len) in [
        ("other", "other-key".to_owned(), 32),
        ("device", "short-hash".to_owned(), 31),
        ("device", String::new(), 32),
        ("device", "x".repeat(256), 32),
        ("device", "has space".to_owned(), 32),
        ("device", "has\nnewline".to_owned(), 32),
        ("device", "é".to_owned(), 32),
    ] {
        assert_eq!(
            code(&pool, &insert(kind, h.user, &key, len))
                .await
                .as_deref(),
            Some("23514")
        );
    }
    assert_eq!(
        code(&pool, &insert("device", h.user, &"x".repeat(255), 32)).await,
        None
    );
    for status in [99, 600] {
        assert_eq!(
            code(
                &pool,
                &format!("update porchlight.idempotency_keys set response_status = {status}")
            )
            .await
            .as_deref(),
            Some("23514")
        );
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn idempotency_insert_waits_for_uncommitted_duplicate(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut a = connect_as(&pool, Role::App, None).await.unwrap();
    let mut b = connect_as(&pool, Role::App, None).await.unwrap();
    sqlx::query("begin").execute(&mut a).await.unwrap();
    assert_eq!(
        idem_insert(&mut a, h.id, h.user)
            .await
            .unwrap()
            .rows_affected(),
        1
    );
    sqlx::query("set lock_timeout = '200ms'")
        .execute(&mut b)
        .await
        .unwrap();
    assert_code(
        idem_insert(&mut b, h.id, h.user).await.unwrap_err(),
        "55P03",
    );
    sqlx::query("commit").execute(&mut a).await.unwrap();
    assert_eq!(
        idem_insert(&mut b, h.id, h.user)
            .await
            .unwrap()
            .rows_affected(),
        0
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn step_up_binds_session_and_device(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let x = device(&pool, &h).await;
    let y = device(&pool, &h).await;
    let s = session(&pool, &h, x).await;
    assert_code(
        step_up(&pool, &h, s, y, "device.revoke", "00")
            .await
            .unwrap_err(),
        "23503",
    );
    step_up(&pool, &h, s, x, "device.revoke", "00")
        .await
        .unwrap();
    assert_code(
        step_up(&pool, &h, s, x, "Device.Revoke", "01")
            .await
            .unwrap_err(),
        "23514",
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn deleting_a_session_cascades_its_step_ups(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let d = device(&pool, &h).await;
    let s = session(&pool, &h, d).await;
    let used = step_up(&pool, &h, s, d, "device.revoke", "00")
        .await
        .unwrap();
    step_up(&pool, &h, s, d, "device.revoke", "01")
        .await
        .unwrap();
    sqlx::query("update porchlight.step_ups set used_at = now() where id = $1")
        .bind(used)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("delete from porchlight.sessions where id = $1")
        .bind(s)
        .execute(&pool)
        .await
        .unwrap();
    let count: i64 =
        sqlx::query_scalar("select count(*) from porchlight.step_ups where session_id = $1")
            .bind(s)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn install_state_is_a_single_row(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let insert = |id: &str, len: usize| {
        format!(
            "insert into porchlight.install_state (id, household_id, ca_fingerprint) \
         values ({id}, '{}', decode(repeat('00', {len}), 'hex'))",
            h.id,
        )
    };
    assert_eq!(
        code(&pool, &insert("true", 31)).await.as_deref(),
        Some("23514")
    );
    assert_eq!(
        code(&pool, &insert("false", 32)).await.as_deref(),
        Some("23514")
    );
    let valid = format!(
        "insert into porchlight.install_state (household_id, ca_fingerprint) \
        values ('{}', decode(repeat('00', 32), 'hex'))",
        h.id
    );
    assert_eq!(code(&pool, &valid).await, None);
    assert_eq!(code(&pool, &valid).await.as_deref(), Some("23505"));
    let count: i64 = sqlx::query_scalar("select count(*) from porchlight.install_state where id")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

fn recipients(ids: &[Uuid]) -> Value {
    let mut ids = ids.to_vec();
    ids.sort();
    ids.dedup();
    json!(ids)
}

async fn access_events(pool: &PgPool, item: Uuid) -> Vec<Value> {
    sqlx::query_scalar("select payload from porchlight.outbox where kind = 'item.access_changed' and entity_id = $1 order by id")
        .bind(item).fetch_all(pool).await.unwrap()
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn access_changed_names_former_readers(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    // Only live app members are readers; dependent subjects cannot have sessions (S6b).
    let all = recipients(&[h.super_admin, h.user]);
    let household = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set visibility = 'private' where id = $1")
        .bind(household)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        access_events(&pool, household).await,
        vec![json!({"previous_visibility": "household", "recipients": all})]
    );

    let selected = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set visibility = 'selected' where id = $1")
        .bind(selected)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into porchlight.item_viewers (household_id, item_id, member_id) values ($1, $2, $3)")
        .bind(h.id).bind(selected).bind(h.user).execute(&pool).await.unwrap();
    sqlx::query("update porchlight.items set visibility = 'private' where id = $1")
        .bind(selected)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        access_events(&pool, selected).await,
        vec![
            json!({"previous_visibility": "household", "recipients": all}),
            json!({"previous_visibility": "selected", "recipients": recipients(&[h.super_admin, h.user])}),
        ]
    );
    // The lingering viewer is not a private reader; owner transfer names only the previous owner.
    sqlx::query("update porchlight.items set owner_id = $2 where id = $1")
        .bind(selected)
        .bind(h.dependent)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        access_events(&pool, selected).await[2],
        json!({"previous_owner_id": h.super_admin, "recipients": [h.super_admin]})
    );

    let deleted = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set deleted_at = now() where id = $1")
        .bind(deleted)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        access_events(&pool, deleted).await,
        vec![json!({"deleted": true, "recipients": all})]
    );
    let title = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set title = 'New title' where id = $1")
        .bind(title)
        .execute(&pool)
        .await
        .unwrap();
    assert!(access_events(&pool, title).await.is_empty());
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn item_readers_include_assignees_and_only_live_household_members(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("insert into porchlight.item_assignees (household_id, item_id, member_id) values ($1, $2, $3)")
        .bind(h.id).bind(item).bind(h.user).execute(&pool).await.unwrap();
    sqlx::query("update porchlight.household_members set deleted_at = now() where id = $1")
        .bind(h.dependent)
        .execute(&pool)
        .await
        .unwrap();
    for visibility in ["private", "selected", "household"] {
        let actual: Vec<Uuid> =
            sqlx::query_scalar("select porchlight.pl_item_readers($1, $2, $3, $4)")
                .bind(h.id)
                .bind(item)
                .bind(h.super_admin)
                .bind(visibility)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(json!(actual), recipients(&[h.super_admin, h.user]));
    }
    sqlx::query("update porchlight.items set visibility = 'private' where id = $1")
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        access_events(&pool, item).await,
        vec![
            json!({"previous_visibility": "household", "recipients": recipients(&[h.super_admin, h.user])})
        ]
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn reset_failed_attempts_defaults_to_zero_and_is_non_negative(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = reset(&pool, &h).await;
    let attempts: i32 =
        sqlx::query_scalar("select failed_attempts from porchlight.password_resets where id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(attempts, 0);
    sqlx::query("update porchlight.password_resets set failed_attempts = 5 where id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        code(
            &pool,
            &format!(
                "update porchlight.password_resets set failed_attempts = -1 where id = '{id}'"
            )
        )
        .await
        .as_deref(),
        Some("23514")
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn agent_cannot_touch_trust_tables(pool: PgPool) {
    let agent = pool_as(&pool, Role::Agent, Some("agent:test"))
        .await
        .unwrap();
    for table in ["idempotency_keys", "step_ups", "install_state"] {
        assert_eq!(
            code(&agent, &format!("select * from porchlight.{table}"))
                .await
                .as_deref(),
            Some("42501")
        );
    }
    agent.close().await;
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn app_privileges_on_trust_tables(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let d = device(&pool, &h).await;
    let s = session(&pool, &h, d).await;
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    assert_eq!(idem_insert(&app, h.id, d).await.unwrap().rows_affected(), 1);
    let count: i64 = sqlx::query_scalar("select count(*) from porchlight.idempotency_keys")
        .fetch_one(&app)
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(sqlx::query("update porchlight.idempotency_keys set response_status = 200, response_sealed = '\\x0102'")
        .execute(&app).await.unwrap().rows_affected(), 1);
    let response: (Option<i32>, Option<Vec<u8>>) =
        sqlx::query_as("select response_status, response_sealed from porchlight.idempotency_keys")
            .fetch_one(&app)
            .await
            .unwrap();
    assert_eq!(response, (Some(200), Some(vec![1, 2])));
    assert_eq!(
        sqlx::query("delete from porchlight.idempotency_keys")
            .execute(&app)
            .await
            .unwrap()
            .rows_affected(),
        1
    );

    let grant = step_up(&app, &h, s, d, "device.revoke", "00")
        .await
        .unwrap();
    let id: Uuid = sqlx::query_scalar("select id from porchlight.step_ups")
        .fetch_one(&app)
        .await
        .unwrap();
    assert_eq!(id, grant);
    assert_eq!(
        sqlx::query("update porchlight.step_ups set used_at = now() where id = $1")
            .bind(grant)
            .execute(&app)
            .await
            .unwrap()
            .rows_affected(),
        1
    );
    assert_eq!(
        sqlx::query("delete from porchlight.step_ups where id = $1")
            .bind(grant)
            .execute(&app)
            .await
            .unwrap()
            .rows_affected(),
        1
    );

    sqlx::query("insert into porchlight.install_state (household_id, ca_fingerprint) values ($1, decode(repeat('00', 32), 'hex'))")
        .bind(h.id).execute(&app).await.unwrap();
    let household: Uuid = sqlx::query_scalar("select household_id from porchlight.install_state")
        .fetch_one(&app)
        .await
        .unwrap();
    assert_eq!(household, h.id);
    assert_eq!(
        code(
            &app,
            "update porchlight.install_state set initialized_at = now()"
        )
        .await
        .as_deref(),
        Some("42501")
    );
    assert_eq!(
        code(&app, "delete from porchlight.install_state")
            .await
            .as_deref(),
        Some("42501")
    );
    app.close().await;
}
