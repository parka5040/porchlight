use porchlight_db::testing::seed_household;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

async fn snapshot(pool: &PgPool, tables: &[String]) -> Vec<Value> {
    let mut rows = Vec::new();
    for table in tables {
        rows.push(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]') FROM porchlight.{table} t"
        ))).fetch_one(pool).await.unwrap());
    }
    rows
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn handled_fk_child_outside_delete_plan_refuses_purge_atomically(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item: Uuid = sqlx::query_scalar("INSERT INTO porchlight.items(household_id,owner_id,title,kind,deleted_at) VALUES ($1,$2,'Handled dependency','todo',now()) RETURNING id")
        .bind(h.id).bind(h.user).fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO porchlight.item_reminders(household_id,item_id,offset_minutes) VALUES ($1,$2,30)")
        .bind(h.id).bind(item).execute(&pool).await.unwrap();
    let mut tables: Vec<String> =
        sqlx::query_scalar("SELECT table_name FROM porchlight.pl_purge_relations() ORDER BY ord")
            .fetch_all(&pool)
            .await
            .unwrap();
    tables.extend(["purge_log".into(), "outbox".into(), "blobs".into()]);
    let before = snapshot(&pool, &tables).await;
    // Simulate a retention migration that accidentally omits a real handled
    // child from its delete set. Leave the actual FK and allowlist untouched.
    let original: String = sqlx::query_scalar(
        "SELECT pg_get_functiondef('porchlight.pl_purge_relations()'::regprocedure)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let saved = original.replacen(
        "porchlight.pl_purge_relations()",
        "porchlight.pl_test_purge_relations()",
        1,
    );
    assert_ne!(saved, original);
    sqlx::raw_sql(sqlx::AssertSqlSafe(saved))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE OR REPLACE FUNCTION porchlight.pl_purge_relations() RETURNS TABLE(ord integer,table_name text,predicate text) LANGUAGE sql IMMUTABLE SET search_path=pg_catalog,porchlight,pg_temp AS $$ SELECT ord,table_name,CASE WHEN table_name='item_reminders' THEN 'false' ELSE predicate END FROM porchlight.pl_test_purge_relations() $$")
        .execute(&pool).await.unwrap();
    let unhandled: Option<String> =
        sqlx::query_scalar("SELECT porchlight.pl_purge_unhandled_relation($1,$2)")
            .bind(h.id)
            .bind(vec![item])
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unhandled.as_deref(), Some("porchlight.item_reminders"));
    let error = sqlx::query("SELECT porchlight.pl_purge_items($1,$2)")
        .bind(h.id)
        .bind(vec![item])
        .execute(&pool)
        .await
        .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("PL409")
    );
    assert_eq!(
        snapshot(&pool, &tables).await,
        before,
        "refusal changes no rows or audit entries"
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(original))
        .execute(&pool)
        .await
        .unwrap();
    let result: Value = sqlx::query_scalar("SELECT porchlight.pl_purge_items($1,$2)")
        .bind(h.id)
        .bind(vec![item])
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(result["counts"]["item_reminders"], 1);
    assert_eq!(result["counts"]["items"], 1);
}
