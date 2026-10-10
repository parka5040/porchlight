//! PAT contention must be an unsealed retry, not merely a DB-level lock failure.
use porchlight_api::testing::{TestServer, agent_request, trust};
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrations = "../db/migrations")]
async fn pat_revoke_lock_timeout_is_unstored_and_same_key_retries(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let (target, _) = trust::seed_pat(&pool, &s, &["admin"]).await;
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("select id from porchlight.api_tokens where id=$1 for no key update")
        .bind(target)
        .execute(&mut *holder)
        .await
        .unwrap();
    let path = format!("/v1/admin/tokens/{target}/revoke");
    let send = || trust::req("POST", &path, json!({}), "m3b-pat-retry");
    let r = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        porchlight_api::testing::agent_request_within(
            &s.agent_socket,
            Some(s.bootstrap_token()),
            send(),
            std::time::Duration::from_secs(15),
        ),
    )
    .await
    .unwrap();
    assert_eq!(r.status, 409, "{r:?}");
    assert_eq!(r.headers["retry-after"], "1");
    assert_eq!(r.json()["type"], "urn:porchlight:problem:conflict");
    let claims: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.idempotency_keys where idem_key='m3b-pat-retry'",
    )
    .fetch_one(&mut *holder)
    .await
    .unwrap();
    assert_eq!(claims, 0, "transient claim must roll back, never seal");
    let live: bool =
        sqlx::query_scalar("select revoked_at is null from porchlight.api_tokens where id=$1")
            .bind(target)
            .fetch_one(&mut *holder)
            .await
            .unwrap();
    assert!(live);
    holder.rollback().await.unwrap();
    let success = agent_request(&s.agent_socket, Some(s.bootstrap_token()), send()).await;
    assert_eq!(success.status, 200, "{success:?}");
    let replay = agent_request(&s.agent_socket, Some(s.bootstrap_token()), send()).await;
    assert_eq!(replay.status, 200);
    assert_eq!(success.body, replay.body);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn busy_pat_revoke_queues_ahead_of_new_claims(pool: PgPool) {
    use porchlight_api::testing::wait_blocked_behind;
    use porchlight_db::{
        testing::{Role, pool_as},
        tokens,
    };
    use std::time::Duration;
    let s = TestServer::start(&pool).await;
    let (target, _) = trust::seed_pat(&pool, &s, &["admin"]).await;
    let app = pool_as(&pool, Role::App, None).await.unwrap();
    let mut holder = app.begin().await.unwrap();
    tokens::share_pat(&mut holder, target)
        .await
        .unwrap()
        .unwrap();
    let holder_pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    let path = format!("/v1/admin/tokens/{target}/revoke");
    let socket = s.agent_socket.clone();
    let caller_token = s.bootstrap_token().to_owned();
    let revoke = agent_request(
        &socket,
        Some(&caller_token),
        trust::req("POST", &path, json!({}), "busy-pat-revoke"),
    );
    tokio::pin!(revoke);
    tokio::select! {
        response = &mut revoke => panic!("busy revoke must queue, not return {}", response.status),
        blocked = wait_blocked_behind(&pool, holder_pid, 1, Duration::from_secs(15)) => blocked.unwrap(),
    }
    // A fresh request arrives while the PAT is continuously busy. FIFO lock
    // acquisition must put the revoker first, rather than polling for a gap.
    let mut late = app.begin().await.unwrap();
    let late_pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
        .fetch_one(&mut *late)
        .await
        .unwrap();
    let late_claim = async {
        let identity = tokens::share_pat(&mut late, target).await.unwrap();
        late.rollback().await.unwrap();
        identity
    };
    tokio::pin!(late_claim);
    let late_blocked = async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let blocked: bool = sqlx::query_scalar("select cardinality(pg_blocking_pids($1))>0")
                .bind(late_pid)
                .fetch_one(&pool)
                .await
                .unwrap();
            if blocked {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "late claim must reach its lock wait"
            );
            tokio::task::yield_now().await;
        }
    };
    tokio::select! {
        _ = &mut late_claim => panic!("late claim bypassed holder"),
        () = late_blocked => {},
        response = &mut revoke => panic!("revoker bypassed holder: {}", response.status),
    }
    holder.commit().await.unwrap();
    let (response, identity) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(&mut revoke, &mut late_claim)
    })
    .await
    .unwrap();
    assert_eq!(response.status, 200, "{response:?}");
    assert!(
        identity.is_none(),
        "queued new request must observe revocation"
    );
    let replay = agent_request(
        &s.agent_socket,
        Some(s.bootstrap_token()),
        trust::req("POST", &path, json!({}), "busy-pat-revoke"),
    )
    .await;
    assert_eq!(replay.status, 200);
    assert_eq!(replay.body, response.body);
    app.close().await;
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reciprocal_api_pat_revokes_use_ordered_reservations(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    for reverse in [false, true].into_iter().cycle().take(6) {
        let (a, ta) = trust::seed_pat(&pool, &s, &["admin"]).await;
        let (b, tb) = trust::seed_pat(&pool, &s, &["admin"]).await;
        let one = trust::req(
            "POST",
            &format!("/v1/admin/tokens/{b}/revoke"),
            json!({}),
            "cross-one",
        );
        let two = trust::req(
            "POST",
            &format!("/v1/admin/tokens/{a}/revoke"),
            json!({}),
            "cross-two",
        );
        let (one, two) = if reverse { (two, one) } else { (one, two) };
        let (ta, tb) = if reverse { (tb, ta) } else { (ta, tb) };
        // Both ordered reservations must reach the lock queue before release.
        // Hold the first UUID so neither request can serialize unnoticed.
        use porchlight_api::testing::{agent_request_within, wait_blocked_behind};
        use std::time::Duration;
        let mut holder = pool.begin().await.unwrap();
        sqlx::query("select id from porchlight.api_tokens where id=$1 for no key update")
            .bind(a.min(b))
            .execute(&mut *holder)
            .await
            .unwrap();
        let pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
            .fetch_one(&mut *holder)
            .await
            .unwrap();
        let first = agent_request_within(&s.agent_socket, Some(&ta), one, Duration::from_secs(30));
        tokio::pin!(first);
        tokio::select! {
            r = &mut first => panic!("first revoke did not queue: {}", r.status),
            blocked = wait_blocked_behind(&pool, pid, 1, Duration::from_secs(15)) => blocked.unwrap(),
        }
        let second = agent_request_within(&s.agent_socket, Some(&tb), two, Duration::from_secs(30));
        tokio::pin!(second);
        tokio::select! {
            r = &mut first => panic!("first revoke escaped holder: {}", r.status),
            r = &mut second => panic!("reciprocal revoke did not overlap: {}", r.status),
            blocked = wait_blocked_behind(&pool, pid, 2, Duration::from_secs(15)) => blocked.unwrap(),
        }
        // No shortened lock_timeout on this normal, potentially NOTIFYing commit.
        holder.commit().await.unwrap();
        let (r1, r2) = tokio::time::timeout(Duration::from_secs(15), async {
            tokio::join!(first, second)
        })
        .await
        .unwrap();
        assert!(r1.status == 200 || r2.status == 200, "a revoke must win");
        assert!(
            [r1.status, r2.status]
                .into_iter()
                .all(|s| matches!(s, 200 | 401)),
            "ordered revokes must not conflict/deadlock: {}, {}",
            r1.status,
            r2.status
        );
    }
    s.stop().await;
}
