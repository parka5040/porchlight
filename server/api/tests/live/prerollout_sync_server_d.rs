//! Poll the shipping response body directly so publishing/revocation can interleave
//! deterministically with replay without a socket buffering the whole backlog.
use axum::body::Body;
use http_body_util::BodyExt;
use porchlight_api::{
    routes::mtls_router,
    server::{ConnInfo, Transport},
    testing::{TestDevice, TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

async fn setup(pool: &PgPool) -> (TestServer, TestDevice, i64) {
    setup_with_heartbeat(pool, Duration::from_secs(30)).await
}
async fn setup_with_heartbeat(pool: &PgPool, heartbeat: Duration) -> (TestServer, TestDevice, i64) {
    let mut opts = TestOptions {
        background_derivation: false,
        ..Default::default()
    };
    opts.settings.heartbeat = heartbeat;
    let s = TestServer::start_with(pool, opts).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    while porchlight_db::outbox::assign_stream_seqs(pool, 500)
        .await
        .unwrap()
        .is_some()
    {}
    let cursor = porchlight_db::stream::maximum(pool).await.unwrap();
    (s, d, cursor)
}

async fn open(s: &TestServer, d: &TestDevice, after: i64, capability: &str) -> Body {
    let mut req = d
        .request("GET", &format!("/v1/stream?after={after}"), json!({}), None)
        .map(Body::new);
    req.headers_mut()
        .insert("Porchlight-Capabilities", capability.parse().unwrap());
    req.extensions_mut().insert(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(d.fingerprint),
    });
    let response = mtls_router(s.state.clone()).oneshot(req).await.unwrap();
    assert_eq!(response.status(), 200);
    response.into_body()
}

async fn chunk(body: &mut Body) -> String {
    let frame = tokio::time::timeout(Duration::from_secs(3), body.frame())
        .await
        .expect("catch-up must not wait for a heartbeat")
        .expect("stream closed")
        .unwrap();
    String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap()
}
fn control(kind: &str, cursor: i64) -> String {
    format!("event: {kind}\ndata: {{\"cursor\":{cursor}}}\n\n")
}
async fn seed(pool: &PgPool, s: &TestServer, first: i64, count: i64, member: Uuid) {
    sqlx::query("insert into porchlight.outbox(household_id,kind,entity_type,entity_id,payload,stream_seq) select $1,'settings.changed','member_settings',$2,jsonb_build_object('member_id',$3::text),$4+n from generate_series(0,$5-1) n")
        .bind(s.init.household_id).bind(member).bind(member).bind(first).bind(count).execute(pool).await.unwrap();
}
fn ids(chunk: &str) -> Vec<i64> {
    chunk
        .lines()
        .filter_map(|line| line.strip_prefix("id: ").map(|id| id.parse().unwrap()))
        .collect()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn empty_backlog_is_ready_then_caught_up_once(pool: PgPool) {
    let (s, d, cursor) = setup(&pool).await;
    let mut body = open(&s, &d, cursor, "item-sync-v1").await;
    assert_eq!(chunk(&mut body).await, control("ready", cursor));
    assert_eq!(chunk(&mut body).await, control("caught_up", cursor));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), body.frame())
            .await
            .is_err(),
        "caught_up is sent only once"
    );
    drop(body);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn filtered_backlog_and_concurrent_publish_preserve_connect_watermark(pool: PgPool) {
    let (s, d, cursor) = setup(&pool).await;
    let first = cursor + 1000;
    // A wholly invisible batch, two visible batches, then an invisible tail.
    seed(&pool, &s, first, 500, Uuid::nil()).await;
    seed(&pool, &s, first + 500, 1001, s.init.admin_member_id).await;
    seed(&pool, &s, first + 1501, 500, Uuid::nil()).await;
    let watermark = first + 2000;
    let mut body = open(&s, &d, cursor, "future, item-sync-v1").await;
    assert_eq!(chunk(&mut body).await, control("ready", cursor));
    let mut seen = ids(&chunk(&mut body).await);
    assert_eq!(seen, (first + 500..first + 1000).collect::<Vec<_>>());
    // The response has flushed just one visible batch; the rest is still replay.
    seed(&pool, &s, watermark + 1, 20, s.init.admin_member_id).await;
    loop {
        let frame = chunk(&mut body).await;
        if frame.starts_with("event: caught_up\n") {
            assert_eq!(frame, control("caught_up", watermark));
            break;
        }
        assert!(
            !frame.starts_with(":"),
            "short replay must not interleave a heartbeat"
        );
        seen.extend(ids(&frame));
    }
    assert_eq!(
        seen.iter()
            .copied()
            .filter(|seq| *seq <= watermark)
            .collect::<Vec<_>>(),
        (first + 500..=first + 1500).collect::<Vec<_>>()
    );
    drop(body);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn wholly_filtered_backlog_still_catches_up(pool: PgPool) {
    let (s, d, cursor) = setup(&pool).await;
    seed(&pool, &s, cursor + 1, 1001, Uuid::nil()).await;
    let mut body = open(&s, &d, cursor, "item-sync-v1").await;
    assert_eq!(chunk(&mut body).await, control("ready", cursor));
    assert_eq!(chunk(&mut body).await, control("caught_up", cursor + 1001));
    drop(body);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn global_watermark_covers_other_households_without_local_events(pool: PgPool) {
    let (s, d, cursor) = setup(&pool).await;
    let other: Uuid = sqlx::query_scalar(
        "insert into porchlight.households(name) values('Other household') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let watermark = cursor + 1000;
    sqlx::query("insert into porchlight.outbox(household_id,kind,entity_type,entity_id,stream_seq) values($1,'member.changed','member',$2,$3)")
        .bind(other).bind(Uuid::nil()).bind(watermark).execute(&pool).await.unwrap();
    let mut body = open(&s, &d, cursor, "item-sync-v1").await;
    assert_eq!(chunk(&mut body).await, control("ready", cursor));
    assert_eq!(chunk(&mut body).await, control("caught_up", watermark));
    drop(body);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn slow_replay_allows_heartbeat_but_finishes_backlog_first(pool: PgPool) {
    let (s, d, cursor) = setup_with_heartbeat(&pool, Duration::from_millis(100)).await;
    seed(&pool, &s, cursor + 1, 501, s.init.admin_member_id).await;
    let mut body = open(&s, &d, cursor, "item-sync-v1").await;
    assert_eq!(chunk(&mut body).await, control("ready", cursor));
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(chunk(&mut body).await, ": hb\n\n");
    let mut seen = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let frame = chunk(&mut body).await;
            if frame.starts_with("event: caught_up\n") {
                assert_eq!(frame, control("caught_up", cursor + 501));
                break;
            }
            seen.extend(ids(&frame));
        }
    })
    .await
    .unwrap();
    assert_eq!(seen, (cursor + 1..=cursor + 501).collect::<Vec<_>>());
    drop(body);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn drain_and_revocation_win_over_pending_caught_up(pool: PgPool) {
    let (s, d, cursor) = setup(&pool).await;
    seed(&pool, &s, cursor + 1, 1, s.init.admin_member_id).await;
    let mut revoked = open(&s, &d, cursor, "item-sync-v1").await;
    assert_eq!(chunk(&mut revoked).await, control("ready", cursor));
    assert_eq!(ids(&chunk(&mut revoked).await), vec![cursor + 1]);
    s.state.revocations.send(d.device_id).unwrap();
    assert!(revoked.frame().await.is_none());
    let mut draining = open(&s, &d, cursor + 1, "item-sync-v1").await;
    assert_eq!(chunk(&mut draining).await, control("ready", cursor + 1));
    s.state.hub.drain();
    assert_eq!(
        chunk(&mut draining).await,
        "event: drain\ndata: {\"reason\":\"shutdown\"}\n\n"
    );
    assert!(draining.frame().await.is_none());
    drop((revoked, draining));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn legacy_stream_bytes_are_unchanged(pool: PgPool) {
    let (s, d, cursor) = setup(&pool).await;
    seed(&pool, &s, cursor + 1, 1, s.init.admin_member_id).await;
    let mut body = open(&s, &d, cursor, "future").await;
    assert_eq!(chunk(&mut body).await, control("ready", cursor));
    let id = s.init.admin_member_id;
    let seq = cursor + 1;
    assert_eq!(
        chunk(&mut body).await,
        format!(
            "id: {seq}\nevent: settings.changed\ndata: {{\"seq\":{seq},\"kind\":\"settings.changed\",\"entity_type\":\"member_settings\",\"entity_id\":\"{id}\"}}\n\n"
        )
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), body.frame())
            .await
            .is_err()
    );
    drop(body);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn invalid_and_expired_cursors_only_resnapshot(pool: PgPool) {
    let (s, d, cursor) = setup(&pool).await;
    seed(&pool, &s, cursor + 1, 2, s.init.admin_member_id).await;
    sqlx::query(
        "update porchlight.outbox set created_at=now()-interval '8 days' where stream_seq=$1",
    )
    .bind(cursor + 1)
    .execute(&pool)
    .await
    .unwrap();
    for (after, reason) in [
        (cursor + 1, "cursor_expired"),
        (cursor + 3, "cursor_unknown"),
    ] {
        let mut body = open(&s, &d, after, "item-sync-v1").await;
        assert_eq!(
            chunk(&mut body).await,
            format!("event: resnapshot\ndata: {{\"reason\":\"{reason}\"}}\n\n")
        );
        assert!(body.frame().await.is_none());
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn revocation_during_replay_wins_over_caught_up(pool: PgPool) {
    let (s, d, cursor) = setup(&pool).await;
    seed(&pool, &s, cursor + 1, 1001, s.init.admin_member_id).await;
    let mut body = open(&s, &d, cursor, "item-sync-v1").await;
    assert_eq!(chunk(&mut body).await, control("ready", cursor));
    assert_eq!(ids(&chunk(&mut body).await).len(), 500);
    s.state.revocations.send(d.device_id).unwrap();
    assert!(
        body.frame().await.is_none(),
        "revocation must suppress remaining replay and caught_up"
    );
    drop(body);
    s.stop().await;
}
