use chrono::{DateTime, Duration, Utc};
use porchlight_api::testing::{HouseholdFixture, TestClient, TestOptions};
use porchlight_db::testing::with_planning_hooks;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;
fn now() -> DateTime<Utc> {
    "2026-10-05T13:00:00Z".parse().unwrap()
}
fn options() -> TestOptions {
    TestOptions {
        background_derivation: false,
        db_pool_size: 2,
        item_clock: Arc::new(now),
        ..Default::default()
    }
}
async fn task(f: &HouseholdFixture, client: &TestClient) -> Value {
    let r = client
        .request(
            f.server.lan,
            f.admin.device.request(
                "POST",
                "/v1/items",
                json!({"title":"Fix2 task","due_date":"2026-10-12","est_minutes":30}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 201, "{}", r.json());
    r.json()
}
async fn proposal(
    pool: &PgPool,
    f: &HouseholdFixture,
    item: Uuid,
    start: DateTime<Utc>,
) -> (Uuid, i32) {
    let (occ, version): (Uuid, i32) =
        sqlx::query_as("select id,version from porchlight.item_occurrences where item_id=$1")
            .bind(item)
            .fetch_one(pool)
            .await
            .unwrap();
    let id = sqlx::query_scalar("insert into porchlight.schedule_suggestions(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id) values($1,$2,$3,$4,$5,$6,$7) returning id").bind(f.server.init.household_id).bind(f.admin.member_id).bind(item).bind(occ).bind(start).bind(start+Duration::minutes(30)).bind(Uuid::now_v7()).fetch_one(pool).await.unwrap();
    (id, version)
}
#[sqlx::test(migrations = "../db/migrations")]
async fn window_start_equal_now_gets_next_quarter_hour_and_can_accept(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let client = TestClient::new(&f.server.ca_pem, Some(&f.admin.device));
    let mut req = f.admin.device.request(
        "PUT",
        &format!("/v1/members/{}/availability", f.admin.member_id),
        json!({"windows":[{"weekday":0,"start":"09:00:00","end":"12:00:00"}]}),
        None,
    );
    req.headers_mut().insert("if-match", "0".parse().unwrap());
    assert_eq!(client.request(f.server.lan, req).await.unwrap().status, 200);
    let item = task(&f, &client).await;
    let id: Uuid = item["id"].as_str().unwrap().parse().unwrap();
    let before: Value =
        sqlx::query_scalar("select to_jsonb(i) from porchlight.items i where id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let r = client
        .request(
            f.server.lan,
            f.admin
                .device
                .request("POST", "/v1/schedule/suggest", json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200, "{}", r.json());
    let p = r.json()["suggestions"][0].clone();
    assert_eq!(p["proposed_start"], "2026-10-05T13:15:00Z");
    let mut req = f.admin.device.request(
        "POST",
        &format!("/v1/suggestions/{}/accept", p["id"].as_str().unwrap()),
        json!({}),
        None,
    );
    req.headers_mut().insert(
        "if-match",
        p["occurrence_version"].to_string().parse().unwrap(),
    );
    let r = client.request(f.server.lan, req).await.unwrap();
    assert_eq!(r.status, 200, "{}", r.json());
    let after: Value = sqlx::query_scalar("select to_jsonb(i) from porchlight.items i where id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, after, "accept must never edit item details");
    f.server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn accept_grace_is_inclusive_five_minutes_and_404s_deleted_or_hidden(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let client = TestClient::new(&f.server.ca_pem, Some(&f.admin.device));
    for (seconds, status) in [(-301, 422), (-300, 200), (-1, 200), (0, 200), (60, 200)] {
        let item = task(&f, &client).await;
        let item: Uuid = item["id"].as_str().unwrap().parse().unwrap();
        let (id, version) = proposal(&pool, &f, item, now() + Duration::seconds(seconds)).await;
        let mut req = f.admin.device.request(
            "POST",
            &format!("/v1/suggestions/{id}/accept"),
            json!({}),
            None,
        );
        req.headers_mut()
            .insert("if-match", version.to_string().parse().unwrap());
        let r = client.request(f.server.lan, req).await.unwrap();
        assert_eq!(r.status, status, "offset {seconds}: {}", r.json());
        if status == 422 {
            assert_eq!(r.json()["errors"][0]["code"], "not-future");
        }
    }
    for hidden in [false, true] {
        for action in ["accept", "dismiss"] {
            let item = task(&f, &client).await;
            let item: Uuid = item["id"].as_str().unwrap().parse().unwrap();
            let (id, version) = proposal(&pool, &f, item, now() + Duration::hours(1)).await;
            if hidden {
                sqlx::query(
                    "update porchlight.items set owner_id=$2,visibility='private' where id=$1",
                )
                .bind(item)
                .bind(f.users[0].member_id)
                .execute(&pool)
                .await
                .unwrap();
            } else {
                sqlx::query("update porchlight.items set deleted_at=$2 where id=$1")
                    .bind(item)
                    .bind(now())
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            let mut req = f.admin.device.request(
                "POST",
                &format!("/v1/suggestions/{id}/{action}"),
                json!({}),
                None,
            );
            req.headers_mut().insert(
                "if-match",
                if action == "accept" {
                    version.to_string()
                } else {
                    "1".into()
                }
                .parse()
                .unwrap(),
            );
            let r = client.request(f.server.lan, req).await.unwrap();
            assert_eq!(r.status, 404, "{action} hidden={hidden}: {}", r.json());
            let status: String = sqlx::query_scalar(
                "select status from porchlight.schedule_suggestions where id=$1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(status, "proposed");
        }
    }
    f.server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn nightly_notifies_inside_only_with_real_run_and_candidates_handle_pending_poison_and_past(
    pool: PgPool,
) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let client = TestClient::new(&f.server.ca_pem, Some(&f.admin.device));
    let mut req = f.admin.device.request(
        "PUT",
        &format!("/v1/members/{}/availability", f.admin.member_id),
        json!({"windows":[{"weekday":0,"start":"09:00:00","end":"12:00:00"}]}),
        None,
    );
    req.headers_mut().insert("if-match", "0".parse().unwrap());
    assert_eq!(client.request(f.server.lan, req).await.unwrap().status, 200);
    task(&f, &client).await;
    let dependent: Uuid = sqlx::query_scalar("insert into porchlight.household_members(household_id,role,display_name,color,tzid) values($1,'dependent','Fix2 dependent','#D3869B','America/New_York') returning id").bind(f.server.init.household_id).fetch_one(&pool).await.unwrap();
    let mut appointments = Vec::new();
    for (start, end) in [
        ("2026-10-12T10:00:00", "2026-10-12T11:00:00"),
        ("2026-10-12T14:00:00", "2026-10-12T15:00:00"),
    ] {
        let r = client.request(f.server.lan, f.admin.device.request("POST", "/v1/items", json!({"title":"Dependent","kind":"appointment","time_mode":"floating","subject_member_id":dependent,"local_start":start,"local_end":end}), None)).await.unwrap();
        assert_eq!(r.status, 201, "{}", r.json());
        appointments.push(r.json()["id"].as_str().unwrap().parse::<Uuid>().unwrap());
    }
    let (result, hooks) = with_planning_hooks(porchlight_api::planning::nightly(
        &f.server.state.pool,
        now(),
    ))
    .await;
    result.unwrap();
    let emitted: Vec<_> = hooks
        .suggestions
        .iter()
        .filter(|(_, _, item)| item.is_some())
        .collect();
    assert_eq!(
        emitted.len(),
        1,
        "only inside-availability candidates notify: {hooks:?}"
    );
    assert_eq!(emitted[0].0, f.admin.member_id);
    assert_eq!(emitted[0].2, Some(appointments[0]));
    let runs: Vec<Uuid> =
        sqlx::query_scalar("select run_id from porchlight.schedule_suggestions where member_id=$1")
            .bind(f.admin.member_id)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(
        runs.contains(&emitted[0].1),
        "dependent hook must use actual EDF run"
    );
    let events:Vec<(Option<Uuid>,Option<Uuid>,Value)>=sqlx::query_as("select item_id,occurrence_id,payload from porchlight.notification_events where kind='suggestion' and recipient_member_id=$1 and item_id is not null").bind(f.admin.member_id).fetch_all(&pool).await.unwrap();
    assert_eq!(
        events.len(),
        1,
        "nightly must emit real inside-only candidate event"
    );
    assert_eq!(events[0].0, Some(appointments[0]));
    assert!(events[0].1.is_some());
    assert_eq!(events[0].2["run_id"], json!(emitted[0].1));
    porchlight_api::planning::nightly(&f.server.state.pool, now())
        .await
        .unwrap();
    let count:i64=sqlx::query_scalar("select count(*) from porchlight.notification_events where kind='suggestion' and recipient_member_id=$1 and item_id is not null").bind(f.admin.member_id).fetch_one(&pool).await.unwrap();
    assert_eq!(
        count, 1,
        "same dependent occurrence dedupes across real nightly runs"
    );
    let peer = task(&f, &client).await;
    let peer: Uuid = peer["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("update porchlight.items set est_minutes=45 where id=$1")
        .bind(peer)
        .execute(&pool)
        .await
        .unwrap();
    let path = format!("/v1/items/{}/candidates", appointments[0]);
    let r = client
        .request(
            f.server.lan,
            f.admin.device.request("GET", &path, Value::Null, None),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["pending_derivation"], true);
    assert_eq!(r.json()["candidates"], json!([]));
    sqlx::query("update porchlight.item_derivation set attempts=5,last_error='invalid_time' where item_id=$1").bind(peer).execute(&pool).await.unwrap();
    let r = client
        .request(
            f.server.lan,
            f.admin.device.request("GET", &path, Value::Null, None),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(
        r.json()["pending_derivation"],
        false,
        "poison must not pin all candidates pending"
    );
    assert!(!r.json()["candidates"].as_array().unwrap().is_empty());
    let mut conn = f.server.state.pool.acquire().await.unwrap();
    let c = porchlight_api::extract::Caller::Member {
        member_id: f.admin.member_id,
        household_id: f.server.init.household_id,
        user_id: f.admin.user_id,
        role: "super_admin".into(),
        device_id: Uuid::nil(),
        session_id: Uuid::nil(),
        client_addr: None,
        request_id: Uuid::now_v7(),
    };
    let past = porchlight_api::planning::candidates(
        &mut conn,
        &c,
        appointments[0],
        "2026-10-12T14:00:01Z".parse().unwrap(),
    )
    .await
    .unwrap();
    assert!(
        past.occurrence_id.is_none(),
        "past appointment must not rank candidates"
    );
    sqlx::query("update porchlight.item_derivation set requested_gen=requested_gen+1,attempts=5,last_error='invalid_time' where item_id=$1").bind(appointments[0]).execute(&pool).await.unwrap();
    let poisoned = porchlight_api::planning::candidates(&mut conn, &c, appointments[0], now())
        .await
        .unwrap();
    assert!(
        !poisoned.pending_derivation,
        "poisoned parent must not stay pending forever"
    );
    assert!(
        poisoned.candidates.is_empty(),
        "never rank a poisoned parent projection"
    );
    drop(conn);
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn queue_nowait_error_does_not_abort_later_members(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let mut members = [f.admin.member_id, f.users[0].member_id];
    members.sort();
    for member in members {
        sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,0,'09:00','12:00')").bind(f.server.init.household_id).bind(member).execute(&pool).await.unwrap();
    }
    sqlx::query("update porchlight.member_load_jobs set retry_at=$1")
        .bind(now())
        .execute(&pool)
        .await
        .unwrap();
    let mut held = pool.begin().await.unwrap();
    sqlx::query("select member_id from porchlight.member_load_jobs where member_id=$1 for update")
        .bind(members[0])
        .execute(&mut *held)
        .await
        .unwrap();
    assert_eq!(
        porchlight_api::planning::run_load_batch(&f.server.state.pool, now())
            .await
            .unwrap(),
        1
    );
    let pending: Vec<bool> = sqlx::query_scalar("select requested_gen>applied_gen from porchlight.member_load_jobs where member_id=any($1) order by member_id").bind(&members[..]).fetch_all(&pool).await.unwrap();
    assert_eq!(pending, vec![true, false]);
    held.rollback().await.unwrap();
    assert_eq!(
        porchlight_api::planning::run_load_batch(&f.server.state.pool, now())
            .await
            .unwrap(),
        1
    );
    f.server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn suggestions_page_scans_invisible_rows_without_gaps_or_duplicates(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let client = TestClient::new(&f.server.ca_pem, Some(&f.admin.device));
    let mut expected = Vec::new();
    for hidden in [true, true, true, false, false, true, false, false] {
        let made = task(&f, &client).await;
        let item = made["id"].as_str().unwrap().parse().unwrap();
        let (id, _) = proposal(&pool, &f, item, now() + Duration::hours(1)).await;
        if hidden {
            sqlx::query("update porchlight.items set owner_id=$2,visibility='private' where id=$1")
                .bind(item)
                .bind(f.users[0].member_id)
                .execute(&pool)
                .await
                .unwrap();
        } else {
            expected.push(id);
        }
    }
    let mut collected = Vec::new();
    let mut path = "/v1/suggestions?limit=2".to_string();
    for _ in 0..10 {
        let r = client
            .request(
                f.server.lan,
                f.admin.device.request("GET", &path, Value::Null, None),
            )
            .await
            .unwrap();
        assert_eq!(r.status, 200, "{}", r.json());
        let body = r.json();
        assert!(body["items"].as_array().unwrap().len() <= 2);
        collected.extend(
            body["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["id"].as_str().unwrap().parse::<Uuid>().unwrap()),
        );
        match body["next_cursor"].as_str() {
            Some(cursor) => path = format!("/v1/suggestions?limit=2&cursor={cursor}"),
            None => break,
        }
    }
    assert_eq!(collected, expected);
    f.server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn derive_worker_respects_deleted_peer_member_lock(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let client = TestClient::new(&f.server.ca_pem, Some(&f.admin.device));
    let made = task(&f, &client).await;
    let item: Uuid = made["id"].as_str().unwrap().parse().unwrap();
    sqlx::query(
        "insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)",
    )
    .bind(f.server.init.household_id)
    .bind(item)
    .bind(f.users[0].member_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("update porchlight.item_derivation set retry_at=$1 where item_id=$2")
        .bind(now())
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        porchlight_api::derive_worker::run_batch(&f.server.state.pool, now())
            .await
            .unwrap(),
        1
    );
    sqlx::query("update porchlight.household_members set deleted_at=$2 where id=$1")
        .bind(f.users[0].member_id)
        .bind(now())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update porchlight.items set est_minutes=45 where id=$1")
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update porchlight.item_derivation set retry_at=$1 where item_id=$2")
        .bind(now())
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    let mut held = pool.begin().await.unwrap();
    sqlx::query("select id from porchlight.household_members where id=$1 for no key update")
        .bind(f.users[0].member_id)
        .execute(&mut *held)
        .await
        .unwrap();
    assert_eq!(
        porchlight_api::derive_worker::run_batch(&f.server.state.pool, now())
            .await
            .unwrap(),
        0
    );
    held.rollback().await.unwrap();
    assert_eq!(
        porchlight_api::derive_worker::run_batch(&f.server.state.pool, now())
            .await
            .unwrap(),
        1
    );
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn visible_unplaced_task_is_reported_without_loading_all_occurrences(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let client = TestClient::new(&f.server.ca_pem, Some(&f.admin.device));
    let made = task(&f, &client).await;
    let item: Uuid = made["id"].as_str().unwrap().parse().unwrap();
    let occ: Uuid =
        sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1")
            .bind(item)
            .fetch_one(&pool)
            .await
            .unwrap();
    // No windows: the eligible task is unplaced, but must still be View-filtered independently of placed rows.
    let r = client
        .request(
            f.server.lan,
            f.admin
                .device
                .request("POST", "/v1/schedule/suggest", json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200, "{}", r.json());
    assert_eq!(r.json()["suggestions"], json!([]));
    assert_eq!(
        r.json()["unplaced"],
        json!([{"occurrence_id":occ,"reason":"no_slot_before_deadline"}])
    );
    f.server.stop().await;
}
