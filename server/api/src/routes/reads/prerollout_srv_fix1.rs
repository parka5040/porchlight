use super::*;
use sqlx::Acquire;
use std::sync::{Arc, Mutex};
use tracing::{Subscriber, instrument::WithSubscriber};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt},
};

struct Statements(Arc<Mutex<Vec<String>>>);
impl<S: Subscriber> Layer<S> for Statements {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        struct Sql(Option<String>);
        impl tracing::field::Visit for Sql {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name() == "db.statement" {
                    self.0 = Some(value.into());
                }
            }
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
        }
        if event.metadata().target() == "sqlx::query" {
            let mut sql = Sql(None);
            event.record(&mut sql);
            if let Some(sql) = sql.0 {
                self.0.lock().unwrap().push(sql);
            }
        }
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn snapshot_candidates_keep_mixed_anchor_hull(pool: sqlx::PgPool) {
    let h = porchlight_db::testing::seed_household(&pool).await.unwrap();
    let item: Uuid = sqlx::query_scalar("INSERT INTO porchlight.items(household_id,owner_id,title,kind,time_mode,local_due) VALUES($1,$2,'Mixed anchors','deadline','floating','2026-01-01') RETURNING id")
        .bind(h.id).bind(h.user).fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO porchlight.item_occurrences(household_id,item_id,seq,local_due,due_at,override_local_due,override_local_start,override_local_end) VALUES($1,$2,0,'2026-01-01','2026-01-01Z','2030-01-01','2030-01-01','2030-01-02')")
        .bind(h.id).bind(item).execute(&pool).await.unwrap();
    let from = "2028-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
    let mut tx = porchlight_db::tx::begin(&pool).await.unwrap();
    let rows = snapshot_candidates(&mut tx, h.id, h.user, true, from, from + Duration::days(1))
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "snapshot hull must retain mixed generated/override anchors"
    );
}

fn relation_work(plan: &Value, relation: &str) -> f64 {
    match plan {
        Value::Object(map) => {
            let own = if map.get("Relation Name").and_then(Value::as_str) == Some(relation) {
                (map.get("Actual Rows")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    + map
                        .get("Rows Removed by Filter")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0))
                    * map
                        .get("Actual Loops")
                        .and_then(Value::as_f64)
                        .unwrap_or(1.0)
            } else {
                0.0
            };
            own + map
                .values()
                .map(|v| relation_work(v, relation))
                .sum::<f64>()
        }
        Value::Array(values) => values.iter().map(|v| relation_work(v, relation)).sum(),
        _ => 0.0,
    }
}
fn bounded_stream(plan: &Value) -> bool {
    plan.to_string().contains("outbox_created_stream") && relation_work(plan, "outbox") < 100.0
}
fn bounded_board(plan: &Value) -> bool {
    let text = plan.to_string();
    text.contains("item_occurrences_board_open")
        && text.contains("item_occurrences_board_done")
        && relation_work(plan, "item_occurrences") < 50000.0
}

