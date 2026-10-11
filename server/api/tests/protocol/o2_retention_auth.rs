use porchlight_api::testing::{HouseholdFixture, TestOptions};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn app_maintenance_counts_expired_auth_and_session_step_up_children(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let now: chrono::DateTime<chrono::Utc> = "2200-01-01T03:45:00Z".parse().unwrap();
    let h = f.server.init.household_id;
    let m = f.admin.member_id;
    let user: Uuid =
        sqlx::query_scalar("select user_id from porchlight.household_members where id=$1")
            .bind(m)
            .fetch_one(&pool)
            .await
            .unwrap();
    let session: Uuid = sqlx::query_scalar("select id from porchlight.sessions where device_id=$1")
        .bind(f.admin.device.device_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    // Unexpired step-up still has to be explicitly counted if its parent expires.
    sqlx::query("insert into porchlight.step_ups(household_id,session_id,device_id,action,token_hash,expires_at) values($1,$2,$3,'household.export',decode(repeat('ab',32),'hex'),$4)").bind(h).bind(session).bind(f.admin.device.device_id).bind(now+chrono::Duration::seconds(1)).execute(&pool).await.unwrap();
    for (n, expires) in [(1_u8, now), (2, now + chrono::Duration::seconds(1))] {
        let hash = vec![n; 32];
        sqlx::query("insert into porchlight.enrollment_tokens(household_id,member_id,device_label,token_hash,expires_at,created_by) values($1,$2,'retention fixture',$3,$4,$2)").bind(h).bind(m).bind(&hash).bind(expires).execute(&pool).await.unwrap();
        sqlx::query("insert into porchlight.password_resets(household_id,user_id,code_hash,expires_at,created_by) values($1,$2,$3,$4,$5)").bind(h).bind(user).bind(&hash).bind(expires).bind(m).execute(&pool).await.unwrap();
        sqlx::query("insert into porchlight.api_tokens(household_id,owner_member_id,name,token_hash,scopes,expires_at) values($1,$2,'retention fixture',$3,array['read'],$4)").bind(h).bind(m).bind(&hash).bind(expires).execute(&pool).await.unwrap();
    }
    let mut expected = std::collections::BTreeMap::new();
    for table in [
        "sessions",
        "enrollment_tokens",
        "password_resets",
        "api_tokens",
    ] {
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "select count(*) from porchlight.{table} where household_id=$1 and expires_at<=$2"
        )))
        .bind(h)
        .bind(now)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(count > 0);
        expected.insert(table, count);
    }
    let mut tx =
        porchlight_db::tx::begin_as(&f.server.state.pool, &porchlight_db::tx::Actor::system())
            .await
            .unwrap();
    let counts: Value = sqlx::query_scalar("select porchlight.pl_retention($1)")
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for (table, count) in expected {
        assert_eq!(counts[table], count);
        let remaining: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "select count(*) from porchlight.{table} where household_id=$1 and expires_at<=$2"
        )))
        .bind(h)
        .bind(now)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining, 0);
    }
    assert_eq!(counts["step_ups"], 1);
    for table in ["enrollment_tokens", "password_resets", "api_tokens"] {
        let remaining: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "select count(*) from porchlight.{table} where household_id=$1 and expires_at=$2"
        )))
        .bind(h)
        .bind(now + chrono::Duration::seconds(1))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remaining, 1);
    }
    let audit: Value = sqlx::query_scalar(
        "select counts from porchlight.purge_log where household_id=$1 and cardinality(item_ids)=0",
    )
    .bind(h)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit, counts);
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn retention_waiting_on_notifications_does_not_hold_publication_mutex(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let now: chrono::DateTime<chrono::Utc> = "2200-01-01T03:45:00Z".parse().unwrap();
    let event: Uuid=sqlx::query_scalar("insert into porchlight.notification_events(household_id,recipient_member_id,kind,entity_type,entity_id,created_at) values($1,$2,'assigned','member',$2,$3) returning id").bind(f.server.init.household_id).bind(f.admin.member_id).bind(now-chrono::Duration::days(91)).fetch_one(&pool).await.unwrap();
    let mut holder = pool.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    sqlx::query("select id from porchlight.notification_events where id=$1 for update")
        .bind(event)
        .execute(&mut *holder)
        .await
        .unwrap();
    let app = f.server.state.pool.clone();
    let task = tokio::spawn(async move {
        let mut tx = porchlight_db::tx::begin_as(&app, &porchlight_db::tx::Actor::system())
            .await
            .unwrap();
        sqlx::query("select porchlight.pl_retention($1)")
            .bind(now)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    });
    porchlight_api::testing::wait_blocked_behind(&pool, pid, 1, std::time::Duration::from_secs(3))
        .await
        .unwrap();
    // The outbox assigner takes the same mutex briefly; retry NOWAIT until a deadline. A retention backend holding it
    // while blocked would keep it for the whole wait, so every attempt would fail.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let available = loop {
        let mut observer = pool.begin().await.unwrap();
        let ok = sqlx::query("select id from porchlight.stream_assign_mutex for update nowait")
            .execute(&mut *observer)
            .await
            .is_ok();
        observer.rollback().await.unwrap();
        if ok || std::time::Instant::now() >= deadline {
            break ok;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    holder.rollback().await.unwrap();
    task.await.unwrap();
    assert!(
        available,
        "retention blocked publication while waiting on notification history"
    );
}
