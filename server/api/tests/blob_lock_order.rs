//! Deterministic lock-footprint proof for comment attachment bulk deletion vs GC/backup.
use porchlight_api::testing::{TestClient, TestOptions, TestServer};
use porchlight_core::auth::session::Platform;
use porchlight_db::tx::{Actor, begin_as};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn comment_delete_prelocks_entire_blob_union_in_hash_order(pool: PgPool) {
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
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let item = c
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/items",
                json!({"title":"Blob lock ordering"}),
                None,
            ),
        )
        .await
        .unwrap()
        .json();
    let item: Uuid = item["id"].as_str().unwrap().parse().unwrap();
    let comment = c
        .request(
            s.lan,
            d.request(
                "POST",
                &format!("/v1/items/{item}/comments"),
                json!({"body":"Seed"}),
                None,
            ),
        )
        .await
        .unwrap()
        .json();
    let comment: Uuid = comment["id"].as_str().unwrap().parse().unwrap();
    let low = "a".repeat(64);
    let high = "b".repeat(64);
    let mut seed = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    // Attachment insertion order opposes the global blob order. No files are needed for this lock proof.
    for hash in [&high, &low] {
        sqlx::query("INSERT INTO porchlight.blobs(household_id,sha256,size,content_type) VALUES($1,$2,0,'text/plain')")
            .bind(s.init.household_id).bind(hash).execute(&mut *seed).await.unwrap();
        sqlx::query("INSERT INTO porchlight.attachments(household_id,item_id,comment_id,blob_sha256,original_name,uploader_id) VALUES($1,$2,$3,$4,'seed.txt',$5)")
            .bind(s.init.household_id).bind(item).bind(comment).bind(hash).bind(s.init.admin_member_id).execute(&mut *seed).await.unwrap();
    }
    seed.commit().await.unwrap();
    let mut collector = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *collector)
        .await
        .unwrap();
    sqlx::query(
        "SELECT sha256 FROM porchlight.blobs WHERE household_id=$1 AND sha256=$2 FOR UPDATE",
    )
    .bind(s.init.household_id)
    .bind(&low)
    .fetch_one(&mut *collector)
    .await
    .unwrap();
    let mut req = d.request(
        "DELETE",
        &format!("/v1/comments/{comment}"),
        json!({}),
        None,
    );
    req.headers_mut().insert("if-match", "1".parse().unwrap());
    let addr = s.lan;
    let deletion = tokio::spawn(async move { c.request(addr, req).await.unwrap() });
    // Observe the actual wait, rather than hoping a sleep placed the DELETE at its blob acquisition.
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let blocked: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))",
            )
            .bind(pid)
            .fetch_one(&pool)
            .await
            .unwrap();
            if blocked {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("comment deletion must reach the collector's first hash");
    // A per-attachment loop would hold high while waiting on low: the classic cycle.
    let second_lock = sqlx::query(
        "SELECT sha256 FROM porchlight.blobs WHERE household_id=$1 AND sha256=$2 FOR UPDATE NOWAIT",
    )
    .bind(s.init.household_id)
    .bind(&high)
    .fetch_one(&mut *collector)
    .await;
    collector.rollback().await.unwrap();
    assert_eq!(deletion.await.unwrap().status, 204);
    assert!(
        second_lock.is_ok(),
        "bulk deletion must not hold a later hash while waiting on an earlier one"
    );
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.attachments WHERE comment_id=$1 AND deleted_at IS NULL",
    )
    .bind(comment)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live, 0);
    s.stop().await;
}
