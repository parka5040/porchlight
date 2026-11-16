use porchlight_db::{testing::seed_household, tx};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn derived_lookup_indexes_serve_real_predicates(pool: PgPool) {
    let mut tx = tx::begin(&pool).await.unwrap();
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let mut missing = Vec::new();
    for (sql, index) in [
        (
            "DELETE FROM porchlight.member_agenda WHERE item_id=$1",
            "member_agenda_item",
        ),
        (
            "DELETE FROM porchlight.scheduled_reminders WHERE item_id=$1",
            "scheduled_reminders_item",
        ),
        (
            "SELECT 1 FROM porchlight.member_agenda WHERE household_id=$1 AND occurrence_id=$2",
            "member_agenda_occurrence",
        ),
        (
            "SELECT 1 FROM porchlight.scheduled_reminders WHERE household_id=$1 AND occurrence_id=$2",
            "scheduled_reminders_occurrence",
        ),
        (
            "SELECT 1 FROM porchlight.scheduled_reminders WHERE household_id=$1 AND reminder_id=$2",
            "scheduled_reminders_reminder",
        ),
    ] {
        let query = format!("EXPLAIN (FORMAT JSON) {sql}");
        let mut query =
            sqlx::query_scalar::<_, Value>(sqlx::AssertSqlSafe(query.as_str())).bind(Uuid::nil());
        if sql.contains("$2") {
            query = query.bind(Uuid::nil());
        }
        let plan = query.fetch_one(&mut *tx).await.unwrap();
        if !plan.to_string().contains(index) {
            missing.push(format!("{index}: {plan}"));
        }
    }
    let plan: Value = sqlx::query_scalar("EXPLAIN (FORMAT JSON) SELECT * FROM porchlight.scheduled_reminders WHERE fire_at >= $1 AND fire_at < $2")
        .bind("2026-11-01T00:00:00Z".parse::<chrono::DateTime<chrono::Utc>>().unwrap())
        .bind("2026-11-02T00:00:00Z".parse::<chrono::DateTime<chrono::Utc>>().unwrap())
        .fetch_one(&mut *tx).await.unwrap();
    if !plan.to_string().contains("scheduled_reminders_fire") {
        missing.push(format!("scheduled_reminders_fire: {plan}"));
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn stream_bounds_use_independent_indexes(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    sqlx::query("INSERT INTO porchlight.outbox(household_id,kind,entity_type,entity_id,created_at,stream_seq) SELECT $1,'derived','item',$2,CASE WHEN n=1 THEN '2026-10-01'::timestamptz ELSE '2026-11-10'::timestamptz END,n+100000 FROM generate_series(1,10000) n")
        .bind(h.id).bind(Uuid::nil()).execute(&pool).await.unwrap();
    sqlx::query("ANALYZE porchlight.outbox")
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = tx::begin(&pool).await.unwrap();
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let plan: Value = sqlx::query_scalar(
        "EXPLAIN (FORMAT JSON) SELECT max(stream_seq) FROM porchlight.outbox WHERE created_at < $1",
    )
    .bind(
        "2026-11-01T00:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap(),
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(plan.to_string().contains("outbox_created_stream"), "{plan}");
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn stream_window_matches_full_aggregate_when_publication_is_reordered(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    sqlx::query("INSERT INTO porchlight.outbox(household_id,kind,entity_type,entity_id,created_at,stream_seq) SELECT $1,'derived','item',$2, '2026-11-10'::timestamptz - (n%3)*interval '10 days', n+100000 FROM generate_series(1,30) n")
        .bind(h.id).bind(Uuid::nil()).execute(&pool).await.unwrap();
    let now = "2026-11-13T00:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let expected: (i64, i64, Option<i64>) = sqlx::query_as("SELECT coalesce(max(stream_seq),0), coalesce(max(stream_seq) FILTER (WHERE created_at < $1),0),min(stream_seq) FROM porchlight.outbox")
        .bind(now - chrono::Duration::days(7)).fetch_one(&pool).await.unwrap();
    let actual = porchlight_db::stream::window_at(&pool, now).await.unwrap();
    assert_eq!(
        (actual.maximum, actual.expired_through, actual.first),
        expected
    );
    assert!(!actual.expired(actual.maximum));
}
