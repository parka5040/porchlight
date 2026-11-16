use porchlight_api::{
    extract::Caller,
    items,
    routes::items::OccurrencesQuery,
    testing::{TestOptions, TestServer},
};
use porchlight_db::derive;
use sqlx::PgPool;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tracing::{Subscriber, instrument::WithSubscriber};
use tracing_subscriber::{
    Layer,
    layer::{Context, SubscriberExt},
};
use uuid::Uuid;

struct Rows(Arc<AtomicU64>);
impl<S: Subscriber> Layer<S> for Rows {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        struct Visitor(u64);
        impl tracing::field::Visit for Visitor {
            fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
                if field.name() == "rows_returned" {
                    self.0 += value;
                }
            }
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
        }
        if event.metadata().target() == "sqlx::query" {
            let mut v = Visitor(0);
            event.record(&mut v);
            self.0.fetch_add(v.0, Ordering::Relaxed);
        }
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn occurrence_pages_match_full_series_and_decode_bounded_rows(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let c = Caller::Agent {
        household_id: s.init.household_id,
        member_id: s.init.admin_member_id,
        token_id: Uuid::nil(),
        label: "Page probe".into(),
        scopes: vec!["admin".into()],
        request_id: Uuid::nil(),
    };
    let id:Uuid=sqlx::query_scalar("INSERT INTO porchlight.items(household_id,owner_id,title,kind,time_mode,local_due,recurrence_mode,rrule) VALUES($1,$2,'Fold series','deadline','floating','2020-01-01 01:30','calendar','FREQ=DAILY;COUNT=5000') RETURNING id")
        .bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO porchlight.item_occurrences(household_id,item_id,seq,local_due,override_local_due) SELECT $1,$2,n,'2020-01-01 01:30'::timestamp+n*interval '1 day',CASE WHEN n=1 THEN '2026-11-01 01:30'::timestamp WHEN n=2496 THEN '2035-01-01'::timestamp END FROM generate_series(0,4999) n")
        .bind(s.init.household_id).bind(id).execute(&pool).await.unwrap();
    let zone = chrono_tz::America::New_York;
    let from = "2026-11-01T05:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let to = from + chrono::Duration::days(3);
    let mut conn = pool.acquire().await.unwrap();
    let parent = derive::load_item(&mut conn, id).await.unwrap();
    let spec = parent.spec().unwrap();
    let mut full = derive::occurrences(&mut conn, id).await.unwrap();
    full.sort_by_key(|o| o.anchor.seq);
    let caps: Vec<String> = items::resource_capabilities(&mut conn, &c, id)
        .await
        .unwrap()
        .into_iter()
        .map(|cap| cap.as_str().into())
        .collect();
    let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM porchlight.item_derivation WHERE item_id=$1 AND requested_gen>applied_gen)").bind(id).fetch_one(&mut *conn).await.unwrap();
    let wire = |o: &derive::OccurrenceRow| {
        let a = derive::effective_anchor(o, &spec).unwrap();
        serde_json::to_value(porchlight_api::routes::items::OccurrenceResponse {
            id: o.id,
            item_id: o.item_id,
            seq: a.seq,
            version: o.version,
            status: o.status.as_str().into(),
            disposition: o.disposition.as_str().into(),
            board_position: o.board_position.clone(),
            completed_by: o.completed_by,
            completed_at: o.completed_at,
            local_start: a.local_start,
            local_end: a.local_end,
            local_due: a.local_due,
            starts_at: a.starts_at,
            ends_at: a.ends_at,
            due_at: a.due_at,
            due_date: a.due_date,
            override_local_start: o.override_local_start,
            override_local_end: o.override_local_end,
            override_local_due: o.override_local_due,
            override_due_date: o.override_due_date,
            is_detached: o.is_detached,
            capabilities: caps.clone(),
            pending_derivation: pending,
            conflicts: vec![],
        })
        .unwrap()
    };
    let expected: Vec<_> = full
        .iter()
        .filter(|o| {
            let anchor = derive::effective_anchor(o, &spec).unwrap();
            let at = porchlight_core::time::resolve_local(anchor.local_due.unwrap(), zone);
            at >= from && at < to
        })
        .map(wire)
        .collect();
    assert!(
        expected.contains(&wire(&full[1])),
        "moved-in override survives SQL prefilter"
    );
    drop(conn);
    let _ = tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(tracing_subscriber::filter::LevelFilter::TRACE),
    );
    for window in [false, true] {
        let expected = if window {
            expected.clone()
        } else {
            full.iter().take(50).map(wire).collect()
        };
        let mut cursor = None;
        let mut got = Vec::new();
        for _ in 0..if window { 10 } else { 2 } {
            let rows = Arc::new(AtomicU64::new(0));
            let page = items::occurrences_with_zone(
                &s.state,
                &c,
                id,
                OccurrencesQuery {
                    from: window.then_some(from),
                    to: window.then_some(to),
                    undated: None,
                    cursor,
                    limit: Some(if window { 2 } else { 25 }),
                },
                Some(zone),
            )
            .with_subscriber(
                tracing_subscriber::registry()
                    .with(Rows(rows.clone()))
                    .with(tracing_subscriber::filter::LevelFilter::TRACE),
            )
            .await
            .unwrap();
            let loaded = rows.load(Ordering::Relaxed);
            assert!(
                loaded > 0 && loaded < 256,
                "rows decoded per page: {loaded}"
            );
            got.extend(page.items.iter().map(|o| serde_json::to_value(o).unwrap()));
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(
            got, expected,
            "full-materialisation equivalence, window={window}"
        );
    }
    s.stop().await;
}
