use super::*;
use crate::testing::{TestOptions, TestServer};
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
async fn agenda_prefilter_preserves_mixed_projection_membership(pool: sqlx::PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let h = s.init.household_id;
    let member = s.init.admin_member_id;
    let now = "2026-11-13T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
    let item:Uuid=sqlx::query_scalar("INSERT INTO porchlight.items(household_id,owner_id,title,kind) VALUES($1,$2,'Mixed projection','todo') RETURNING id")
        .bind(h).bind(member).fetch_one(&pool).await.unwrap();
    let occurrence:Uuid=sqlx::query_scalar("INSERT INTO porchlight.item_occurrences(household_id,item_id,seq) VALUES($1,$2,0) RETURNING id")
        .bind(h).bind(item).fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO porchlight.member_agenda(household_id,member_id,item_id,occurrence_id,starts_at,ends_at,due_date) VALUES($1,$2,$3,$4,$5-interval '10 days',$5-interval '9 days',($5 AT TIME ZONE 'UTC')::date+40)")
        .bind(h).bind(member).bind(item).bind(occurrence).bind(now).execute(&pool).await.unwrap();
    let caller = Caller::Agent {
        household_id: h,
        member_id: member,
        token_id: Uuid::nil(),
        label: "Agenda probe".into(),
        scopes: vec!["admin".into()],
        request_id: Uuid::nil(),
    };
    let from = now - Duration::days(1);
    let to = now + Duration::days(1);
    // The legacy date branch uses a coalesced anchor, not necessarily due_date.
    // A SQL superset must retain it even for mixed raw-SQL projections.
    let expected:i64=sqlx::query_scalar("SELECT count(*) FROM porchlight.member_agenda a WHERE a.item_id=$1 AND ((a.starts_at<$3 AND a.ends_at>$2) OR (a.due_at>=$2 AND a.due_at<$3) OR (a.due_date IS NOT NULL AND coalesce(a.starts_at,a.due_at,a.due_date::timestamp AT TIME ZONE 'UTC')<$3 AND (a.due_date+1)::timestamp AT TIME ZONE 'UTC'>$2))")
        .bind(item).bind(from).bind(to).fetch_one(&pool).await.unwrap();
    let Json(page) = agenda(
        State(s.state.clone()),
        caller,
        Query(AgendaParams {
            from,
            to,
            member_id: None,
            cursor: None,
            limit: Some(50),
        }),
    )
    .await
    .unwrap();
    assert_eq!(expected, 1);
    assert_eq!(
        page.items.len(),
        expected as usize,
        "agenda SQL superset changed membership"
    );
    s.stop().await;
}

