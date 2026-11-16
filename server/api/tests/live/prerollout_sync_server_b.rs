mod coverage;

use futures_util::StreamExt;
use porchlight_api::testing::{TestClient, TestDevice, TestOptions, TestServer, TestStream};
use porchlight_core::auth::session::Platform;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

async fn open(s: &TestServer, d: &TestDevice, capable: bool, after: i64) -> TestStream {
    let mut req = d.request("GET", &format!("/v1/stream?after={after}"), json!({}), None);
    if capable {
        req.headers_mut().insert(
            "Porchlight-Capabilities",
            "future, item-sync-v1".parse().unwrap(),
        );
    }
    let mut stream = TestClient::new(&s.ca_pem, Some(d))
        .stream(s.lan, req)
        .await
        .unwrap();
    assert_eq!(stream.status, 200);
    assert_eq!(stream.next().await.unwrap().event.as_deref(), Some("ready"));
    stream
}
async fn through(stream: &mut TestStream, id: Uuid) -> Vec<Value> {
    let mut frames = Vec::new();
    loop {
        let f = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap();
        if f.id.is_none() {
            continue;
        }
        let data: Value = serde_json::from_str(&f.data).unwrap();
        let done = data["entity_id"] == id.to_string();
        frames.push(data);
        if done {
            return frames;
        }
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn household_reset_is_terminal_unacknowledged_and_legacy_filtered(pool: PgPool) {
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
    while porchlight_db::outbox::assign_stream_seqs(&pool, 500)
        .await
        .unwrap()
        .is_some()
    {}
    let cursor = porchlight_db::stream::maximum(&pool).await.unwrap();
    let mut capable = open(&s, &d, true, cursor).await;
    let caught_up = tokio::time::timeout(Duration::from_secs(5), capable.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(caught_up.event.as_deref(), Some("caught_up"));
    assert!(caught_up.id.is_none());
    assert_eq!(
        serde_json::from_str::<Value>(&caught_up.data).unwrap(),
        json!({"cursor":cursor})
    );
    let mut legacy = open(&s, &d, false, cursor).await;
    sqlx::query("update porchlight.households set name='Updated household' where id=$1")
        .bind(s.init.household_id)
        .execute(&pool)
        .await
        .unwrap();
    let barrier = Uuid::now_v7();
    sqlx::query("insert into porchlight.outbox(household_id,kind,entity_type,entity_id) values($1,'member.changed','member',$2)")
        .bind(s.init.household_id).bind(barrier).execute(&pool).await.unwrap();
    let f = tokio::time::timeout(Duration::from_secs(5), capable.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(f.event.as_deref(), Some("resnapshot"));
    assert!(f.id.is_none());
    assert_eq!(
        serde_json::from_str::<Value>(&f.data).unwrap(),
        json!({"reason":"projection_changed"})
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(5), capable.next())
            .await
            .unwrap()
            .is_none()
    );
    let old = through(&mut legacy, barrier).await;
    assert_eq!(old.len(), 1);
    assert_eq!(old[0]["kind"], "member.changed");
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn capability_negotiates_authoritative_parent_and_keeps_legacy_frames(pool: PgPool) {
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
    let item: Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind) values($1,$2,'Parent','todo') returning id")
        .bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    while porchlight_db::outbox::assign_stream_seqs(&pool, 500)
        .await
        .unwrap()
        .is_some()
    {}
    let cursor = porchlight_db::stream::maximum(&pool).await.unwrap();
    let mut capable = open(&s, &d, true, cursor).await;
    let mut legacy = open(&s, &d, false, cursor).await;
    let comment: Uuid = sqlx::query_scalar("insert into porchlight.comments(household_id,item_id,author_id,body) values($1,$2,$3,'Comment') returning id")
        .bind(s.init.household_id).bind(item).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    let new = through(&mut capable, comment).await;
    let old = through(&mut legacy, comment).await;
    assert_eq!(new.last().unwrap()["item_id"], item.to_string());
    assert_eq!(old.len(), 1);
    let fields = old[0].as_object().unwrap();
    assert_eq!(fields.len(), 4, "legacy wire fields must not change");
    assert!(fields.get("item_id").is_none());
    s.stop().await;
}
