use porchlight_db::testing::seed_household;
use sqlx::PgPool;

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

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn seed_creates_three_members(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let n: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.household_members where household_id = $1",
    )
    .bind(h.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 3);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn dependents_have_no_user_and_users_need_one(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let sql = format!(
        "insert into porchlight.household_members (household_id, role, display_name, color, tzid) \
         values ('{}', 'user', 'Nobody', '#83A598', 'America/New_York')",
        h.id
    );
    assert_eq!(code(&pool, &sql).await.as_deref(), Some("23514"));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn invalid_time_zones_are_rejected(pool: PgPool) {
    for tz in ["Mars/Olympus", "EST5EDT+1", "posix/America/New_York", "PST"] {
        let sql = format!("insert into porchlight.households (name, tzid) values ('H', '{tz}')");
        assert_eq!(code(&pool, &sql).await.as_deref(), Some("23514"), "{tz}");
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn members_cannot_reference_another_households_user(pool: PgPool) {
    let a = seed_household(&pool).await.unwrap();
    let b = seed_household(&pool).await.unwrap();
    // A user of household A that has no member yet, so only the household-scoped FK can reject it
    // (the UNIQUE (user_id) constraint would otherwise fire first with 23505).
    let user_of_a: uuid::Uuid = sqlx::query_scalar(
        "insert into porchlight.users (household_id, username) values ($1, 'spare') returning id",
    )
    .bind(a.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let sql = format!(
        "insert into porchlight.household_members (household_id, user_id, role, display_name, color, tzid) \
         values ('{}', '{}', 'user', 'X', '#83A598', 'America/New_York')",
        b.id, user_of_a
    );
    assert_eq!(code(&pool, &sql).await.as_deref(), Some("23503"));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn usernames_are_unique_per_household_and_well_formed(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let dup = format!(
        "insert into porchlight.users (household_id, username) values ('{}', 'admin')",
        h.id
    );
    assert_eq!(code(&pool, &dup).await.as_deref(), Some("23505"));
    let bad = format!(
        "insert into porchlight.users (household_id, username) values ('{}', 'Has Space')",
        h.id
    );
    assert_eq!(code(&pool, &bad).await.as_deref(), Some("23514"));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn certificate_fingerprints_are_32_bytes_and_unique(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let device: uuid::Uuid = sqlx::query_scalar(
        "insert into porchlight.devices (household_id, member_id, label, platform) values ($1, $2, 'Phone', 'android') returning id")
        .bind(h.id).bind(h.user).fetch_one(&pool).await.unwrap();
    let short = format!(
        "insert into porchlight.device_certificates (household_id, device_id, fingerprint_sha256, serial, not_before, not_after) \
         values ('{}', '{device}', '\\x0102', '01', now(), now() + interval '1 year')",
        h.id
    );
    assert_eq!(code(&pool, &short).await.as_deref(), Some("23514"));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn api_token_scopes_are_restricted(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let sql = format!(
        "insert into porchlight.api_tokens (household_id, owner_member_id, name, token_hash, scopes, expires_at) \
         values ('{}', '{}', 't', '\\x{}', '{{root}}', now() + interval '1 year')",
        h.id,
        h.super_admin,
        "00".repeat(32)
    );
    assert_eq!(code(&pool, &sql).await.as_deref(), Some("23514"));
}
