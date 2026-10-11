use chrono::{DateTime, Utc};
use porchlight_api::{
    calendar::{ImportSummary, Source},
    extract::Caller,
    testing::{TestOptions, TestServer},
};
use porchlight_db::tx::{Actor, begin_as};
use sqlx::PgPool;
use uuid::Uuid;

const EXPANDED: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:shared\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nRRULE:FREQ=DAILY;COUNT=3;BYHOUR=9\r\nSUMMARY:School\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
const EMPTY: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";
fn now() -> DateTime<Utc> {
    "2026-10-06T12:00:00Z".parse().unwrap()
}
async fn source(s: &TestServer, key: &str) -> Source {
    let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    let id: Uuid = sqlx::query_scalar("insert into porchlight.ics_sources(household_id,owner_member_id,target_member_id,kind,import_key,name,url) values($1,$2,$2,'feed',$3,$3,'https://example.invalid/calendar') returning id")
        .bind(s.init.household_id).bind(s.init.admin_member_id).bind(key).fetch_one(&mut *tx).await.unwrap();
    let source = porchlight_api::calendar::load_source(&mut tx, s.init.household_id, id, true)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    source
}
async fn apply(
    s: &TestServer,
    source: &Source,
    content: &str,
    now: DateTime<Utc>,
) -> ImportSummary {
    let caller = Caller::Agent {
        token_id: Uuid::nil(),
        member_id: s.init.admin_member_id,
        label: "test".into(),
        scopes: vec!["admin".into()],
        household_id: s.init.household_id,
        request_id: Uuid::now_v7(),
    };
    let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    let result = porchlight_api::items::ics_import::apply_calendar(
        &mut tx, &caller, source, content, true, now,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    result
}
async fn live(pool: &PgPool, source: Uuid) -> i64 {
    sqlx::query_scalar(
        "select count(*) from porchlight.items where ics_source_id=$1 and deleted_at is null",
    )
    .bind(source)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn uid_is_source_scoped_and_expansion_window_never_licenses_history_deletion(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let a = source(&s, "a").await;
    let b = source(&s, "b").await;
    assert_eq!(apply(&s, &a, EXPANDED, now()).await.created, 3);
    assert_eq!(apply(&s, &b, EXPANDED, now()).await.created, 3);
    assert_eq!(live(&pool, a.id).await, 3);
    assert_eq!(live(&pool, b.id).await, 3);
    let summary = apply(&s, &a, EMPTY, "2028-01-01T12:00:00Z".parse().unwrap()).await;
    assert_eq!(
        summary.deleted, 0,
        "absence outside the exact expansion window is not removal"
    );
    assert_eq!(live(&pool, a.id).await, 3);
    let invalid = EMPTY.replace(
        "END:VCALENDAR",
        "BEGIN:VEVENT\r\nUID:bad\r\nDTSTART:not-a-date\r\nEND:VEVENT\r\nEND:VCALENDAR",
    );
    let summary = apply(&s, &a, &invalid, now()).await;
    assert!(summary.removal_suppressed);
    assert_eq!(
        summary.deleted, 3,
        "a bad UID cannot preserve unrelated removed events"
    );
    assert_eq!(live(&pool, a.id).await, 0);
    assert_eq!(apply(&s, &a, EMPTY, now()).await.deleted, 0);
    assert_eq!(live(&pool, a.id).await, 0);
    assert_eq!(live(&pool, b.id).await, 3);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn semantically_incomplete_feed_does_not_remove_source_exception(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let source = source(&s, "exceptions").await;
    let retained = EXPANDED.replace(
        "FREQ=DAILY;COUNT=3;BYHOUR=9",
        "FREQ=DAILY;COUNT=3\r\nEXDATE:20261013T090000Z",
    );
    assert_eq!(apply(&s, &source, &retained, now()).await.skipped, 1);
    let incomplete = retained.replace("EXDATE:20261013T090000Z", "EXDATE:broken-date");
    assert!(
        apply(&s, &source, &incomplete, now())
            .await
            .removal_suppressed
    );
    let status: String = sqlx::query_scalar("select o.status from porchlight.item_occurrences o join porchlight.items i on i.id=o.item_id where i.ics_source_id=$1 and o.seq=1").bind(source.id).fetch_one(&pool).await.unwrap();
    assert_eq!(status, "skipped");
    s.stop().await;
}