/// Run explicitly on the local test database; captures the production statement
/// strings rather than maintaining approximate copies of these three queries.
#[sqlx::test(migrations = "../db/migrations")]
#[ignore = "200k-row history plan probe"]
async fn household_history_plans(pool: sqlx::PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let h = s.init.household_id;
    let member = s.init.admin_member_id;
    let now = "2026-11-13T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
    sqlx::query("INSERT INTO porchlight.items(household_id,owner_id,title,kind) SELECT $1,$2,'History probe','todo' FROM generate_series(1,2000)")
        .bind(h).bind(member).execute(&pool).await.unwrap();
    sqlx::query("WITH numbered AS (SELECT id,row_number() OVER(ORDER BY id) AS n FROM porchlight.items WHERE household_id=$1)
        INSERT INTO porchlight.item_occurrences(household_id,item_id,seq,status,completed_at,starts_at,ends_at)
        SELECT $1,i.id,seq,CASE WHEN seq<90 THEN 'done' ELSE 'todo' END,
            CASE WHEN seq<90 THEN $2::timestamptz - (100-seq)*interval '1 day' END,
            $2::timestamptz - (100-seq)*interval '1 day' + i.n*interval '1 minute',
            $2::timestamptz - (100-seq)*interval '1 day' + i.n*interval '1 minute' + interval '30 seconds'
        FROM numbered i CROSS JOIN generate_series(0,99) seq")
        .bind(h).bind(now).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO porchlight.member_agenda(household_id,member_id,occurrence_id,item_id,starts_at,ends_at,busy) SELECT household_id,$2,id,item_id,starts_at,ends_at,during FROM porchlight.item_occurrences WHERE household_id=$1")
        .bind(h).bind(member).execute(&pool).await.unwrap();
    sqlx::raw_sql("ANALYZE porchlight.items; ANALYZE porchlight.item_occurrences; ANALYZE porchlight.member_agenda")
        .execute(&pool).await.unwrap();
    let caller = Caller::Agent {
        household_id: h,
        member_id: member,
        token_id: Uuid::nil(),
        label: "Plan probe".into(),
        scopes: vec!["admin".into()],
        request_id: Uuid::nil(),
    };
    let from = now - Duration::days(2);
    let to = now + Duration::days(2);
    let _ = tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(tracing_subscriber::filter::LevelFilter::TRACE),
    );
    let statements = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry()
        .with(Statements(statements.clone()))
        .with(tracing_subscriber::filter::LevelFilter::TRACE);
    async {
        let mut conn = s.state.pool.acquire().await.unwrap();
        board_rows(&mut conn, &caller, "household", now, None, 50)
            .await
            .unwrap();
        let _ = agenda(
            State(s.state.clone()),
            caller.clone(),
            Query(AgendaParams {
                from,
                to,
                member_id: None,
                cursor: None,
                limit: Some(50),
            }),
        )
        .await
        .unwrap();
        let mut tx = porchlight_db::tx::begin(&s.state.pool).await.unwrap();
        snapshot_candidates(&mut tx, h, member, true, from, to)
            .await
            .unwrap();
    }
    .with_subscriber(subscriber)
    .await;
    let statements = statements.lock().unwrap().clone();
    let mut output = String::new();
    for kind in ["board", "agenda", "snapshot"] {
        let sql = statements
            .iter()
            .find(|sql| match kind {
                "board" => sql.contains("with cards as"),
                "agenda" => sql.contains("'_agenda_key'"),
                _ => sql.contains("cross join lateral (select") && sql.contains("generated.lo"),
            })
            .unwrap_or_else(|| panic!("missing {kind} SQL: {statements:?}"));
        let explain = format!("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {sql}");
        let query = sqlx::query_scalar::<_, Value>(sqlx::AssertSqlSafe(explain.as_str()))
            .bind(h)
            .bind(member)
            .bind(true);
        let plan = match kind {
            "board" => query
                .bind(now)
                .bind("household")
                .bind(None::<String>)
                .bind(None::<String>)
                .bind(None::<Uuid>)
                .bind(51_i64)
                .fetch_one(&pool)
                .await
                .unwrap(),
            "agenda" => query
                .bind(member)
                .bind(from)
                .bind(to)
                .bind(None::<DateTime<Utc>>)
                .bind(None::<Uuid>)
                .bind(51_i64)
                .fetch_one(&pool)
                .await
                .unwrap(),
            _ => query.bind(from).bind(to).fetch_one(&pool).await.unwrap(),
        };
        output.push_str(&format!(
            "\n## {kind}\n{}\n",
            serde_json::to_string_pretty(&plan).unwrap()
        ));
        println!("{kind}: {} ms", plan[0]["Execution Time"]);
        fn scans_history(plan: &Value) -> bool {
            match plan {
                Value::Object(map) => {
                    (map.get("Node Type").and_then(Value::as_str) == Some("Seq Scan")
                        && map.get("Relation Name").and_then(Value::as_str)
                            == Some("item_occurrences"))
                        || map.values().any(scans_history)
                }
                Value::Array(rows) => rows.iter().any(scans_history),
                _ => false,
            }
        }
        assert!(!scans_history(&plan), "{kind} scans occurrence history");
    }
    // The function hides its inner plan; expose the identical body for diagnosis.
    let inner:Value=sqlx::query_scalar("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT o.* FROM porchlight.item_occurrences o JOIN porchlight.items i ON i.id=o.item_id WHERE o.household_id=$1 AND i.deleted_at IS NULL AND o.disposition<>'expired' AND ((o.status NOT IN ('done','skipped') AND NOT EXISTS(SELECT 1 FROM porchlight.item_occurrences earlier WHERE earlier.item_id=o.item_id AND earlier.status NOT IN ('done','skipped') AND earlier.disposition<>'expired' AND (earlier.seq,earlier.id)<(o.seq,o.id))) OR (o.status='done' AND o.completed_at>=$2-interval '7 days'))")
        .bind(h).bind(now).fetch_one(&pool).await.unwrap();
    output.push_str(&format!(
        "\n## board function body\n{}\n",
        serde_json::to_string_pretty(&inner).unwrap()
    ));
    println!("board function body: {} ms", inner[0]["Execution Time"]);
    std::fs::write(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var/srv-db-plans.txt"),
        output,
    )
    .unwrap();
    s.stop().await;
}
