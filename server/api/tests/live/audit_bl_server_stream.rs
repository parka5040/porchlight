use futures_util::StreamExt;
use porchlight_api::testing::{SseFrame, TestClient, TestOptions, TestServer, TestStream};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;

async fn event(stream: &mut TestStream) -> SseFrame {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frame = stream.next().await.expect("stream closed before event");
            if frame.event.is_some() {
                return frame;
            }
        }
    })
    .await
    .unwrap()
}
async fn publish(pool: &PgPool, s: &TestServer, seq: i64) {
    sqlx::query("insert into porchlight.outbox(household_id,kind,entity_type,entity_id,stream_seq) values($1,'member.changed','member',$2,$3)")
        .bind(s.init.household_id).bind(s.init.admin_member_id).bind(seq).execute(pool).await.unwrap();
    sqlx::query("select pg_notify('porchlight_stream','wake')")
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "../db/migrations")]
async fn transient_stream_sql_failure_retries_without_claiming_pruning(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            watch_revocations: false,
            db_pool_size: 1,
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    publish(&pool, &s, 1000).await;
    let mut stream = TestClient::new(&s.ca_pem, Some(&d))
        .stream(s.lan, d.request("GET", "/v1/stream", json!({}), None))
        .await
        .unwrap();
    assert_eq!(event(&mut stream).await.event.as_deref(), Some("ready"));
    sqlx::query("set lock_timeout='500ms'")
        .execute(&s.state.pool)
        .await
        .unwrap();
    let mut blocker = porchlight_db::tx::begin(&pool).await.unwrap();
    let pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    sqlx::query("lock table porchlight.stream_retention in access exclusive mode")
        .execute(&mut *blocker)
        .await
        .unwrap();
    publish(&pool, &s, 1001).await;
    porchlight_api::testing::wait_blocked_behind(&pool, pid, 1, Duration::from_secs(5))
        .await
        .unwrap();
    // Observe that the blocked query actually timed out before releasing the table;
    // no sleep-based guess about when the SSE task attempts its SQL.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "select exists(select 1 from pg_stat_activity where $1=any(pg_blocking_pids(pid)))",
            )
            .bind(pid)
            .fetch_one(&pool)
            .await
            .unwrap();
            if !waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    blocker.rollback().await.unwrap();
    let next = event(&mut stream).await;
    assert_eq!(
        next.event.as_deref(),
        Some("member.changed"),
        "SQL failure is not evidence of retention pruning"
    );
    assert_eq!(next.id.as_deref(), Some("1001"));
    drop(stream);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn held_stream_only_resnapshots_when_pruning_passes_its_cursor(pool: PgPool) {
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
    publish(&pool, &s, 1000).await;
    let mut stream = TestClient::new(&s.ca_pem, Some(&d))
        .stream(s.lan, d.request("GET", "/v1/stream", json!({}), None))
        .await
        .unwrap();
    assert_eq!(event(&mut stream).await.event.as_deref(), Some("ready"));
    sqlx::query("update porchlight.stream_retention set epoch=epoch+1,pruned_through=1000")
        .execute(&pool)
        .await
        .unwrap();
    publish(&pool, &s, 1001).await;
    let next = event(&mut stream).await;
    assert_eq!(
        next.event.as_deref(),
        Some("member.changed"),
        "pruned history already scanned must not invalidate the stream"
    );
    assert_eq!(next.id.as_deref(), Some("1001"));
    sqlx::query("update porchlight.stream_retention set epoch=epoch+1,pruned_through=1002")
        .execute(&pool)
        .await
        .unwrap();
    publish(&pool, &s, 1003).await;
    let next = event(&mut stream).await;
    assert_eq!(next.event.as_deref(), Some("resnapshot"));
    assert!(next.data.contains("retention_pruned"));
    drop(stream);
    s.stop().await;
}
