use porchlight_api::testing::{HouseholdFixture, TestClient, TestOptions};
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
#[sqlx::test(migrations = "../db/migrations")]
async fn assigned_not_owned_self_schedule_and_admin_run_boundary(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let s = &f.server;
    let admin = TestClient::new(&s.ca_pem, Some(&f.admin.device));
    let user = &f.users[0];
    let client = TestClient::new(&s.ca_pem, Some(&user.device));
    let mut req = user.device.request(
        "PUT",
        &format!("/v1/members/{}/availability", user.member_id),
        json!({"windows":[{"weekday":0,"start":"13:00:00","end":"17:00:00"}]}),
        None,
    );
    req.headers_mut().insert("if-match", "0".parse().unwrap());
    assert_eq!(client.request(s.lan, req).await.unwrap().status, 200);
    let made=admin.request(s.lan,f.admin.device.request("POST","/v1/items",json!({"title":"Assigned deadline","kind":"deadline","time_mode":"fixed","tzid":"America/New_York","due_at":"2026-10-12T21:00:00Z","est_minutes":30,"assignees":[user.member_id]}),None)).await.unwrap();
    assert_eq!(made.status, 201, "{}", made.json());
    let item: Uuid = made.json()["id"].as_str().unwrap().parse().unwrap();
    let r = client
        .request(
            s.lan,
            user.device
                .request("POST", "/v1/schedule/suggest", json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200, "{}", r.json());
    assert_eq!(r.json()["suggestions"].as_array().unwrap().len(), 1);
    let proposal = r.json()["suggestions"][0].clone();
    let denied = admin
        .request(
            s.lan,
            f.admin.device.request(
                "POST",
                "/v1/schedule/suggest",
                json!({"member_id":user.member_id}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(denied.status, 403);
    let mut req = user.device.request(
        "POST",
        &format!(
            "/v1/suggestions/{}/accept",
            proposal["id"].as_str().unwrap()
        ),
        json!({}),
        None,
    );
    req.headers_mut().insert(
        "if-match",
        proposal["occurrence_version"].to_string().parse().unwrap(),
    );
    let accepted = client.request(s.lan, req).await.unwrap();
    assert_eq!(accepted.status, 200, "{}", accepted.json());
    let span: (
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as(
        "select starts_at,ends_at from porchlight.member_agenda where item_id=$1 and member_id=$2",
    )
    .bind(item)
    .bind(user.member_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        span.0.unwrap(),
        proposal["proposed_start"]
            .as_str()
            .unwrap()
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap()
    );
    assert_eq!(
        span.1.unwrap(),
        proposal["proposed_end"]
            .as_str()
            .unwrap()
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap()
    );
    f.server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn empty_availability_is_untracked_and_missing_weekday_is_zero(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let s = &f.server;
    let client = TestClient::new(&s.ca_pem, Some(&f.admin.device));
    let r = client
        .request(
            s.lan,
            f.admin.device.request(
                "POST",
                "/v1/items",
                json!({"title":"Untracked","due_date":"2026-10-12","est_minutes":30}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 201);
    assert_eq!(r.json()["conflicts"], json!([]));
    let mut req = f.admin.device.request(
        "PUT",
        &format!("/v1/members/{}/availability", f.admin.member_id),
        json!({"windows":[{"weekday":1,"start":"09:00:00","end":"10:00:00"}]}),
        None,
    );
    req.headers_mut().insert("if-match", "0".parse().unwrap());
    let r = client.request(s.lan, req).await.unwrap();
    assert_eq!(r.status, 200);
    assert!(
        r.json()["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["kind"] == "load")
    );
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn accept_rejects_past_nonmember_and_terminal_proposals_with_field_codes(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let s = &f.server;
    let client = TestClient::new(&s.ca_pem, Some(&f.admin.device));
    for code in ["not-future", "not-agenda-member", "not-proposed"] {
        let made = client
            .request(
                s.lan,
                f.admin.device.request(
                    "POST",
                    "/v1/items",
                    json!({"title":"Reject","due_date":"2026-10-12","est_minutes":30}),
                    None,
                ),
            )
            .await
            .unwrap();
        assert_eq!(made.status, 201);
        let item: Uuid = made.json()["id"].as_str().unwrap().parse().unwrap();
        let occ: (Uuid, i32) =
            sqlx::query_as("select id,version from porchlight.item_occurrences where item_id=$1")
                .bind(item)
                .fetch_one(&pool)
                .await
                .unwrap();
        let start = if code == "not-future" {
            "2026-10-05T11:54:59Z"
        } else {
            "2026-10-12T13:00:00Z"
        };
        let id:Uuid=sqlx::query_scalar("insert into porchlight.schedule_suggestions(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id) values($1,$2,$3,$4,$5,'2026-10-12T14:00Z',$6) returning id").bind(s.init.household_id).bind(f.admin.member_id).bind(item).bind(occ.0).bind(start.parse::<chrono::DateTime<chrono::Utc>>().unwrap()).bind(Uuid::now_v7()).fetch_one(&pool).await.unwrap();
        if code == "not-agenda-member" {
            sqlx::query("insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)").bind(s.init.household_id).bind(item).bind(f.users[0].member_id).execute(&pool).await.unwrap();
        }
        if code == "not-proposed" {
            sqlx::query(
                "update porchlight.schedule_suggestions set status='dismissed' where id=$1",
            )
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        }
        let mut req = f.admin.device.request(
            "POST",
            &format!("/v1/suggestions/{id}/accept"),
            json!({}),
            None,
        );
        req.headers_mut()
            .insert("if-match", occ.1.to_string().parse().unwrap());
        let r = client.request(s.lan, req).await.unwrap();
        assert_eq!(r.status, 422, "{}", r.json());
        assert_eq!(r.json()["errors"][0]["code"], code);
        let detached: bool =
            sqlx::query_scalar("select is_detached from porchlight.item_occurrences where id=$1")
                .bind(occ.0)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(!detached);
    }
    f.server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn load_job_backoff_poison_deleted_ack_and_nightly_error(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let s = &f.server;
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,0,'09:00','10:00')").bind(s.init.household_id).bind(f.users[0].member_id).execute(&pool).await.unwrap();
    let now: chrono::DateTime<chrono::Utc> = "2026-10-05T12:00:00Z".parse().unwrap();
    sqlx::query("update porchlight.member_load_jobs set retry_at=$1")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("revoke select on porchlight.member_agenda from porchlight_app")
        .execute(&pool)
        .await
        .unwrap();
    for n in 1..=5 {
        assert_eq!(
            porchlight_api::planning::run_load_batch(&s.state.pool, now)
                .await
                .unwrap(),
            0
        );
        let state:(i32,chrono::DateTime<chrono::Utc>,Option<String>)=sqlx::query_as("select attempts,retry_at,last_error from porchlight.member_load_jobs where member_id=$1").bind(f.users[0].member_id).fetch_one(&pool).await.unwrap();
        assert_eq!(state.0, n);
        assert!(state.1 > now);
        assert_eq!(state.2.as_deref(), Some("database"));
        assert_eq!(
            porchlight_api::planning::run_load_batch(&s.state.pool, now)
                .await
                .unwrap(),
            0
        );
        sqlx::query("update porchlight.member_load_jobs set retry_at=$1")
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
    }
    // Even with retry_at due, a sixth pass must never retry poison.
    assert_eq!(
        porchlight_api::planning::run_load_batch(&s.state.pool, now)
            .await
            .unwrap(),
        0
    );
    let attempts: i32 =
        sqlx::query_scalar("select attempts from porchlight.member_load_jobs where member_id=$1")
            .bind(f.users[0].member_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(attempts, 5);
    assert!(
        porchlight_api::planning::nightly(&s.state.pool, now)
            .await
            .is_err()
    );
    sqlx::query("grant select on porchlight.member_agenda to porchlight_app")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update porchlight.household_members set deleted_at=now() where id=$1")
        .bind(f.users[0].member_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update porchlight.member_load_jobs set attempts=0,retry_at=$1")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        porchlight_api::planning::run_load_batch(&s.state.pool, now)
            .await
            .unwrap(),
        1
    );
    let pending: bool = sqlx::query_scalar(
        "select requested_gen>applied_gen from porchlight.member_load_jobs where member_id=$1",
    )
    .bind(f.users[0].member_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!pending);
    sqlx::query("insert into porchlight.member_load_signals(member_id) values($1)")
        .bind(Uuid::now_v7())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        porchlight_api::planning::run_load_batch(&s.state.pool, now)
            .await
            .unwrap(),
        0
    );
    let signals: i64 = sqlx::query_scalar("select count(*) from porchlight.member_load_signals")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(signals, 0);
    f.server.stop().await;
}