#[sqlx::test(migrations = "../db/migrations")]
async fn captured_stream_window_plan_rejects_aggregate_and_missing_index(pool: sqlx::PgPool) {
    let h = porchlight_db::testing::seed_household(&pool).await.unwrap();
    sqlx::query("INSERT INTO porchlight.outbox(household_id,kind,entity_type,entity_id,created_at,stream_seq) SELECT $1,'derived','item',$2,CASE WHEN n=1 THEN '2026-10-01'::timestamptz ELSE '2026-11-10'::timestamptz END,n+100000 FROM generate_series(1,10000) n")
        .bind(h.id).bind(Uuid::nil()).execute(&pool).await.unwrap();
    sqlx::query("ANALYZE porchlight.outbox")
        .execute(&pool)
        .await
        .unwrap();
    let _ = tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(tracing_subscriber::filter::LevelFilter::TRACE),
    );
    let statements = Arc::new(Mutex::new(Vec::new()));
    let now = "2026-11-13T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
    porchlight_db::stream::window_at(&pool, now)
        .with_subscriber(
            tracing_subscriber::registry()
                .with(Statements(statements.clone()))
                .with(tracing_subscriber::filter::LevelFilter::TRACE),
        )
        .await
        .unwrap();
    let sql = statements
        .lock()
        .unwrap()
        .iter()
        .find(|s| s.contains("AS expired_through"))
        .expect("capture production window_at SQL")
        .clone();
    let explain = format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {sql}");
    let mut tx = porchlight_db::tx::begin(&pool).await.unwrap();
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain.as_str()))
        .bind(now - Duration::days(7))
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(
        bounded_stream(&plan),
        "production stream window scans history: {plan}"
    );
    let old: Value = sqlx::query_scalar("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT coalesce(max(stream_seq),0), coalesce(max(stream_seq) FILTER (WHERE created_at < $1),0), min(stream_seq) FROM porchlight.outbox")
        .bind(now-Duration::days(7)).fetch_one(&mut *tx).await.unwrap();
    assert!(
        !bounded_stream(&old),
        "negative control accepted full-history aggregate"
    );
    sqlx::query("DROP INDEX porchlight.outbox_created_stream")
        .execute(&mut *tx)
        .await
        .unwrap();
    let missing: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain.as_str()))
        .bind(now - Duration::days(7))
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(
        !bounded_stream(&missing),
        "negative control accepted missing stream index"
    );
    println!(
        "stream negative controls rejected: aggregate work={}, missing-index work={}",
        relation_work(&old, "outbox"),
        relation_work(&missing, "outbox")
    );
    tx.rollback().await.unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn production_board_body_plan_rejects_missing_history_indexes(pool: sqlx::PgPool) {
    let h = porchlight_db::testing::seed_household(&pool).await.unwrap();
    let now = "2026-11-13T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
    sqlx::query("INSERT INTO porchlight.items(household_id,owner_id,title,kind) SELECT $1,$2,'History probe','todo' FROM generate_series(1,2000)")
        .bind(h.id).bind(h.user).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO porchlight.item_occurrences(household_id,item_id,seq,status,completed_at) SELECT $1,i.id,seq,CASE WHEN seq<90 THEN 'done' ELSE 'todo' END,CASE WHEN seq<90 THEN $2::timestamptz-(100-seq)*interval '1 day' END FROM porchlight.items i CROSS JOIN generate_series(0,99) seq WHERE i.household_id=$1")
        .bind(h.id).bind(now).execute(&pool).await.unwrap();
    sqlx::raw_sql("ANALYZE porchlight.items; ANALYZE porchlight.item_occurrences")
        .execute(&pool)
        .await
        .unwrap();
    // Read the installed function, not a separately maintained approximation.
    let body: String = sqlx::query_scalar("SELECT prosrc FROM pg_proc WHERE oid='porchlight.pl_board_cards(uuid,timestamptz)'::regprocedure")
        .fetch_one(&pool).await.unwrap();
    let sql = body
        .trim()
        .trim_end_matches(';')
        .replace("p_household", "$1")
        .replace("p_now", "$2");
    let explain = format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {sql}");
    let mut tx = porchlight_db::tx::begin(&pool).await.unwrap();
    sqlx::query("SET LOCAL search_path = pg_catalog, porchlight, pg_temp")
        .execute(&mut *tx)
        .await
        .unwrap();
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain.as_str()))
        .bind(h.id)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(
        bounded_board(&plan),
        "production board function scans closed history: {plan}"
    );
    for index in ["item_occurrences_board_open", "item_occurrences_board_done"] {
        let mut sp = tx.begin().await.unwrap();
        let drop = format!("DROP INDEX porchlight.{index}");
        sqlx::query(sqlx::AssertSqlSafe(drop.as_str()))
            .execute(&mut *sp)
            .await
            .unwrap();
        // Planning alone is enough to reject a missing index; avoid running the unbounded anti-join.
        let explain = format!("EXPLAIN (FORMAT JSON) {sql}");
        let missing: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(explain.as_str()))
            .bind(h.id)
            .bind(now)
            .fetch_one(&mut *sp)
            .await
            .unwrap();
        assert!(
            !bounded_board(&missing),
            "negative control accepted missing {index}"
        );
        println!("board negative control rejected: missing {index}");
        sp.rollback().await.unwrap();
    }
    tx.rollback().await.unwrap();
}
