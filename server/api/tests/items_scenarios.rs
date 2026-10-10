use porchlight_api::testing::{
    HouseholdFixture, TestClient, TestOptions, TestResponse, agent_request, trust,
    wait_blocked_behind,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

/// Liveness bounds for requests queued behind a test-held item lock: the overlap wait, and the
/// response deadline of the queued requests (above it, so a slow peer cannot time out the first).
const OVERLAP_WITHIN: std::time::Duration = std::time::Duration::from_secs(30);
const QUEUED_RESPONSE_WITHIN: std::time::Duration = std::time::Duration::from_secs(60);
fn options() -> TestOptions {
    TestOptions {
        background_derivation: false,
        item_clock: Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
        ..Default::default()
    }
}
async fn write(
    f: &HouseholdFixture,
    method: &str,
    path: &str,
    version: &Value,
    body: Value,
) -> TestResponse {
    let mut req = trust::req(method, path, body, &Uuid::now_v7().to_string());
    req.headers_mut()
        .insert("if-match", version.to_string().parse().unwrap());
    agent_request(
        &f.server.agent_socket,
        Some(f.server.bootstrap_token()),
        req,
    )
    .await
}
async fn occs(f: &HouseholdFixture, id: &Value) -> Value {
    let r = trust::admin(
        &f.server,
        "GET",
        &format!("/v1/items/{}/occurrences", id.as_str().unwrap()),
        json!({}),
    )
    .await;
    assert_eq!(r.status, 200);
    r.json()["items"].clone()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn recurring_scopes_retain_identity_and_fixed_gap_tail_metadata(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let r=trust::admin(&f.server,"POST","/v1/items",json!({"title":"series","kind":"appointment","tzid":"America/New_York","starts_at":"2027-03-13T07:10:00Z","ends_at":"2027-03-13T07:20:00Z","rrule":"FREQ=DAILY;COUNT=4","recurrence_mode":"calendar"})).await;
    assert_eq!(r.status, 201, "{r:?}");
    let item = r.json();
    let original = occs(&f, &item["id"]).await;
    assert_eq!(original.as_array().unwrap().len(), 4);
    let boundary = &original[1];
    let edited=write(&f,"PATCH",&format!("/v1/occurrences/{}",original[2]["id"].as_str().unwrap()),&original[2]["version"],json!({"override_local_start":"2027-03-15T04:00:00","override_local_end":"2027-03-15T04:30:00"})).await;
    assert_eq!(edited.status, 200, "{edited:?}");
    assert_eq!(edited.json()["is_detached"], true);
    let split = write(
        &f,
        "PATCH",
        &format!(
            "/v1/items/{}?scope=this_and_future&occurrence_id={}",
            item["id"].as_str().unwrap(),
            boundary["id"].as_str().unwrap()
        ),
        &item["version"],
        json!({"title":"tail"}),
    )
    .await;
    assert_eq!(split.status, 200, "{split:?}");
    let tail = split.json();
    assert_ne!(tail["id"], item["id"]);
    assert_eq!(tail["nominal_local_anchor"], "2027-03-14T02:10:00");
    assert_eq!(tail["nominal_wall_duration"], 600_000_000i64);
    let future = occs(&f, &tail["id"]).await;
    assert_eq!(future.as_array().unwrap().len(), 3);
    assert_eq!(future[0]["id"], boundary["id"]);
    assert_eq!(future[1]["id"], original[2]["id"]);
    assert_eq!(future[1]["is_detached"], true);
    assert_eq!(future[2]["starts_at"], "2027-03-16T06:10:00Z");
    let head = occs(&f, &item["id"]).await;
    assert_eq!(head.as_array().unwrap().len(), 1);
    let all = write(
        &f,
        "PATCH",
        &format!("/v1/items/{}", tail["id"].as_str().unwrap()),
        &tail["version"],
        json!({"scope":"all","notes":"non-timing"}),
    )
    .await;
    assert_eq!(all.status, 200, "{all:?}");
    assert_eq!(
        all.json()["nominal_local_anchor"],
        tail["nominal_local_anchor"]
    );
    let timing = write(
        &f,
        "PATCH",
        &format!("/v1/items/{}", tail["id"].as_str().unwrap()),
        &all.json()["version"],
        json!({"scope":"all","starts_at":"2027-03-14T08:00:00Z","ends_at":"2027-03-14T08:10:00Z"}),
    )
    .await;
    assert_eq!(timing.status, 200, "{timing:?}");
    assert!(timing.json()["nominal_local_anchor"].is_null());
    assert!(timing.json()["nominal_wall_duration"].is_null());
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn concurrent_completion_has_one_successor_replays_and_reopen_removes_untouched_successor(
    pool: PgPool,
) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let made=trust::admin(&f.server,"POST","/v1/items",json!({"title":"completion","kind":"deadline","tzid":"UTC","due_at":"2026-10-05T12:00:00Z","recurrence_mode":"completion","completion_interval":{"days":7,"months":0,"microseconds":0},"assignees":[f.users[0].member_id,f.users[1].member_id]})).await;
    assert_eq!(made.status, 201, "{made:?}");
    let item = made.json();
    let occurrences = occs(&f, &item["id"]).await;
    let old = &occurrences[0];
    let path = format!("/v1/occurrences/{}/complete", old["id"].as_str().unwrap());
    let mut a = f.users[0]
        .device
        .request("POST", &path, json!({}), Some("complete-a"));
    a.headers_mut()
        .insert("if-match", old["version"].to_string().parse().unwrap());
    let mut b = f.users[1]
        .device
        .request("POST", &path, json!({}), Some("complete-b"));
    b.headers_mut()
        .insert("if-match", old["version"].to_string().parse().unwrap());
    let ca = TestClient::new(&f.server.ca_pem, Some(&f.users[0].device))
        .with_timeout(QUEUED_RESPONSE_WITHIN);
    let cb = TestClient::new(&f.server.ca_pem, Some(&f.users[1].device))
        .with_timeout(QUEUED_RESPONSE_WITHIN);
    let mut hold = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM porchlight.items WHERE id=$1 FOR UPDATE")
        .bind(item["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .fetch_one(&mut *hold)
        .await
        .unwrap();
    let holder: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *hold)
        .await
        .unwrap();
    let barrier = tokio::sync::Barrier::new(3);
    let ra = async {
        barrier.wait().await;
        ca.request(f.server.lan, a).await
    };
    let rb = async {
        barrier.wait().await;
        cb.request(f.server.lan, b).await
    };
    let release = async {
        barrier.wait().await;
        wait_blocked_behind(&pool, holder, 2, OVERLAP_WITHIN)
            .await
            .expect("both HTTP completions must overlap behind the held item");
        hold.commit().await.unwrap();
    };
    let (a, b, ()) = tokio::join!(ra, rb, release);
    let a = a.unwrap();
    let b = b.unwrap();
    assert!(
        (a.status == 200 && b.status == 409) || (a.status == 409 && b.status == 200),
        "a={a:?} b={b:?}"
    );
    let loser = if a.status == 409 { &a } else { &b };
    assert!(
        loser.json()["type"]
            .as_str()
            .unwrap()
            .ends_with("version-conflict"),
        "{loser:?}"
    );
    let winner = if a.status == 200 { 0 } else { 1 };
    let winning = if winner == 0 { &a } else { &b };
    let mut req = f.users[winner].device.request(
        "POST",
        &path,
        json!({}),
        Some(if winner == 0 {
            "complete-a"
        } else {
            "complete-b"
        }),
    );
    req.headers_mut()
        .insert("if-match", old["version"].to_string().parse().unwrap());
    let replay = TestClient::new(&f.server.ca_pem, Some(&f.users[winner].device))
        .request(f.server.lan, req)
        .await
        .unwrap();
    assert_eq!(replay.body, winning.body);
    let after = occs(&f, &item["id"]).await;
    assert_eq!(after.as_array().unwrap().len(), 2);
    assert_eq!(after[1]["due_at"], "2026-10-12T12:00:00Z");
    let reopened = write(
        &f,
        "POST",
        &format!("/v1/occurrences/{}/reopen", old["id"].as_str().unwrap()),
        &after[0]["version"],
        json!({}),
    )
    .await;
    assert_eq!(reopened.status, 200, "{reopened:?}");
    let final_rows = occs(&f, &item["id"]).await;
    assert_eq!(final_rows.as_array().unwrap().len(), 1);
    assert_eq!(final_rows[0]["id"], old["id"]);
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn conflict_masks_title_and_id_but_agents_see_both(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let timing = json!({"title":"private surprise","owner_id":f.users[1].member_id,"subject_member_id":f.users[0].member_id,"visibility":"private","kind":"appointment","tzid":"UTC","starts_at":"2026-10-06T10:00:00Z","ends_at":"2026-10-06T11:00:00Z"});
    let hidden = trust::admin(&f.server, "POST", "/v1/items", timing.clone()).await;
    assert_eq!(hidden.status, 201, "{hidden:?}");
    let mut body = timing;
    body.as_object_mut().unwrap().remove("owner_id");
    body.as_object_mut().unwrap().remove("subject_member_id");
    body["title"] = json!("my appointment");
    let d = &f.users[0].device;
    let client = TestClient::new(&f.server.ca_pem, Some(d));
    let visible = client
        .request(f.server.lan, d.request("POST", "/v1/items", body, None))
        .await
        .unwrap();
    assert_eq!(visible.status, 201, "{visible:?}");
    let conflicts = visible.json()["conflicts"].as_array().unwrap().clone();
    assert!(!conflicts.is_empty());
    assert!(conflicts.iter().any(|c| c["other_title"] == "Busy"
        && c["other_item_id"].is_null()
        && c["other_occurrence_id"].is_null()));
    assert!(!String::from_utf8_lossy(&visible.body).contains("private surprise"));
    assert!(
        !String::from_utf8_lossy(&visible.body).contains(hidden.json()["id"].as_str().unwrap())
    );
    let edited = write(
        &f,
        "PATCH",
        &format!("/v1/items/{}", visible.json()["id"].as_str().unwrap()),
        &visible.json()["version"],
        json!({"notes":"agent"}),
    )
    .await;
    assert_eq!(edited.status, 200, "{edited:?}");
    assert!(
        edited.json()["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["other_title"] == "private surprise"
                && c["other_item_id"] == hidden.json()["id"])
    );
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn board_bounds_use_household_projection_with_hidden_card_and_never_derive(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let mut cards = Vec::new();
    for (title, owner) in [
        ("A", f.users[0].member_id),
        ("hidden", f.users[1].member_id),
        ("B", f.users[0].member_id),
        ("mover", f.users[0].member_id),
    ] {
        let made = trust::admin(
            &f.server,
            "POST",
            "/v1/items",
            json!({"title":title,"owner_id":owner,"visibility":"private"}),
        )
        .await;
        assert_eq!(made.status, 201);
        let rows = occs(&f, &made.json()["id"]).await;
        cards.push(rows[0].clone());
    }
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM porchlight.outbox WHERE kind='derived'")
            .fetch_one(&f.server.state.pool)
            .await
            .unwrap();
    let path = format!("/v1/occurrences/{}", cards[3]["id"].as_str().unwrap());
    let d = &f.users[0].device;
    let mut req = d.request(
        "PATCH",
        &path,
        json!({"after_id":cards[0]["id"],"before_id":cards[2]["id"]}),
        None,
    );
    req.headers_mut()
        .insert("if-match", cards[3]["version"].to_string().parse().unwrap());
    let moved = TestClient::new(&f.server.ca_pem, Some(d))
        .request(f.server.lan, req)
        .await
        .unwrap();
    assert_eq!(moved.status, 200, "{moved:?}");
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM porchlight.item_occurrences ORDER BY board_position COLLATE \"C\",id",
    )
    .fetch_all(&f.server.state.pool)
    .await
    .unwrap();
    let expected: Vec<Uuid> = [0, 3, 1, 2]
        .iter()
        .map(|i| cards[*i]["id"].as_str().unwrap().parse().unwrap())
        .collect();
    assert_eq!(ids, expected);
    let after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM porchlight.outbox WHERE kind='derived'")
            .fetch_one(&f.server.state.pool)
            .await
            .unwrap();
    assert_eq!(before, after);
    let end = write(
        &f,
        "PATCH",
        &path,
        &moved.json()["version"],
        json!({"board_move":{}}),
    )
    .await;
    assert_eq!(end.status, 200, "{end:?}");
    let combined = write(
        &f,
        "PATCH",
        &path,
        &end.json()["version"],
        json!({"status":"doing","board_move":{}}),
    )
    .await;
    assert_eq!(combined.status, 200, "{combined:?}");
    assert_eq!(combined.json()["status"], "doing");
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn errors_after_split_roll_back_every_domain_effect_before_sealing(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let made=trust::admin(&f.server,"POST","/v1/items",json!({"title":"atomic","kind":"appointment","tzid":"UTC","starts_at":"2026-11-01T10:00:00Z","ends_at":"2026-11-01T11:00:00Z","rrule":"FREQ=DAILY;COUNT=5","recurrence_mode":"calendar"})).await;
    assert_eq!(made.status, 201);
    let item = made.json();
    let before = occs(&f, &item["id"]).await;
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.outbox")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    // Valid on the head input, invalid on the new tail: failure occurs AFTER split effects.
    let body = json!({"scope":"this_and_future","occurrence_id":before[3]["id"],"ends_at":"2026-11-03T11:00:00Z"});
    let path = format!("/v1/items/{}", item["id"].as_str().unwrap());
    let r = write(&f, "PATCH", &path, &item["version"], body).await;
    assert_eq!(r.status, 422, "{r:?}");
    assert_eq!(r.json()["errors"][0]["field"], "ends_at");
    let after = occs(&f, &item["id"]).await;
    assert_eq!(before, after);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.items")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let after_events: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.outbox")
        .fetch_one(&f.server.state.pool)
        .await
        .unwrap();
    assert_eq!(events, after_events);
    let fetched = trust::admin(&f.server, "GET", &path, json!({})).await;
    assert_eq!(fetched.json()["version"], item["version"]);
    let fixed = write(
        &f,
        "PATCH",
        &path,
        &item["version"],
        json!({"scope":"this_and_future","occurrence_id":before[3]["id"],"title":"valid tail"}),
    )
    .await;
    assert_eq!(fixed.status, 200, "{fixed:?}");
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn item_lock_timeout_is_transient_and_same_key_can_retry(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            db_pool_size: 1,
            ..options()
        },
    )
    .await;
    let made = trust::admin(&f.server, "POST", "/v1/items", json!({"title":"locked"})).await;
    assert_eq!(made.status, 201);
    let item = made.json();
    sqlx::query("SET lock_timeout='100ms'")
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    let mut holder = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM porchlight.items WHERE id=$1 FOR UPDATE")
        .bind(item["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .execute(&mut *holder)
        .await
        .unwrap();
    let path = format!("/v1/items/{}", item["id"].as_str().unwrap());
    let send = || {
        let mut req = trust::req("PATCH", &path, json!({"title":"retried"}), "lock-retry");
        req.headers_mut()
            .insert("if-match", item["version"].to_string().parse().unwrap());
        req
    };
    let blocked = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        agent_request(
            &f.server.agent_socket,
            Some(f.server.bootstrap_token()),
            send(),
        ),
    )
    .await
    .expect("IdemTx only, no pool SQL in callback");
    assert_eq!(blocked.status, 409, "{blocked:?}");
    assert!(
        blocked.json()["type"]
            .as_str()
            .unwrap()
            .ends_with(":conflict")
    );
    assert_eq!(blocked.headers.get("retry-after").unwrap(), "1");
    let stored: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.idempotency_keys WHERE idem_key='lock-retry'",
    )
    .fetch_one(&mut *holder)
    .await
    .unwrap();
    assert_eq!(stored, 0);
    holder.rollback().await.unwrap();
    // The 100 ms limit only makes the held-item wait above fail fast. Restore the production
    // per-lock limit for the retry: lock_timeout also bounds the cluster-wide lock PostgreSQL takes
    // to COMMIT any NOTIFYing transaction (outbox triggers), which concurrent tests contend for.
    sqlx::query("RESET lock_timeout")
        .execute(&f.server.state.pool)
        .await
        .unwrap();
    let success = agent_request(
        &f.server.agent_socket,
        Some(f.server.bootstrap_token()),
        send(),
    )
    .await;
    assert_eq!(success.status, 200, "{success:?}");
    assert_eq!(success.json()["title"], "retried");
    let replay = agent_request(
        &f.server.agent_socket,
        Some(f.server.bootstrap_token()),
        send(),
    )
    .await;
    assert_eq!(success.body, replay.body);
    f.server.stop().await;
}

#[test]
fn typed_scope_retries_are_transient_only_and_wire_validation_is_preserved() {
    use porchlight_db::derive::Error;
    let e = porchlight_api::items::domain_error(Error::RetryableScope);
    assert!(e.is_transient());
    assert_eq!(e.status(), 409);
    assert_eq!(e.retry_after(), Some(1));
    let e = porchlight_api::items::domain_error(Error::Validation(
        porchlight_core::items::ValidationError {
            field: "local_due",
            reason: "Required".into(),
        },
    ));
    assert_eq!(e.status(), 422);
    assert!(!e.is_transient());
    assert_eq!(
        porchlight_api::items::domain_error(Error::ScopeAlreadyUsed).status(),
        500
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn pagination_filters_transfer_eviction_and_wire_field_errors(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let d = &f.users[0].device;
    let client = TestClient::new(&f.server.ca_pem, Some(d));
    let mut tasks = Vec::new();
    for name in ["one", "two", "three"] {
        let made = client
            .request(
                f.server.lan,
                d.request(
                    "POST",
                    "/v1/items",
                    json!({"title":name,"visibility":"private"}),
                    None,
                ),
            )
            .await
            .unwrap();
        assert_eq!(made.status, 201);
        tasks.push(made.json());
    }
    let first = client
        .request(
            f.server.lan,
            d.request("GET", "/v1/items?limit=2&kind=todo", json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(first.status, 200);
    assert_eq!(first.json()["items"].as_array().unwrap().len(), 2);
    let cursor = first.json()["next_cursor"].as_str().unwrap().to_owned();
    let second = client
        .request(
            f.server.lan,
            d.request(
                "GET",
                &format!("/v1/items?limit=2&kind=todo&cursor={cursor}"),
                json!({}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(second.status, 200);
    assert_eq!(second.json()["items"].as_array().unwrap().len(), 1);
    assert!(second.json()["next_cursor"].is_null());
    assert!(
        first.json()["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["id"] != second.json()["items"][0]["id"])
    );
    let tampered = client
        .request(
            f.server.lan,
            d.request(
                "GET",
                &format!("/v1/items?kind=deadline&cursor={cursor}"),
                json!({}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(tampered.status, 400);
    let future = client
        .request(
            f.server.lan,
            d.request(
                "GET",
                "/v1/items?updated_since=2100-01-01T00:00:00Z",
                json!({}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(future.json()["items"], json!([]));
    let denied = client
        .request(
            f.server.lan,
            d.request("GET", "/v1/items?include_deleted=true", json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(denied.status, 403);
    let path = format!("/v1/items/{}", tasks[0]["id"].as_str().unwrap());
    let mut req = d.request(
        "PATCH",
        &path,
        json!({"owner_id":f.users[1].member_id}),
        Some("transfer"),
    );
    req.headers_mut()
        .insert("if-match", tasks[0]["version"].to_string().parse().unwrap());
    let evicted = client.request(f.server.lan, req).await.unwrap();
    assert_eq!(evicted.status, 204);
    assert!(evicted.body.is_empty());
    let hidden = client
        .request(f.server.lan, d.request("GET", &path, json!({}), None))
        .await
        .unwrap();
    assert_eq!(hidden.status, 404);
    assert!(!String::from_utf8_lossy(&hidden.body).contains("one"));
    let fields = [
        ("kind", json!("alien")),
        ("time_mode", json!("alien")),
        ("recurrence_mode", json!("alien")),
        ("missed_policy", json!("alien")),
        ("priority", json!(4)),
        ("title", json!("")),
        ("notes", json!("n".repeat(20001))),
        ("address", json!("a".repeat(501))),
        ("source", json!("s".repeat(201))),
        ("due_at", json!("not-a-date")),
        ("buffer_after_min", json!(-1)),
    ];
    for (field, value) in fields {
        let mut body = json!({"title":"invalid"});
        body[field] = value;
        let invalid = client
            .request(f.server.lan, d.request("POST", "/v1/items", body, None))
            .await
            .unwrap();
        assert_eq!(invalid.status, 422, "{field}: {invalid:?}");
        assert_eq!(invalid.json()["errors"][0]["field"], field);
    }
    let item = &tasks[1];
    let path = format!("/v1/items/{}", item["id"].as_str().unwrap());
    let deleted = write(&f, "DELETE", &path, &item["version"], json!({})).await;
    assert_eq!(deleted.status, 200);
    let list = trust::admin(
        &f.server,
        "GET",
        "/v1/items?include_deleted=true",
        json!({}),
    )
    .await;
    assert_eq!(list.status, 200);
    assert_eq!(list.json()["items"].as_array().unwrap().len(), 3);
    assert!(
        list.json()["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == item["id"] && !r["deleted_at"].is_null())
    );
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reopening_rule_removed_history_commits_the_regeneration_deletion(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let made=trust::admin(&f.server,"POST","/v1/items",json!({"title":"shortened","kind":"deadline","tzid":"UTC","due_at":"2026-11-01T12:00:00Z","rrule":"FREQ=DAILY;COUNT=3","recurrence_mode":"calendar"})).await;
    assert_eq!(made.status, 201);
    let item = made.json();
    let before = occs(&f, &item["id"]).await;
    let id = before[2]["id"].as_str().unwrap();
    let complete = write(
        &f,
        "POST",
        &format!("/v1/occurrences/{id}/complete"),
        &before[2]["version"],
        json!({}),
    )
    .await;
    assert_eq!(complete.status, 200);
    let shortened = write(
        &f,
        "PATCH",
        &format!("/v1/items/{}", item["id"].as_str().unwrap()),
        &item["version"],
        json!({"rrule":"FREQ=DAILY;COUNT=1"}),
    )
    .await;
    assert_eq!(shortened.status, 200);
    let retained = occs(&f, &item["id"]).await;
    assert_eq!(retained.as_array().unwrap().len(), 2);
    let reopened = write(
        &f,
        "POST",
        &format!("/v1/occurrences/{id}/reopen"),
        &complete.json()["version"],
        json!({}),
    )
    .await;
    assert_eq!(reopened.status, 204, "{reopened:?}");
    assert!(reopened.body.is_empty());
    let after = occs(&f, &item["id"]).await;
    assert_eq!(after.as_array().unwrap().len(), 1);
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn take_business_eligibility_applies_to_agents_too(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    for body in [
        json!({"title":"not about a dependent"}),
        json!({"title":"already assigned","subject_member_id":f.dependent,"assignees":[f.users[0].member_id]}),
    ] {
        let made = trust::admin(&f.server, "POST", "/v1/items", body).await;
        assert_eq!(made.status, 201);
        let item = made.json();
        let taken = write(
            &f,
            "POST",
            &format!("/v1/items/{}/take", item["id"].as_str().unwrap()),
            &item["version"],
            json!({}),
        )
        .await;
        assert_eq!(taken.status, 403, "{taken:?}");
    }
    let made=trust::admin(&f.server,"POST","/v1/items",json!({"title":"eligible","subject_member_id":f.dependent,"visibility":"private","owner_id":f.users[1].member_id})).await;
    assert_eq!(made.status, 201);
    let item = made.json();
    let taken = write(
        &f,
        "POST",
        &format!("/v1/items/{}/take", item["id"].as_str().unwrap()),
        &item["version"],
        json!({}),
    )
    .await;
    assert_eq!(taken.status, 200);
    assert_eq!(taken.json()["assignees"], json!([f.admin.member_id]));
    f.server.stop().await;
}

async fn domain_footprint(pool: &PgPool, id: &Value) -> (i64, i64, i64) {
    sqlx::query_as("SELECT (SELECT count(*) FROM porchlight.outbox),(SELECT count(*) FROM porchlight.task_events),(SELECT requested_gen FROM porchlight.item_derivation WHERE item_id=$1)")
        .bind(id.as_str().unwrap().parse::<Uuid>().unwrap()).fetch_one(pool).await.unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn repeated_status_is_a_read_and_does_not_move_completion_successor(pool: PgPool) {
    let clock = Arc::new(std::sync::atomic::AtomicI64::new(0));
    let shared = clock.clone();
    let mut opts = options();
    opts.item_clock = Arc::new(move || {
        "2026-10-05T12:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap()
            + chrono::Duration::days(shared.load(std::sync::atomic::Ordering::SeqCst))
    });
    let f = HouseholdFixture::build_with(&pool, opts).await;
    let made = trust::admin(&f.server,"POST","/v1/items",json!({"title":"no-op series","kind":"deadline","tzid":"UTC","due_at":"2026-10-05T12:00:00Z","recurrence_mode":"completion","completion_interval":{"days":7,"months":0,"microseconds":0}})).await;
    assert_eq!(made.status, 201, "{made:?}");
    let id = made.json()["id"].clone();
    let all = occs(&f, &id).await;
    let path = format!("/v1/occurrences/{}", all[0]["id"].as_str().unwrap());
    for (method, suffix, body) in [
        ("POST", "/reopen", json!({})),
        ("PATCH", "", json!({"status":"todo"})),
    ] {
        let before = occs(&f, &id).await;
        let footprint = domain_footprint(&pool, &id).await;
        let out = write(
            &f,
            method,
            &format!("{path}{suffix}"),
            &before[0]["version"],
            body,
        )
        .await;
        assert_eq!(out.status, 200, "{out:?}");
        assert_eq!(occs(&f, &id).await, before, "open no-op changed resource");
        assert_eq!(domain_footprint(&pool, &id).await, footprint);
    }
    for status in ["doing", "backlog"] {
        let before = occs(&f, &id).await;
        let changed = write(
            &f,
            "PATCH",
            &path,
            &before[0]["version"],
            json!({"status":status}),
        )
        .await;
        assert_eq!(changed.status, 200);
        let before = occs(&f, &id).await;
        let footprint = domain_footprint(&pool, &id).await;
        let noop = write(
            &f,
            "POST",
            &format!("{path}/reopen"),
            &before[0]["version"],
            json!({}),
        )
        .await;
        assert_eq!(noop.status, 200);
        assert_eq!(
            occs(&f, &id).await,
            before,
            "reopening any open state must be a no-op"
        );
        assert_eq!(domain_footprint(&pool, &id).await, footprint);
    }
    let before = occs(&f, &id).await;
    let empty = write(&f, "PATCH", &path, &before[0]["version"], json!({})).await;
    assert_eq!(empty.status, 422);
    let open = occs(&f, &id).await;
    let done = write(
        &f,
        "POST",
        &format!("{path}/complete"),
        &open[0]["version"],
        json!({}),
    )
    .await;
    assert_eq!(done.status, 200, "{done:?}");
    clock.store(2, std::sync::atomic::Ordering::SeqCst);
    for (method, suffix, body) in [
        ("POST", "/complete", json!({})),
        ("PATCH", "", json!({"status":"done"})),
    ] {
        let before = occs(&f, &id).await;
        let footprint: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM porchlight.outbox),(SELECT count(*) FROM porchlight.task_events),(SELECT requested_gen FROM porchlight.item_derivation WHERE item_id=$1)").bind(id.as_str().unwrap().parse::<Uuid>().unwrap()).fetch_one(&pool).await.unwrap();
        let out = write(
            &f,
            method,
            &format!("{path}{suffix}"),
            &before[0]["version"],
            body,
        )
        .await;
        assert_eq!(out.status, 200, "{out:?}");
        assert_eq!(
            occs(&f, &id).await,
            before,
            "completion re-anchored successor or restamped history"
        );
        let after: (i64,i64,i64) = sqlx::query_as("SELECT (SELECT count(*) FROM porchlight.outbox),(SELECT count(*) FROM porchlight.task_events),(SELECT requested_gen FROM porchlight.item_derivation WHERE item_id=$1)").bind(id.as_str().unwrap().parse::<Uuid>().unwrap()).fetch_one(&pool).await.unwrap();
        assert_eq!(footprint, after);
    }
    let before = occs(&f, &id).await;
    let skipped = write(
        &f,
        "PATCH",
        &path,
        &before[0]["version"],
        json!({"status":"skipped"}),
    )
    .await;
    assert_eq!(skipped.status, 200, "{skipped:?}");
    let before = occs(&f, &id).await;
    let footprint = domain_footprint(&pool, &id).await;
    let skipped = write(
        &f,
        "PATCH",
        &path,
        &before[0]["version"],
        json!({"status":"skipped"}),
    )
    .await;
    assert_eq!(skipped.status, 200, "{skipped:?}");
    assert_eq!(occs(&f, &id).await, before);
    assert_eq!(domain_footprint(&pool, &id).await, footprint);
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn fixed_clock_done_card_can_move_independent_of_machine_date(pool: PgPool) {
    let mut opts = options();
    let days = Arc::new(std::sync::atomic::AtomicI64::new(0));
    let clock = days.clone();
    opts.item_clock = Arc::new(move || {
        "2000-01-01T12:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap()
            + chrono::Duration::days(clock.load(std::sync::atomic::Ordering::SeqCst))
    });
    let f = HouseholdFixture::build_with(&pool, opts).await;
    let made = trust::admin(
        &f.server,
        "POST",
        "/v1/items",
        json!({"title":"ancient Done"}),
    )
    .await;
    let all = occs(&f, &made.json()["id"]).await;
    let path = format!("/v1/occurrences/{}", all[0]["id"].as_str().unwrap());
    let done = write(
        &f,
        "POST",
        &format!("{path}/complete"),
        &all[0]["version"],
        json!({}),
    )
    .await;
    assert_eq!(done.status, 200);
    let moved = write(
        &f,
        "PATCH",
        &path,
        &done.json()["version"],
        json!({"board_move":{}}),
    )
    .await;
    assert_eq!(moved.status, 200, "{moved:?}");
    days.store(8, std::sync::atomic::Ordering::SeqCst);
    let expired = write(
        &f,
        "PATCH",
        &path,
        &moved.json()["version"],
        json!({"board_move":{}}),
    )
    .await;
    assert_eq!(expired.status, 422, "{expired:?}");
    assert_eq!(expired.json()["errors"][0]["field"], "board_move");
    assert_eq!(expired.json()["errors"][0]["code"], "invalid-reference");
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn body_reference_errors_are_uniform_and_projected_neighbours_are_distinct(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let series = trust::admin(&f.server,"POST","/v1/items",json!({"title":"boundary","owner_id":f.users[0].member_id,"kind":"deadline","tzid":"UTC","due_at":"2026-10-05T12:00:00Z","rrule":"FREQ=DAILY;COUNT=3","recurrence_mode":"calendar"})).await.json();
    let rows = occs(&f, &series["id"]).await;
    let path = format!("/v1/items/{}", series["id"].as_str().unwrap());
    let other = trust::admin(
        &f.server,
        "POST",
        "/v1/items",
        json!({"title":"other","visibility":"private","owner_id":f.users[1].member_id}),
    )
    .await
    .json();
    let other_rows = occs(&f, &other["id"]).await;
    for invalid in [json!(Uuid::now_v7()), other_rows[0]["id"].clone()] {
        let out = write(
            &f,
            "PATCH",
            &path,
            &series["version"],
            json!({"scope":"this_and_future","occurrence_id":invalid,"title":"tail"}),
        )
        .await;
        assert_eq!(out.status, 422, "{out:?}");
        assert!(
            out.json()["type"]
                .as_str()
                .unwrap()
                .ends_with("validation-failed")
        );
        assert_eq!(out.json()["errors"][0]["field"], "occurrence_id");
        assert_eq!(out.json()["errors"][0]["code"], "invalid-reference");
        let mut req = f.users[0].device.request(
            "PATCH",
            &path,
            json!({"scope":"this_and_future","occurrence_id":invalid,"title":"tail"}),
            None,
        );
        req.headers_mut()
            .insert("if-match", series["version"].to_string().parse().unwrap());
        let app = TestClient::new(&f.server.ca_pem, Some(&f.users[0].device))
            .request(f.server.lan, req)
            .await
            .unwrap();
        assert_eq!(app.status, 422, "{app:?}");
        assert_eq!(app.json()["errors"], out.json()["errors"]);
    }
    let own = trust::admin(
        &f.server,
        "POST",
        "/v1/items",
        json!({"title":"mover","owner_id":f.users[0].member_id}),
    )
    .await
    .json();
    let own_rows = occs(&f, &own["id"]).await;
    let move_path = format!("/v1/occurrences/{}", own_rows[0]["id"].as_str().unwrap());
    let client = TestClient::new(&f.server.ca_pem, Some(&f.users[0].device));
    let wrong_column = trust::admin(
        &f.server,
        "POST",
        "/v1/items",
        json!({"title":"doing","visibility":"household"}),
    )
    .await
    .json();
    let wrong_rows = occs(&f, &wrong_column["id"]).await;
    let changed = write(
        &f,
        "PATCH",
        &format!("/v1/occurrences/{}", wrong_rows[0]["id"].as_str().unwrap()),
        &wrong_rows[0]["version"],
        json!({"status":"doing"}),
    )
    .await;
    assert_eq!(changed.status, 200);
    for field in ["after_id", "before_id"] {
        for (invalid, code) in [
            (json!(Uuid::now_v7()), "invalid-reference"),
            (other_rows[0]["id"].clone(), "invalid-reference"),
            (wrong_rows[0]["id"].clone(), "invalid-reference"),
            (rows[1]["id"].clone(), "stale-neighbour"),
        ] {
            let mut req =
                f.users[0]
                    .device
                    .request("PATCH", &move_path, json!({field:invalid}), None);
            req.headers_mut().insert(
                "if-match",
                own_rows[0]["version"].to_string().parse().unwrap(),
            );
            let out = client.request(f.server.lan, req).await.unwrap();
            assert_eq!(out.status, 422, "{out:?}");
            assert_eq!(out.json()["errors"][0]["field"], field);
            assert_eq!(out.json()["errors"][0]["code"], code);
        }
    }
    for (after, before, missing_field) in [
        (&rows[0]["id"], &rows[1]["id"], "before_id"),
        (&rows[1]["id"], &rows[0]["id"], "after_id"),
    ] {
        let out = write(
            &f,
            "PATCH",
            &move_path,
            &own_rows[0]["version"],
            json!({"after_id":after,"before_id":before}),
        )
        .await;
        assert_eq!(out.status, 422, "{out:?}");
        assert_eq!(out.json()["errors"][0]["field"], missing_field);
        assert_eq!(out.json()["errors"][0]["code"], "stale-neighbour");
    }
    for field in ["assignees", "viewers"] {
        let out = write(
            &f,
            "PUT",
            &format!("{path}/{field}"),
            &series["version"],
            json!({field:[Uuid::now_v7()]}),
        )
        .await;
        assert_eq!(out.status, 422);
        assert_eq!(out.json()["errors"][0]["field"], field);
    }
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn malformed_path_and_btrim_length_return_client_errors(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    for method in ["GET", "PATCH", "DELETE"] {
        let out = write(&f, method, "/v1/items/not-a-uuid", &json!(1), json!({})).await;
        assert_eq!(out.status, 400, "{out:?}");
    }
    let out = trust::admin(
        &f.server,
        "POST",
        "/v1/items",
        json!({"title":format!("\t{}\t","x".repeat(200))}),
    )
    .await;
    assert_eq!(out.status, 422, "{out:?}");
    assert_eq!(out.json()["errors"][0]["field"], "title");
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn occurrence_pages_honour_signed_cursor_and_tail_timing_is_validated_on_tail(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let made = trust::admin(&f.server,"POST","/v1/items",json!({"title":"paged tail","kind":"appointment","tzid":"UTC","starts_at":"2026-10-05T10:00:00Z","ends_at":"2026-10-05T11:00:00Z","rrule":"FREQ=DAILY;COUNT=4","recurrence_mode":"calendar"})).await.json();
    let path = format!("/v1/items/{}/occurrences", made["id"].as_str().unwrap());
    let first = trust::admin(&f.server, "GET", &format!("{path}?limit=1"), json!({})).await;
    assert_eq!(first.status, 200);
    assert_eq!(first.json()["items"].as_array().unwrap().len(), 1);
    let cursor = first.json()["next_cursor"].as_str().unwrap().to_string();
    let next = trust::admin(
        &f.server,
        "GET",
        &format!("{path}?limit=1&cursor={cursor}"),
        json!({}),
    )
    .await;
    assert_eq!(next.status, 200);
    assert_eq!(next.json()["items"][0]["seq"], 1);
    let misuse = trust::admin(
        &f.server,
        "GET",
        &format!("{path}?limit=1&from=2026-10-06T00:00:00Z&cursor={cursor}"),
        json!({}),
    )
    .await;
    assert_eq!(misuse.status, 400);
    let cross_caller = TestClient::new(&f.server.ca_pem, Some(&f.users[0].device))
        .request(
            f.server.lan,
            f.users[0].device.request(
                "GET",
                &format!("{path}?limit=1&cursor={cursor}"),
                json!({}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(cross_caller.status, 400);
    let other = trust::admin(
        &f.server,
        "POST",
        "/v1/items",
        json!({"title":"other cursor item"}),
    )
    .await;
    assert_eq!(other.status, 201);
    let cross_item = trust::admin(
        &f.server,
        "GET",
        &format!(
            "/v1/items/{}/occurrences?limit=1&cursor={cursor}",
            other.json()["id"].as_str().unwrap()
        ),
        json!({}),
    )
    .await;
    assert_eq!(cross_item.status, 400);
    let out = write(&f,"PATCH",&format!("/v1/items/{}",made["id"].as_str().unwrap()),&made["version"],json!({"scope":"this_and_future","occurrence_id":next.json()["items"][0]["id"],"starts_at":"2026-10-06T10:30:00Z"})).await;
    assert_eq!(out.status, 200, "{out:?}");
    assert_eq!(out.json()["starts_at"], "2026-10-06T10:30:00Z");
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn concurrent_take_is_versioned_and_noncreate_key_mismatch_is_422(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let item = trust::admin(
        &f.server,
        "POST",
        "/v1/items",
        json!({"title":"take race","subject_member_id":f.dependent}),
    )
    .await
    .json();
    let path = format!("/v1/items/{}/take", item["id"].as_str().unwrap());
    let mut a = f.users[0].device.request("POST", &path, json!({}), None);
    let mut b = f.users[1].device.request("POST", &path, json!({}), None);
    for req in [&mut a, &mut b] {
        req.headers_mut()
            .insert("if-match", item["version"].to_string().parse().unwrap());
    }
    let ca = TestClient::new(&f.server.ca_pem, Some(&f.users[0].device))
        .with_timeout(QUEUED_RESPONSE_WITHIN);
    let cb = TestClient::new(&f.server.ca_pem, Some(&f.users[1].device))
        .with_timeout(QUEUED_RESPONSE_WITHIN);
    let mut hold = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM porchlight.items WHERE id=$1 FOR UPDATE")
        .bind(item["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .fetch_one(&mut *hold)
        .await
        .unwrap();
    let holder: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *hold)
        .await
        .unwrap();
    let barrier = tokio::sync::Barrier::new(3);
    let ra = async {
        barrier.wait().await;
        ca.request(f.server.lan, a).await.unwrap()
    };
    let rb = async {
        barrier.wait().await;
        cb.request(f.server.lan, b).await.unwrap()
    };
    let release = async {
        barrier.wait().await;
        wait_blocked_behind(&pool, holder, 2, OVERLAP_WITHIN)
            .await
            .expect("both takes must overlap behind the held item");
        hold.commit().await.unwrap();
    };
    let (a, b, ()) = tokio::join!(ra, rb, release);
    assert!(
        (a.status == 200 && b.status == 409) || (a.status == 409 && b.status == 200),
        "{a:?} {b:?}"
    );
    let loser = if a.status == 409 { a } else { b };
    assert!(
        loser.json()["type"]
            .as_str()
            .unwrap()
            .ends_with("version-conflict")
    );
    let current = trust::admin(
        &f.server,
        "GET",
        &format!("/v1/items/{}", item["id"].as_str().unwrap()),
        json!({}),
    )
    .await
    .json();
    assert_eq!(current["assignees"].as_array().unwrap().len(), 1);
    let path = format!("/v1/items/{}", item["id"].as_str().unwrap());
    let mut req = trust::req("PATCH", &path, json!({"notes":"one"}), "noncreate-mismatch");
    req.headers_mut()
        .insert("if-match", current["version"].to_string().parse().unwrap());
    let first = agent_request(
        &f.server.agent_socket,
        Some(f.server.bootstrap_token()),
        req,
    )
    .await;
    assert_eq!(first.status, 200);
    let mut req = trust::req("PATCH", &path, json!({"notes":"two"}), "noncreate-mismatch");
    req.headers_mut().insert(
        "if-match",
        first.json()["version"].to_string().parse().unwrap(),
    );
    let mismatch = agent_request(
        &f.server.agent_socket,
        Some(f.server.bootstrap_token()),
        req,
    )
    .await;
    assert_eq!(mismatch.status, 422);
    assert!(
        mismatch.json()["type"]
            .as_str()
            .unwrap()
            .ends_with("idempotency-key-reused")
    );
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn occurrence_window_uses_reported_zone(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let made=trust::admin(&f.server,"POST","/v1/items",json!({"title":"floating window","kind":"appointment","time_mode":"floating","local_start":"2026-10-05T10:00:00","local_end":"2026-10-05T11:00:00"})).await;
    assert_eq!(made.status, 201, "{made:?}");
    let path = format!(
        "/v1/items/{}/occurrences?from=2026-10-05T14:00:00Z&to=2026-10-05T15:00:00Z",
        made.json()["id"].as_str().unwrap()
    );
    sqlx::query("UPDATE porchlight.household_members SET tzid='UTC' WHERE id=$1")
        .bind(f.admin.member_id)
        .execute(&pool)
        .await
        .unwrap();
    let mut req = trust::req("GET", &path, json!({}), "zone-window");
    req.headers_mut()
        .insert("porchlight-tz", "America/New_York".parse().unwrap());
    let out = agent_request(
        &f.server.agent_socket,
        Some(f.server.bootstrap_token()),
        req,
    )
    .await;
    assert_eq!(out.status, 200);
    assert_eq!(out.json()["items"].as_array().unwrap().len(), 1);
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn domain_version_conflict_has_authorized_current_and_agenda_is_internal(pool: PgPool) {
    use axum::response::IntoResponse;
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let c = porchlight_api::extract::Caller::Member {
        member_id: f.users[0].member_id,
        user_id: Uuid::nil(),
        role: "user".into(),
        device_id: f.users[0].device.device_id,
        session_id: Uuid::nil(),
        household_id: f.server.init.household_id,
        client_addr: None,
        request_id: Uuid::nil(),
    };
    for (owner, expected) in [(f.users[0].member_id, 409), (f.users[1].member_id, 404)] {
        let item = trust::admin(
            &f.server,
            "POST",
            "/v1/items",
            json!({"title":"version detail","visibility":"private","owner_id":owner}),
        )
        .await
        .json();
        let id = item["id"].as_str().unwrap().parse::<Uuid>().unwrap();
        let e = porchlight_api::items::domain_error(porchlight_db::derive::Error::VersionConflict);
        let mut conn = pool.acquire().await.unwrap();
        let e = porchlight_api::items::attach_current(&mut conn, &c, id, false, e).await;
        assert_eq!(e.status().as_u16(), expected);
        let body = axum::body::to_bytes(e.into_response().into_body(), 65536)
            .await
            .unwrap();
        let problem: Value = serde_json::from_slice(&body).unwrap();
        if expected == 409 {
            assert_eq!(problem["current"]["id"], item["id"]);
        } else {
            assert!(problem.get("current").is_none());
        }
    }
    let e = porchlight_api::items::domain_error(porchlight_db::derive::Error::Agenda(
        porchlight_core::agenda::AgendaError::InvalidSpan,
    ));
    assert_eq!(e.status().as_u16(), 500);
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn invalid_create_member_references_precede_initial_owner_policy(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let client = TestClient::new(&f.server.ca_pem, Some(&f.users[0].device));
    for field in ["owner_id", "subject_member_id", "assignees", "viewers"] {
        let id = Uuid::now_v7();
        let value = if ["assignees", "viewers"].contains(&field) {
            json!([id])
        } else {
            json!(id)
        };
        let out = client
            .request(
                f.server.lan,
                f.users[0].device.request(
                    "POST",
                    "/v1/items",
                    json!({"title":"invalid member",field:value}),
                    None,
                ),
            )
            .await
            .unwrap();
        assert_eq!(out.status, 422, "{out:?}");
        assert_eq!(out.json()["errors"][0]["field"], field);
    }
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn list_and_get_conflicts_agree_during_pending_reassignment(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let other = trust::admin(
        &f.server,
        "POST",
        "/v1/items",
        json!({
            "title":"private counterpart", "visibility":"private", "owner_id":f.users[1].member_id,
            "assignees":[f.users[0].member_id], "kind":"appointment", "tzid":"UTC",
            "starts_at":"2026-10-05T14:00:00Z", "ends_at":"2026-10-05T15:00:00Z"
        }),
    )
    .await;
    assert_eq!(other.status, 201);
    let made = trust::admin(&f.server, "POST", "/v1/items", json!({
        "title":"visible source", "owner_id":f.users[0].member_id,
        "kind":"appointment", "tzid":"UTC", "starts_at":"2026-10-05T14:00:00Z", "ends_at":"2026-10-05T15:00:00Z"
    })).await;
    assert_eq!(made.status, 201);
    let id: Uuid = made.json()["id"].as_str().unwrap().parse().unwrap();
    for pending in [false, true] {
        let one = trust::admin(&f.server, "GET", &format!("/v1/items/{id}"), json!({})).await;
        let page = trust::admin(&f.server, "GET", "/v1/items", json!({})).await;
        assert_eq!(one.status, 200);
        assert_eq!(page.status, 200);
        let page = page.json();
        let listed = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == id.to_string())
            .unwrap();
        assert_eq!(listed["pending_derivation"], pending);
        assert_eq!(
            listed["conflicts"],
            one.json()["conflicts"],
            "pending={pending}"
        );
        if pending {
            assert!(
                listed["conflicts"].as_array().unwrap().is_empty(),
                "removed member agenda must not contribute conflicts"
            );
        } else {
            assert!(!listed["conflicts"].as_array().unwrap().is_empty());
            // Change authority without deriving: old member_agenda rows remain.
            let actor = porchlight_db::tx::Actor {
                member_id: Some(f.admin.member_id),
                device_id: Some(f.admin.device.device_id),
                request_id: Uuid::now_v7(),
                agent_label: None,
                token_id: None,
            };
            let mut tx = porchlight_db::tx::begin_as(&f.server.state.pool, &actor)
                .await
                .unwrap();
            sqlx::query("UPDATE porchlight.items SET owner_id=$2 WHERE id=$1")
                .bind(id)
                .bind(f.admin.member_id)
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    }
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn off_board_mover_is_uniform_invalid_reference(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let made = trust::admin(&f.server,"POST","/v1/items",json!({"title":"future mover", "kind":"deadline", "tzid":"UTC", "due_at":"2026-10-05T12:00:00Z", "rrule":"FREQ=DAILY;COUNT=3", "recurrence_mode":"calendar"})).await;
    assert_eq!(made.status, 201);
    let rows = occs(&f, &made.json()["id"]).await;
    for body in [
        json!({"board_move":{}}),
        json!({"after_id":rows[0]["id"]}),
        json!({"before_id":rows[0]["id"]}),
    ] {
        let out = write(
            &f,
            "PATCH",
            &format!("/v1/occurrences/{}", rows[1]["id"].as_str().unwrap()),
            &rows[1]["version"],
            body,
        )
        .await;
        assert_eq!(out.status, 422, "{out:?}");
        assert_eq!(out.json()["errors"][0]["field"], "board_move");
        assert_eq!(out.json()["errors"][0]["code"], "invalid-reference");
        assert_eq!(out.json()["errors"][0]["message"], "Invalid reference");
    }
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn collection_skips_rows_rejected_by_authoritative_acl(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let made = trust::admin(
        &f.server,
        "POST",
        "/v1/items",
        json!({"title":"household row"}),
    )
    .await;
    assert_eq!(made.status, 201);
    let caller = porchlight_api::extract::Caller::Member {
        member_id: f.dependent,
        user_id: Uuid::nil(),
        role: "dependent".into(),
        device_id: Uuid::nil(),
        session_id: Uuid::nil(),
        household_id: f.server.init.household_id,
        client_addr: None,
        request_id: Uuid::now_v7(),
    };
    let query = porchlight_api::routes::items::ListQuery {
        cursor: None,
        limit: Some(1),
        updated_since: None,
        kind: None,
        include_deleted: None,
    };
    let page = porchlight_api::items::list(&f.server.state, &caller, query)
        .await
        .unwrap_or_else(|_| panic!("an ACL-rejected row must not fail the page"));
    assert!(page.items.is_empty());
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn unchanged_status_composes_with_override_and_board_without_status_write(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let made = trust::admin(&f.server,"POST","/v1/items",json!({"title":"composed noop", "kind":"appointment", "tzid":"UTC", "starts_at":"2026-10-05T14:00:00Z", "ends_at":"2026-10-05T15:00:00Z"})).await;
    assert_eq!(made.status, 201);
    let rows = occs(&f, &made.json()["id"]).await;
    let path = format!("/v1/occurrences/{}", rows[0]["id"].as_str().unwrap());
    let edited = write(&f,"PATCH",&path,&rows[0]["version"],json!({"status":"todo", "override_local_start":"2026-10-05T14:30:00", "override_local_end":"2026-10-05T15:30:00"})).await;
    assert_eq!(edited.status, 200, "{edited:?}");
    assert_eq!(
        edited.json()["version"].as_i64().unwrap(),
        rows[0]["version"].as_i64().unwrap() + 1
    );
    assert_eq!(edited.json()["override_local_start"], "2026-10-05T14:30:00");
    let moved = write(
        &f,
        "PATCH",
        &path,
        &edited.json()["version"],
        json!({"status":"todo", "board_move":{}}),
    )
    .await;
    assert_eq!(moved.status, 200, "{moved:?}");
    assert_eq!(
        moved.json()["version"].as_i64().unwrap(),
        edited.json()["version"].as_i64().unwrap() + 1
    );
    assert_eq!(
        moved.json()["override_local_start"],
        edited.json()["override_local_start"]
    );
    assert!(moved.json()["completed_at"].is_null());
    assert!(moved.json()["completed_by"].is_null());
    f.server.stop().await;
}

// Place each fixture at the end of its target column, so bounds have a known order.
async fn board_card(f: &HouseholdFixture, title: &str, status: &str) -> Value {
    let made = trust::admin(&f.server, "POST", "/v1/items", json!({"title":title})).await;
    assert_eq!(made.status, 201, "{made:?}");
    let rows = occs(f, &made.json()["id"]).await;
    let path = format!("/v1/occurrences/{}", rows[0]["id"].as_str().unwrap());
    let changed = write(
        f,
        "PATCH",
        &path,
        &rows[0]["version"],
        json!({"status":status}),
    )
    .await;
    assert_eq!(changed.status, 200, "{changed:?}");
    let placed = write(
        f,
        "PATCH",
        &path,
        &changed.json()["version"],
        json!({"board_move":{}}),
    )
    .await;
    assert_eq!(placed.status, 200, "{placed:?}");
    placed.json()
}

async fn assert_board_order(f: &HouseholdFixture, cards: &[&Value]) {
    let expected: Vec<Uuid> = cards
        .iter()
        .map(|card| card["id"].as_str().unwrap().parse().unwrap())
        .collect();
    let actual: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM porchlight.item_occurrences WHERE id=ANY($1) ORDER BY board_position COLLATE \"C\",id")
        .bind(&expected).fetch_all(&f.server.state.pool).await.unwrap();
    assert_eq!(actual, expected);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn composed_status_board_move_completes_future_occurrence_between_done_neighbours(
    pool: PgPool,
) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let left = board_card(&f, "Done left", "done").await;
    let right = board_card(&f, "Done right", "done").await;
    let made = trust::admin(&f.server,"POST","/v1/items",json!({"title":"future completion", "kind":"deadline", "tzid":"UTC", "due_at":"2026-10-05T12:00:00Z", "rrule":"FREQ=DAILY;COUNT=3", "recurrence_mode":"calendar"})).await;
    assert_eq!(made.status, 201, "{made:?}");
    let rows = occs(&f, &made.json()["id"]).await;
    let path = format!("/v1/occurrences/{}", rows[1]["id"].as_str().unwrap());
    let placed = write(
        &f,
        "PATCH",
        &path,
        &rows[1]["version"],
        json!({"status":"done", "board_move":{"after_id":left["id"], "before_id":right["id"]}}),
    )
    .await;
    assert_eq!(placed.status, 200, "{placed:?}");
    assert_eq!(placed.json()["status"], "done");
    assert!(!placed.json()["completed_at"].is_null());
    assert_board_order(&f, &[&left, &placed.json(), &right]).await;
    let current = occs(&f, &made.json()["id"]).await;
    assert_eq!(current[0]["status"], "todo");
    assert_eq!(current[1], placed.json());
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn composed_status_board_move_unskips_between_todo_neighbours(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let left = board_card(&f, "Todo left", "todo").await;
    let right = board_card(&f, "Todo right", "todo").await;
    let card = board_card(&f, "unskip mover", "todo").await;
    let path = format!("/v1/occurrences/{}", card["id"].as_str().unwrap());
    let skipped = write(
        &f,
        "PATCH",
        &path,
        &card["version"],
        json!({"status":"skipped"}),
    )
    .await;
    assert_eq!(skipped.status, 200, "{skipped:?}");
    let placed = write(
        &f,
        "PATCH",
        &path,
        &skipped.json()["version"],
        json!({"status":"todo", "board_move":{"after_id":left["id"], "before_id":right["id"]}}),
    )
    .await;
    assert_eq!(placed.status, 200, "{placed:?}");
    assert_eq!(placed.json()["status"], "todo");
    assert_board_order(&f, &[&left, &placed.json(), &right]).await;
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn composed_status_board_move_skip_is_sealed_invalid_reference_and_replays(pool: PgPool) {
    let f = HouseholdFixture::build_with(&pool, options()).await;
    let card = board_card(&f, "skip mover", "todo").await;
    let path = format!("/v1/occurrences/{}", card["id"].as_str().unwrap());
    let before = domain_footprint(&f.server.state.pool, &card["item_id"]).await;
    let request = || {
        let mut req = trust::req(
            "PATCH",
            &path,
            json!({"status":"skipped", "board_move":{}}),
            "skip-board-sealed",
        );
        req.headers_mut()
            .insert("if-match", card["version"].to_string().parse().unwrap());
        req
    };
    let rejected = agent_request(
        &f.server.agent_socket,
        Some(f.server.bootstrap_token()),
        request(),
    )
    .await;
    assert_eq!(rejected.status, 422, "{rejected:?}");
    assert_eq!(rejected.json()["errors"][0]["field"], "board_move");
    assert_eq!(rejected.json()["errors"][0]["code"], "invalid-reference");
    assert_eq!(rejected.json()["errors"][0]["message"], "Invalid reference");
    assert!(!rejected.headers.contains_key("retry-after"));
    let replay = agent_request(
        &f.server.agent_socket,
        Some(f.server.bootstrap_token()),
        request(),
    )
    .await;
    assert_eq!(replay.status, 422, "{replay:?}");
    assert_eq!(replay.body, rejected.body);
    assert!(!replay.headers.contains_key("retry-after"));
    let sealed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.idempotency_keys WHERE idem_key='skip-board-sealed'",
    )
    .fetch_one(&f.server.state.pool)
    .await
    .unwrap();
    assert_eq!(sealed, 1);
    assert_eq!(
        domain_footprint(&f.server.state.pool, &card["item_id"]).await,
        before
    );
    let current = occs(&f, &card["item_id"]).await;
    assert_eq!(current[0], card);
    f.server.stop().await;
}
