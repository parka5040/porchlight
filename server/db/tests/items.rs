use porchlight_db::testing::{Household, insert_appointment, seed_household};
use sqlx::PgPool;
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

/// Runs `insert into porchlight.items (household_id, owner_id, title, <cols>) values (.., 'T', <vals>)`.
async fn insert_item(pool: &PgPool, h: &Household, cols: &str, vals: &str) -> Option<String> {
    let sql = format!(
        "insert into porchlight.items (household_id, owner_id, title, {cols}) values ('{}', '{}', 'T', {vals})",
        h.id, h.super_admin
    );
    code(pool, &sql).await
}

const APPT: &str = "kind, time_mode, tzid, starts_at, ends_at";

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn appointment_requires_ends_at(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let r = insert_item(
        &pool,
        &h,
        "kind, time_mode, tzid, starts_at",
        "'appointment', 'fixed', 'America/New_York', now()",
    )
    .await;
    assert_eq!(r.as_deref(), Some("23514"));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn appointment_start_must_precede_end(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    for (s, e) in [("now()", "now()"), ("now() + interval '1 hour'", "now()")] {
        let r = insert_item(
            &pool,
            &h,
            APPT,
            &format!("'appointment', 'fixed', 'America/New_York', {s}, {e}"),
        )
        .await;
        assert_eq!(r.as_deref(), Some("23514"), "{s} {e}");
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn deadline_with_due_at_requires_tzid(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let r = insert_item(
        &pool,
        &h,
        "kind, time_mode, due_at",
        "'deadline', 'fixed', now()",
    )
    .await;
    assert_eq!(r.as_deref(), Some("23514"));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn floating_appointment_must_not_have_tzid(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let r = insert_item(
        &pool,
        &h,
        "kind, time_mode, tzid, local_start, local_end",
        "'appointment', 'floating', 'America/New_York', '2026-10-12 14:00', '2026-10-12 15:00'",
    )
    .await;
    assert_eq!(r.as_deref(), Some("23514"));
    let ok = insert_item(
        &pool,
        &h,
        "kind, time_mode, local_start, local_end",
        "'appointment', 'floating', '2026-10-12 14:00', '2026-10-12 15:00'",
    )
    .await;
    assert_eq!(ok, None);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn todo_must_not_have_starts_at(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let r = insert_item(&pool, &h, "kind, starts_at", "'todo', now()").await;
    assert_eq!(r.as_deref(), Some("23514"));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn calendar_recurrence_requires_rrule(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let r = insert_item(&pool, &h, "kind, recurrence_mode", "'todo', 'calendar'").await;
    assert_eq!(r.as_deref(), Some("23514"));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn recurrence_shape_accepts_exactly_three_combinations(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut accepted = Vec::new();
    for mode in ["null", "'calendar'", "'completion'"] {
        for rrule in ["null", "'FREQ=DAILY'"] {
            for interval in ["null", "'1 day'"] {
                let combo = format!("{mode}, {rrule}, {interval}");
                let r = insert_item(
                    &pool,
                    &h,
                    "kind, recurrence_mode, rrule, completion_interval",
                    &format!("'todo', {combo}"),
                )
                .await;
                match r.as_deref() {
                    None => accepted.push(combo),
                    Some("23514") => {}
                    other => panic!("{combo}: unexpected {other:?}"),
                }
            }
        }
    }
    assert_eq!(
        accepted,
        [
            "null, null, null",
            "'calendar', 'FREQ=DAILY', null",
            "'completion', null, '1 day'"
        ]
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn rrule_grammar_is_enforced(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let cols = "kind, recurrence_mode, rrule";
    let bad = insert_item(&pool, &h, cols, "'todo', 'calendar', 'FREQ=HOURLY'").await;
    assert_eq!(bad.as_deref(), Some("23514"));
    for ok in [
        "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE",
        "FREQ=MONTHLY;BYDAY=2TU",
    ] {
        let r = insert_item(&pool, &h, cols, &format!("'todo', 'calendar', '{ok}'")).await;
        assert_eq!(r, None, "{ok}");
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn insert_appointment_succeeds(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let id = insert_appointment(&pool, &h).await.unwrap();
    let (title, owner, tz): (String, Uuid, String) =
        sqlx::query_as("select title, owner_id, tzid from porchlight.items where id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (title.as_str(), owner, tz.as_str()),
        ("Dentist", h.super_admin, "America/New_York")
    );
}

async fn insert_occurrence(
    pool: &PgPool,
    h: &Household,
    item: Uuid,
    cols: &str,
    vals: &str,
) -> Result<Uuid, sqlx::Error> {
    let sql = format!(
        "insert into porchlight.item_occurrences (household_id, item_id, seq{cols}) values ($1, $2, 0{vals}) returning id"
    );
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(h.id)
        .bind(item)
        .fetch_one(pool)
        .await
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn during_is_null_without_both_bounds(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let id = insert_occurrence(&pool, &h, item, "", "").await.unwrap();
    let null: bool =
        sqlx::query_scalar("select during is null from porchlight.item_occurrences where id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(null);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn during_matches_start_and_end(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let id = insert_occurrence(
        &pool,
        &h,
        item,
        ", starts_at, ends_at",
        ", '2026-10-12 18:00+00', '2026-10-12 19:00+00'",
    )
    .await
    .unwrap();
    let eq: bool = sqlx::query_scalar(
        "select during = tstzrange(starts_at, ends_at) from porchlight.item_occurrences where id = $1")
        .bind(id).fetch_one(&pool).await.unwrap();
    assert!(eq);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn overrides_toggle_is_detached(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let id = insert_occurrence(&pool, &h, item, "", "").await.unwrap();
    let detached = || async {
        sqlx::query_scalar::<_, bool>(
            "select is_detached from porchlight.item_occurrences where id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    assert!(!detached().await);
    sqlx::query("update porchlight.item_occurrences set override_local_start = '2026-10-13 14:00', override_local_end = '2026-10-13 15:00' where id = $1")
        .bind(id).execute(&pool).await.unwrap();
    assert!(detached().await);
    sqlx::query("update porchlight.item_occurrences set override_local_start = null, override_local_end = null where id = $1")
        .bind(id).execute(&pool).await.unwrap();
    assert!(!detached().await);
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn done_requires_completed_at(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let err = insert_occurrence(&pool, &h, item, ", status", ", 'done'")
        .await
        .unwrap_err();
    assert_eq!(
        err.as_database_error()
            .and_then(|d| d.code().map(|c| c.into_owned()))
            .as_deref(),
        Some("23514")
    );
    insert_occurrence(&pool, &h, item, ", status, completed_at", ", 'done', now()")
        .await
        .unwrap();
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn cannot_assign_member_of_another_household(pool: PgPool) {
    let a = seed_household(&pool).await.unwrap();
    let b = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &a).await.unwrap();
    let sql = format!(
        "insert into porchlight.item_assignees (household_id, item_id, member_id) values ('{}', '{}', '{}')",
        a.id, item, b.user
    );
    assert_eq!(code(&pool, &sql).await.as_deref(), Some("23503"));
    let ok = format!(
        "insert into porchlight.item_assignees (household_id, item_id, member_id) values ('{}', '{}', '{}')",
        a.id, item, a.user
    );
    assert_eq!(code(&pool, &ok).await, None);
}
