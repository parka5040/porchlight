//! Raw-agent convergence and acquisition-time ACL loss with injected clocks.
use porchlight_api::testing::{HouseholdFixture, TestClient, TestOptions, wait_blocked_behind};
use porchlight_db::{
    testing::{Role, pool_as},
    tx::{Actor, begin_as},
};
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;
fn options() -> TestOptions {
    TestOptions {
        background_derivation: false,
        db_pool_size: 1,
        item_clock: Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
        ..Default::default()
    }
}
async fn drain(pool: &PgPool, app: &PgPool, now: chrono::DateTime<chrono::Utc>) -> usize {
    // SQL enqueue timestamps use the database clock. Align only the operational
    // retry timestamp to the injected test clock; never rely on the laptop date.
    sqlx::query(
        "update porchlight.item_derivation set retry_at=$1 where requested_gen>applied_gen",
    )
    .bind(now)
    .execute(pool)
    .await
    .unwrap();
    porchlight_api::derive_worker::run_batch(app, now)
        .await
        .unwrap()
}
#[sqlx::test(migrations = "../db/migrations")]
async fn raw_estimate_assignment_completion_and_delete_converge_via_real_worker(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let s = &f.server;
    let client = TestClient::new(&s.ca_pem, Some(&f.admin.device));
    for member in [f.admin.member_id, f.users[0].member_id] {
        sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,0,'09:00','09:15')").bind(s.init.household_id).bind(member).execute(&pool).await.unwrap();
    }
    let made = client
        .request(
            s.lan,
            f.admin.device.request(
                "POST",
                "/v1/items",
                json!({"title":"Raw task","due_date":"2026-10-12","est_minutes":30}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(made.status, 201);
    assert!(
        made.json()["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["kind"] == "load"
                && c["other_item_id"].is_null()
                && c["other_occurrence_id"].is_null())
    );
    let item = made.json()["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let occ: Uuid =
        sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1")
            .bind(item)
            .fetch_one(&pool)
            .await
            .unwrap();
    let agent = pool_as(&pool, Role::Agent, Some("agent:planner"))
        .await
        .unwrap();
    let now = "2026-10-05T12:00:00Z".parse().unwrap();
    let mut raw = begin_as(&agent, &Actor::system()).await.unwrap();
    sqlx::query("update porchlight.items set est_minutes=90 where id=$1")
        .bind(item)
        .execute(&mut *raw)
        .await
        .unwrap();
    sqlx::query(
        "insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)",
    )
    .bind(s.init.household_id)
    .bind(item)
    .bind(f.users[0].member_id)
    .execute(&mut *raw)
    .await
    .unwrap();
    raw.commit().await.unwrap();
    assert_eq!(drain(&pool, &s.state.pool, now).await, 1);
    let loads: Vec<(Uuid, i64)> =
        sqlx::query_as("select member_id,planned_minutes from porchlight.member_daily_load")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(loads, vec![(f.users[0].member_id, 90)]); // Old owner projection is removed.
    let mut raw = begin_as(&agent, &Actor::system()).await.unwrap();
    sqlx::query("update porchlight.item_occurrences set status='done',completed_at=$2,completed_by=$3 where id=$1")
        .bind(occ)
        .bind(now)
        .bind(f.users[0].member_id)
        .execute(&mut *raw)
        .await
        .unwrap();
    raw.commit().await.unwrap();
    assert_eq!(drain(&pool, &s.state.pool, now).await, 1);
    let count: i64 = sqlx::query_scalar("select count(*) from porchlight.member_daily_load")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let mut raw = begin_as(&agent, &Actor::system()).await.unwrap();
    sqlx::query("update porchlight.item_occurrences set status='todo',completed_at=null,completed_by=null where id=$1")
        .bind(occ)
        .execute(&mut *raw)
        .await
        .unwrap();
    raw.commit().await.unwrap();
    assert_eq!(drain(&pool, &s.state.pool, now).await, 1);
    let restored: Vec<(Uuid, i64)> =
        sqlx::query_as("select member_id,planned_minutes from porchlight.member_daily_load")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(restored, vec![(f.users[0].member_id, 90)]);
    let mut raw = begin_as(&agent, &Actor::system()).await.unwrap();
    sqlx::query("update porchlight.items set deleted_at=now() where id=$1")
        .bind(item)
        .execute(&mut *raw)
        .await
        .unwrap();
    raw.commit().await.unwrap();
    assert_eq!(drain(&pool, &s.state.pool, now).await, 1);
    let count: i64 = sqlx::query_scalar("select count(*) from porchlight.member_daily_load")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    f.server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn accept_rolls_back_all_effects_when_visibility_changes_during_scope_acquisition(
    pool: PgPool,
) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let s = &f.server;
    let owner_client = TestClient::new(&s.ca_pem, Some(&f.users[0].device));
    let made = owner_client
        .request(
            s.lan,
            f.users[0].device.request(
                "POST",
                "/v1/items",
                json!({"title":"ACL race","due_date":"2026-10-12","est_minutes":30}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(made.status, 201);
    let item = made.json()["id"].as_str().unwrap().parse::<Uuid>().unwrap();
    let occ: (Uuid, i32) =
        sqlx::query_as("select id,version from porchlight.item_occurrences where item_id=$1")
            .bind(item)
            .fetch_one(&pool)
            .await
            .unwrap();
    let suggestion:Uuid=sqlx::query_scalar("insert into porchlight.schedule_suggestions(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id) values($1,$2,$3,$4,'2026-10-12T13:00:00Z','2026-10-12T13:30:00Z',$5) returning id").bind(s.init.household_id).bind(f.users[0].member_id).bind(item).bind(occ.0).bind(Uuid::now_v7()).fetch_one(&pool).await.unwrap();
    let agent = pool_as(&pool, Role::Agent, Some("agent:planner"))
        .await
        .unwrap();
    let mut raw = begin_as(&agent, &Actor::system()).await.unwrap();
    sqlx::query("update porchlight.items set visibility='private' where id=$1")
        .bind(item)
        .execute(&mut *raw)
        .await
        .unwrap();
    let mut req = f.admin.device.request(
        "POST",
        &format!("/v1/suggestions/{suggestion}/accept"),
        json!({}),
        Some("acl-race"),
    );
    req.headers_mut()
        .insert("if-match", occ.1.to_string().parse().unwrap());
    let holder: i32 = sqlx::query_scalar("select pg_backend_pid()")
        .fetch_one(&mut *raw)
        .await
        .unwrap();
    let admin_client = TestClient::new(&s.ca_pem, Some(&f.admin.device))
        .with_timeout(std::time::Duration::from_secs(60));
    let addr = s.lan;
    let request = tokio::spawn(async move { admin_client.request(addr, req).await.unwrap() });
    wait_blocked_behind(&pool, holder, 1, std::time::Duration::from_secs(30))
        .await
        .expect("accept must wait for the held item, after initial authorization");
    raw.commit().await.unwrap();
    let result = request.await.unwrap();
    assert_eq!(result.status, 404, "{}", result.json());
    let state:(bool,Option<chrono::NaiveDateTime>,i32)=sqlx::query_as("select is_detached,override_local_start,version from porchlight.item_occurrences where id=$1").bind(occ.0).fetch_one(&pool).await.unwrap();
    assert_eq!(state, (false, None, occ.1));
    let suggestion_state: (String, i32) =
        sqlx::query_as("select status,version from porchlight.schedule_suggestions where id=$1")
            .bind(suggestion)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(suggestion_state, ("proposed".into(), 1));
    let audit:i64=sqlx::query_scalar("select count(*) from porchlight.planning_audit where entity_type='suggestion' and entity_id=$1").bind(suggestion).fetch_one(&pool).await.unwrap();
    assert_eq!(audit, 1); // Only the proposal creation survives.
    f.server.stop().await;
}
