use axum::{Json, extract::State};
use porchlight_api::extract::ApiQuery as Query;
use porchlight_api::{
    extract::Caller,
    list_types::ListsQuery,
    lists,
    routes::reads::{self, SearchParams},
    testing::{TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use sqlx::PgPool;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tracing::{Subscriber, instrument::WithSubscriber};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt},
};

struct Counter(Arc<AtomicUsize>);
impl<S: Subscriber> Layer<S> for Counter {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        // Count application statements, not pool liveness/transaction cleanup.
        struct Query(bool);
        impl tracing::field::Visit for Query {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name() == "db.statement" && value.contains("porchlight.") {
                    self.0 = true;
                }
            }
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
        }
        if event.metadata().target() == "sqlx::query" {
            let mut query = Query(false);
            event.record(&mut query);
            if query.0 {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
async fn counted<T>(future: impl std::future::Future<Output = T>) -> (usize, T) {
    let _ = tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(tracing_subscriber::filter::LevelFilter::TRACE),
    );
    let count = Arc::new(AtomicUsize::new(0));
    let result = future
        .with_subscriber(
            tracing_subscriber::registry()
                .with(Counter(count.clone()))
                .with(tracing_subscriber::filter::LevelFilter::TRACE),
        )
        .await;
    (count.load(Ordering::Relaxed), result)
}
fn caller(s: &TestServer) -> Caller {
    Caller::Agent {
        household_id: s.init.household_id,
        member_id: s.init.admin_member_id,
        token_id: uuid::Uuid::nil(),
        label: "Measured".into(),
        scopes: vec!["admin".into()],
        request_id: uuid::Uuid::nil(),
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unchanged_zone_report_is_one_read(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let actor = porchlight_db::tx::Actor::system();
    porchlight_db::members::report_device_zone(
        &s.state.pool,
        &actor,
        s.init.admin_member_id,
        d.device_id,
        chrono_tz::UTC,
    )
    .await
    .unwrap();
    let (count, result) = counted(porchlight_db::members::report_device_zone(
        &s.state.pool,
        &actor,
        s.init.admin_member_id,
        d.device_id,
        chrono_tz::UTC,
    ))
    .await;
    assert!(!result.unwrap());
    assert_eq!(count, 1, "unchanged zone report statements");
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn list_collection_snapshot_search_statement_counts_are_constant(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let c = caller(&s);
    let mut counts = Vec::new();
    for n in [5, 45] {
        sqlx::query("WITH parents AS (INSERT INTO porchlight.items(household_id,owner_id,title,kind) SELECT $1,$2,'List parent','todo' FROM generate_series(1,$3) RETURNING id), numbered AS (SELECT id,row_number() OVER(ORDER BY id) AS n FROM parents) INSERT INTO porchlight.lists(household_id,owner_id,item_id,name,mode,visibility) SELECT $1,$2,CASE WHEN n%2=0 THEN id END,'Measured list','persistent','household' FROM numbered")
            .bind(s.init.household_id).bind(s.init.admin_member_id).bind(n).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO porchlight.list_viewers(household_id,list_id,member_id) SELECT household_id,id,$2 FROM porchlight.lists WHERE household_id=$1 AND item_id IS NULL ON CONFLICT DO NOTHING")
            .bind(s.init.household_id).bind(s.init.admin_member_id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO porchlight.list_entries(household_id,list_id,label) SELECT household_id,id,'Measured entry' FROM porchlight.lists l WHERE household_id=$1 AND NOT EXISTS(SELECT 1 FROM porchlight.list_entries e WHERE e.list_id=l.id)")
            .bind(s.init.household_id).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO porchlight.list_entry_checks(household_id,entry_id,checked_by) SELECT household_id,id,$2 FROM porchlight.list_entries WHERE household_id=$1 ON CONFLICT DO NOTHING")
            .bind(s.init.household_id).bind(s.init.admin_member_id).execute(&pool).await.unwrap();
        let mut tx = porchlight_db::tx::begin(&s.state.pool).await.unwrap();
        let (collection, page) = counted(lists::collection(
            &s.state,
            &mut tx,
            &c,
            ListsQuery {
                limit: Some(100),
                ..Default::default()
            },
        ))
        .await;
        let page = page.unwrap();
        let (snapshot, full) = counted(lists::snapshot_contribution(&mut tx, &c)).await;
        let full = full.unwrap();
        assert_eq!(
            serde_json::to_value(&page.items).unwrap(),
            serde_json::to_value(&full).unwrap()
        );
        for value in &full {
            let single = lists::response(&mut tx, &c, value.fields.id, None, false)
                .await
                .unwrap();
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                serde_json::to_value(single).unwrap()
            );
        }
        let _ = reads::search(
            State(s.state.clone()),
            c.clone(),
            Query(SearchParams {
                q: "Measured".into(),
                cursor: None,
                limit: Some(100),
            }),
        )
        .await
        .unwrap();
        let (search, result) = counted(reads::search(
            State(s.state.clone()),
            c.clone(),
            Query(SearchParams {
                q: "Measured".into(),
                cursor: None,
                limit: Some(100),
            }),
        ))
        .await;
        let Json(result) = result.unwrap();
        assert_eq!(result.items.len(), full.len());
        counts.push((collection, snapshot, search));
    }
    assert_eq!(counts[0], counts[1], "5 vs 50 lists: {counts:?}");
    assert!(counts[0].0 > 0);
    s.stop().await;
}
