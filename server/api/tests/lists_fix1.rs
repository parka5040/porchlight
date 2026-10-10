mod common;
use common::*;
use porchlight_api::{
    extract::Caller,
    lists,
    testing::{
        HouseholdFixture, TestClient, TestDevice, TestOptions, TestResponse, TestServer,
        agent_request,
    },
};
use porchlight_core::auth::session::Platform;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

fn ok(r: TestResponse) -> Value {
    assert!(r.status < 300, "status {}", r.status);
    r.json()
}
fn id(v: &Value) -> &str {
    v["id"].as_str().unwrap()
}
async fn write(
    s: &TestServer,
    d: &TestDevice,
    method: &str,
    path: &str,
    b: Value,
    v: &Value,
) -> TestResponse {
    let mut r = d.request(method, path, b, None);
    r.headers_mut()
        .insert("if-match", v.to_string().parse().unwrap());
    TestClient::new(&s.ca_pem, Some(d))
        .request(s.lan, r)
        .await
        .unwrap()
}
async fn setup(pool: &PgPool) -> (TestServer, TestDevice) {
    let s = TestServer::start_with(
        pool,
        TestOptions {
            background_derivation: false,
            db_pool_size: 2,
            item_clock: std::sync::Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    (s, d)
}
#[sqlx::test(migrations = "../db/migrations")]
async fn deleted_parent_is_absent_from_agent_collections_and_seams_but_owner_can_delete(
    pool: PgPool,
) {
    let (s, d) = setup(&pool).await;
    let item = ok(post(&s, &d, "/v1/items", json!({"title":"orphan"})).await);
    let l = ok(post(
        &s,
        &d,
        "/v1/lists",
        json!({"name":"orphan","item_id":item["id"]}),
    )
    .await);
    let live = ok(post(&s, &d, "/v1/lists", json!({"name":"live"})).await);
    ok(write(
        &s,
        &d,
        "DELETE",
        &format!("/v1/items/{}", id(&item)),
        json!({}),
        &item["version"],
    )
    .await);
    let page = ok(agent_request(
        &s.agent_socket,
        Some(s.bootstrap_token()),
        d.request("GET", "/v1/lists", Value::Null, None),
    )
    .await);
    assert_eq!(
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(id)
            .collect::<Vec<_>>(),
        [id(&live)]
    );
    let orphan_page = ok(get(&s, &d, "/v1/lists?include_orphaned=true").await);
    let orphan = orphan_page["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == l["id"])
        .expect("owner can discover orphan");
    assert_eq!(orphan["capabilities"], json!(["view", "delete"]));
    let c = Caller::Agent {
        token_id: Uuid::nil(),
        household_id: s.init.household_id,
        member_id: s.init.admin_member_id,
        scopes: vec!["admin".into()],
        label: "fix1".into(),
        request_id: Uuid::now_v7(),
    };
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(
        lists::snapshot_contribution(&mut conn, &c)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        lists::search_contribution(&mut conn, &c, "orphan")
            .await
            .unwrap()
            .is_empty()
    );
    drop(conn);
    let stale = write(
        &s,
        &d,
        "DELETE",
        &format!("/v1/lists/{}", id(&l)),
        json!({}),
        &json!(0),
    )
    .await;
    problem(&stale, 409, "version-conflict");
    assert_eq!(stale.json()["current"]["id"], l["id"]);
    assert_eq!(
        stale.json()["current"]["capabilities"],
        json!(["view", "delete"])
    );
    assert_eq!(
        write(
            &s,
            &d,
            "DELETE",
            &format!("/v1/lists/{}", id(&l)),
            json!({}),
            &l["version"]
        )
        .await
        .status,
        204
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn body_references_are_uniform_and_decode_names_fields(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let s = &f.server;
    let a = &f.users[0].device;
    let b = &f.users[1].device;
    let hidden = ok(post(
        s,
        a,
        "/v1/items",
        json!({"title":"secret","visibility":"private"}),
    )
    .await);
    for item in [
        hidden["id"].clone(),
        json!(Uuid::now_v7()),
        json!("not-uuid"),
    ] {
        let r = post(s, b, "/v1/lists", json!({"name":"x","item_id":item})).await;
        problem(&r, 422, "validation-failed");
        assert_eq!(r.json()["errors"][0]["field"], "item_id");
        assert_eq!(r.json()["errors"][0]["code"], "invalid-reference");
    }
    let plain = ok(post(s, a, "/v1/items", json!({"title":"plain"})).await);
    problem(
        &post(
            s,
            a,
            "/v1/lists",
            json!({"name":"x","item_id":plain["id"],"mode":"per_occurrence"}),
        )
        .await,
        422,
        "validation-failed",
    );
    for (field, value) in [
        ("owner_id", json!(Uuid::now_v7())),
        ("viewers", json!([Uuid::now_v7()])),
        ("name", json!(123)),
        ("surprise", json!(true)),
    ] {
        let mut body = json!({"name":"x"});
        body[field] = value;
        let r = post(s, a, "/v1/lists", body).await;
        problem(&r, 422, "validation-failed");
        assert_eq!(r.json()["errors"][0]["field"], field);
    }
    f.server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn split_tail_occurrence_keeps_head_list_checks(pool: PgPool) {
    let (s, d) = setup(&pool).await;
    let item=ok(post(&s,&d,"/v1/items",json!({"title":"meds","kind":"deadline","time_mode":"fixed","due_at":"2026-10-05T12:00:00Z","tzid":"UTC","rrule":"FREQ=DAILY;COUNT=4","recurrence_mode":"calendar"})).await);
    let os = ok(get(&s, &d, &format!("/v1/items/{}/occurrences", id(&item))).await);
    let os_head = os.clone();
    let occ = &os["items"][1]["id"];
    let l = ok(post(
        &s,
        &d,
        "/v1/lists",
        json!({"name":"meds","item_id":item["id"],"mode":"per_occurrence"}),
    )
    .await);
    let lp = format!("/v1/lists/{}", id(&l));
    let e = ok(post(&s, &d, &format!("{lp}/entries"), json!({"label":"dose"})).await);
    let ep = format!("/v1/list-entries/{}", id(&e));
    let checked_before = ok(write(
        &s,
        &d,
        "POST",
        &format!("{ep}/check"),
        json!({"occurrence_id":occ}),
        &e["version"],
    )
    .await);
    let tail = ok(write(
        &s,
        &d,
        "PATCH",
        &format!("/v1/items/{}", id(&item)),
        json!({"scope":"this_and_future","occurrence_id":occ,"title":"tail"}),
        &item["version"],
    )
    .await);
    let os = ok(get(&s, &d, &format!("/v1/items/{}/occurrences", id(&tail))).await);
    let occ = &os["items"][0]["id"];
    let ep = format!("/v1/list-entries/{}", id(&e));
    let checked = ok(write(
        &s,
        &d,
        "POST",
        &format!("{ep}/check"),
        json!({"occurrence_id":occ}),
        &checked_before["version"],
    )
    .await);
    assert_eq!(checked["version"], checked_before["version"]);
    assert!(checked["checked_at"].is_string());
    let shown = ok(get(
        &s,
        &d,
        &format!("{lp}?occurrence_id={}", occ.as_str().unwrap()),
    )
    .await);
    assert!(shown["entries"][0]["checked_at"].is_string());
    ok(write(
        &s,
        &d,
        "POST",
        &format!("{ep}/uncheck"),
        json!({"occurrence_id":occ}),
        &checked["version"],
    )
    .await);
    let page = ok(get(&s, &d, &format!("/v1/lists?item_id={}", id(&tail))).await);
    assert_eq!(page["items"][0]["id"], l["id"]);
    let tail_list = ok(post(
        &s,
        &d,
        "/v1/lists",
        json!({"name":"tail only","item_id":tail["id"],"mode":"per_occurrence"}),
    )
    .await);
    let tail_entry = ok(post(
        &s,
        &d,
        &format!("/v1/lists/{}/entries", id(&tail_list)),
        json!({"label":"tail dose"}),
    )
    .await);
    let unrelated = ok(post(&s, &d, "/v1/items", json!({"title":"unrelated"})).await);
    let unrelated_os = ok(get(&s, &d, &format!("/v1/items/{}/occurrences", id(&unrelated))).await);
    for (entry, occurrence) in [
        (&e, &unrelated_os["items"][0]["id"]),
        (&tail_entry, &os_head["items"][0]["id"]),
    ] {
        let current = ok(get(
            &s,
            &d,
            &format!("/v1/lists/{}", entry["list_id"].as_str().unwrap()),
        )
        .await);
        let current_entry = &current["entries"][0];
        let r = write(
            &s,
            &d,
            "POST",
            &format!("/v1/list-entries/{}/check", id(entry)),
            json!({"occurrence_id":occurrence}),
            &current_entry["version"],
        )
        .await;
        problem(&r, 422, "validation-failed");
        assert_eq!(r.json()["errors"][0]["code"], "invalid-reference");
    }
    let second = ok(write(&s,&d,"PATCH",&format!("/v1/items/{}",id(&tail)),json!({"scope":"this_and_future","occurrence_id":os["items"][1]["id"],"title":"second tail"}),&tail["version"]).await);
    let second_os = ok(get(&s, &d, &format!("/v1/items/{}/occurrences", id(&second))).await);
    let current = ok(get(&s, &d, &lp).await);
    ok(write(
        &s,
        &d,
        "POST",
        &format!("{ep}/check"),
        json!({"occurrence_id":second_os["items"][0]["id"]}),
        &current["entries"][0]["version"],
    )
    .await);
    let page = ok(get(&s, &d, &format!("/v1/lists?item_id={}", id(&second))).await);
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn standalone_readers_manage_entries_not_metadata_and_noops_write_nothing(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let s = &f.server;
    let a = &f.users[0].device;
    let b = &f.users[1].device;
    let l = ok(post(s, a, "/v1/lists", json!({"name":"shared"})).await);
    let lp = format!("/v1/lists/{}", id(&l));
    let writes_before:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM porchlight.outbox)+(SELECT count(*) FROM porchlight.list_revisions)+(SELECT count(*) FROM porchlight.list_entry_revisions)").fetch_one(&pool).await.unwrap();
    let unchanged = ok(write(s, a, "PATCH", &lp, json!({}), &l["version"]).await);
    assert_eq!(unchanged["version"], l["version"]);
    let clear = ok(write(
        s,
        b,
        "POST",
        &format!("{lp}/clear-checked"),
        json!({}),
        &l["version"],
    )
    .await);
    assert_eq!(clear["version"], l["version"]);
    let writes_after:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM porchlight.outbox)+(SELECT count(*) FROM porchlight.list_revisions)+(SELECT count(*) FROM porchlight.list_entry_revisions)").fetch_one(&pool).await.unwrap();
    assert_eq!(writes_before, writes_after);
    let e = ok(post(s, b, &format!("{lp}/entries"), json!({"label":"milk"})).await);
    let ep = format!("/v1/list-entries/{}", id(&e));
    let same = ok(write(s, b, "PATCH", &ep, json!({}), &e["version"]).await);
    assert_eq!(same["version"], e["version"]);
    let checked = ok(write(
        s,
        b,
        "POST",
        &format!("{ep}/check"),
        json!({}),
        &e["version"],
    )
    .await);
    let current = ok(get(s, a, &lp).await);
    assert_eq!(current["version"], l["version"]); // clear preconditions cover metadata, not check set
    ok(write(
        s,
        b,
        "POST",
        &format!("{ep}/uncheck"),
        json!({}),
        &checked["version"],
    )
    .await);
    problem(
        &write(s, b, "PATCH", &lp, json!({"name":"rename"}), &l["version"]).await,
        403,
        "forbidden",
    );
    problem(
        &write(s, b, "DELETE", &lp, json!({}), &l["version"]).await,
        403,
        "forbidden",
    );
    let stale = write(s, a, "PATCH", &lp, json!({"name":"rename"}), &json!(0)).await;
    problem(&stale, 409, "version-conflict");
    assert_eq!(stale.json()["current"]["id"], l["id"]);
    f.server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn ordering_rebalances_only_tied_or_exhausted_run(pool: PgPool) {
    let (s, d) = setup(&pool).await;
    for exhausted in [false, true] {
        let l = ok(post(&s, &d, "/v1/lists", json!({"name":"order"})).await);
        let lp = format!("/v1/lists/{}", id(&l));
        let keys = if exhausted {
            vec![
                "2080".to_string(),
                exhausted_gap().0,
                exhausted_gap().1,
                "c080".to_string(),
            ]
        } else {
            vec!["2080".into(), "80".into(), "80".into(), "c080".into()]
        };
        let mut es = vec![];
        for (i, key) in keys.iter().enumerate() {
            let e = ok(post(
                &s,
                &d,
                &format!("{lp}/entries"),
                json!({"label":format!("e{i}")}),
            )
            .await);
            let _ = key;
            es.push(e);
        }
        for (e, key) in es.iter().zip(&keys) {
            sqlx::query("UPDATE porchlight.list_entries SET position=$2 WHERE id=$1")
                .bind(Uuid::parse_str(id(e)).unwrap())
                .bind(key)
                .execute(&pool)
                .await
                .unwrap();
        }
        let rows:Vec<(Uuid,String,i32)>=sqlx::query_as("SELECT id,position,version FROM porchlight.list_entries WHERE list_id=$1 ORDER BY position COLLATE \"C\",id").bind(Uuid::parse_str(id(&l)).unwrap()).fetch_all(&pool).await.unwrap();
        let added = ok(post(
            &s,
            &d,
            &format!("{lp}/entries"),
            json!({"label":"insert","after_id":rows[1].0,"before_id":rows[2].0}),
        )
        .await);
        let now:Vec<(Uuid,String,i32)>=sqlx::query_as("SELECT id,position,version FROM porchlight.list_entries WHERE id=ANY($1) ORDER BY position COLLATE \"C\",id").bind(rows.iter().map(|r|r.0).collect::<Vec<_>>()).fetch_all(&pool).await.unwrap();
        assert_eq!(now[0], rows[0], "unrelated prefix changed");
        assert_eq!(now[3], rows[3], "unrelated suffix changed");
        assert!(now[1].1.as_str() < added["position"].as_str().unwrap());
        assert!(added["position"].as_str().unwrap() < now[2].1.as_str());
    }
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn concurrent_inserts_same_neighbours_both_orders_twenty_times(pool: PgPool) {
    let (s, d) = setup(&pool).await;
    for reverse in [false, true] {
        for _ in 0..20 {
            let l = ok(post(&s, &d, "/v1/lists", json!({"name":"race"})).await);
            let path = format!("/v1/lists/{}/entries", id(&l));
            let lo = ok(post(&s, &d, &path, json!({"label":"lo"})).await);
            let hi = ok(post(&s, &d, &path, json!({"label":"hi"})).await);
            let (low, high) = exhausted_gap();
            for (entry, key) in [(&lo, &low), (&hi, &high)] {
                sqlx::query("UPDATE porchlight.list_entries SET position=$2 WHERE id=$1")
                    .bind(Uuid::parse_str(id(entry)).unwrap())
                    .bind(key)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            let baseline = ok(get(&s, &d, &format!("/v1/lists/{}", id(&l))).await);
            let baseline = baseline["entries"].as_array().unwrap();
            assert_eq!(baseline[0]["position"], low);
            assert_eq!(baseline[1]["position"], high);
            let a = json!({"label":if reverse {"b"} else {"a"},"after_id":lo["id"],"before_id":hi["id"]});
            let b = json!({"label":if reverse {"a"} else {"b"},"after_id":lo["id"],"before_id":hi["id"]});
            let (a, b) = tokio::join!(post(&s, &d, &path, a), post(&s, &d, &path, b));
            assert_eq!(
                (a.status, b.status),
                (201, 201),
                "no deadlock/conflict allowed"
            );
            let (a, b) = (a.json(), b.json());
            assert_ne!(a["position"], b["position"]);
            let shown = ok(get(&s, &d, &format!("/v1/lists/{}", id(&l))).await);
            let rows = shown["entries"].as_array().unwrap();
            assert_eq!(rows.len(), 4);
            assert_eq!(rows[0]["id"], lo["id"]);
            assert_eq!(rows[3]["id"], hi["id"]);
            for (before, after) in [(&baseline[0], &rows[0]), (&baseline[1], &rows[3])] {
                assert_ne!(
                    after["position"], before["position"],
                    "rebalance changed neighbour key"
                );
                assert!(
                    after["version"].as_i64().unwrap() > before["version"].as_i64().unwrap(),
                    "rebalance changed neighbour version after setup UPDATE"
                );
            }
            assert!(
                rows.windows(2)
                    .all(|w| w[0]["position"].as_str() < w[1]["position"].as_str())
            );
        }
    }
    s.stop().await;
}

fn exhausted_gap() -> (String, String) {
    let lo = "80".to_string();
    let mut hi = "8180".to_string();
    while let Ok(key) = porchlight_core::ordering::key_between(Some(&lo), Some(&hi)) {
        hi = key;
    }
    (lo, hi)
}

#[sqlx::test(migrations = "../db/migrations")]
async fn member_collection_acl_metadata_evictions_and_reference_fields(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let s = &f.server;
    let a = &f.users[0];
    let b = &f.users[1];
    let hidden = ok(post(
        s,
        &a.device,
        "/v1/lists",
        json!({"name":"private","visibility":"private"}),
    )
    .await);
    let shared = ok(post(s, &a.device, "/v1/lists", json!({"name":"shared"})).await);
    let lp = format!("/v1/lists/{}", id(&shared));
    let page = ok(get(s, &b.device, "/v1/lists").await);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["id"], shared["id"]);
    let made_private = ok(write(
        s,
        &a.device,
        "PATCH",
        &lp,
        json!({"visibility":"private"}),
        &shared["version"],
    )
    .await);
    assert!(
        ok(get(s, &b.device, "/v1/lists").await)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let event:Value=sqlx::query_scalar("SELECT payload FROM porchlight.outbox WHERE entity_id=$1 AND kind='list.changed' AND payload->>'eviction'='true' ORDER BY id DESC LIMIT 1").bind(Uuid::parse_str(id(&shared)).unwrap()).fetch_one(&pool).await.unwrap();
    assert!(
        event["recipients"]
            .as_array()
            .unwrap()
            .contains(&json!(b.member_id))
    );
    let transferred = write(
        s,
        &a.device,
        "PATCH",
        &lp,
        json!({"owner_id":b.member_id}),
        &made_private["version"],
    )
    .await;
    assert_eq!(transferred.status, 204);
    problem(&get(s, &a.device, &lp).await, 404, "not-found");
    assert_eq!(
        ok(get(s, &b.device, &lp).await)["owner_id"],
        json!(b.member_id)
    );
    let event:Value=sqlx::query_scalar("SELECT payload FROM porchlight.outbox WHERE entity_id=$1 AND kind='list.changed' AND payload->>'eviction'='true' ORDER BY id DESC LIMIT 1").bind(Uuid::parse_str(id(&shared)).unwrap()).fetch_one(&pool).await.unwrap();
    assert!(
        event["recipients"]
            .as_array()
            .unwrap()
            .contains(&json!(a.member_id))
    );
    let hp = format!("/v1/lists/{}/entries", id(&hidden));
    for field in ["after_id", "before_id"] {
        let mut body = json!({"label":"bad"});
        body[field] = json!(Uuid::now_v7());
        let r = post(s, &a.device, &hp, body).await;
        problem(&r, 422, "validation-failed");
        assert_eq!(r.json()["errors"][0]["field"], field);
        assert_eq!(r.json()["errors"][0]["code"], "invalid-reference");
    }
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn shared_snapshot_and_search_include_lists_with_acl_and_source_keysets(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let s = &f.server;
    let a = &f.users[0].device;
    let b = &f.users[1].device;
    let item = ok(post(s, a, "/v1/items", json!({"title":"integrationneedle item"})).await);
    let hidden_item = ok(post(
        s,
        a,
        "/v1/items",
        json!({"title":"integrationneedle secret","visibility":"private"}),
    )
    .await);
    let public = ok(post(
        s,
        a,
        "/v1/lists",
        json!({"name":"integrationneedle public"}),
    )
    .await);
    let entry_only = ok(post(s, a, "/v1/lists", json!({"name":"entry-only"})).await);
    let entry = ok(post(
        s,
        a,
        &format!("/v1/lists/{}/entries", id(&entry_only)),
        json!({"label":"integrationneedle label","notes":"noteneedle"}),
    )
    .await);
    let hidden = ok(post(
        s,
        a,
        "/v1/lists",
        json!({"name":"integrationneedle private","visibility":"private"}),
    )
    .await);
    let selected = ok(post(s, a, "/v1/lists", json!({"name":"integrationneedle selected","visibility":"selected","viewers":[f.users[1].member_id]})).await);
    let attached = ok(post(
        s,
        a,
        "/v1/lists",
        json!({"name":"integrationneedle attached","item_id":item["id"]}),
    )
    .await);
    let hidden_attached = ok(post(
        s,
        a,
        "/v1/lists",
        json!({"name":"integrationneedle hidden-attached","item_id":hidden_item["id"]}),
    )
    .await);
    let checked_entry = ok(post(
        s,
        a,
        &format!("/v1/lists/{}/entries", id(&public)),
        json!({"label":"checked"}),
    )
    .await);
    ok(write(
        s,
        b,
        "POST",
        &format!("/v1/list-entries/{}/check", id(&checked_entry)),
        json!({}),
        &checked_entry["version"],
    )
    .await);
    let doc = serde_json::to_value(porchlight_api::openapi::document()).unwrap();
    let snapshot = ok(get(s, b, "/v1/snapshot").await);
    assert_schema(&doc, "/v1/snapshot", &snapshot);
    let admin_snapshot = ok(get(s, &f.admin.device, "/v1/snapshot").await);
    for absent in [&hidden, &hidden_attached, &selected] {
        assert!(
            !admin_snapshot["lists"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["id"] == absent["id"]),
            "app super admin follows shared visibility"
        );
    }
    let admin_search = ok(get(s, &f.admin.device, "/v1/search?q=integrationneedle").await);
    assert_schema(&doc, "/v1/search", &admin_search);
    assert_eq!(admin_search["items"].as_array().unwrap().len(), 4);
    for absent in [&hidden, &hidden_attached, &selected, &hidden_item] {
        assert!(
            !admin_search["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["id"] == absent["id"]),
            "app super admin search follows visibility"
        );
    }
    let agent_snapshot = ok(agent_request(
        &s.agent_socket,
        Some(s.bootstrap_token()),
        a.request("GET", "/v1/snapshot", Value::Null, None),
    )
    .await);
    assert_schema(&doc, "/v1/snapshot", &agent_snapshot);
    assert_eq!(agent_snapshot["lists"].as_array().unwrap().len(), 6);
    let agent_search = ok(agent_request(
        &s.agent_socket,
        Some(s.bootstrap_token()),
        a.request("GET", "/v1/search?q=integrationneedle", Value::Null, None),
    )
    .await);
    assert_schema(&doc, "/v1/search", &agent_search);
    assert_eq!(agent_search["items"].as_array().unwrap().len(), 8);
    let visible = snapshot["lists"].as_array().expect("shared snapshot lists");
    for expected in [&public, &entry_only, &selected, &attached] {
        assert!(visible.iter().any(|v| v["id"] == expected["id"]));
    }
    for absent in [&hidden, &hidden_attached] {
        assert!(!visible.iter().any(|v| v["id"] == absent["id"]));
    }
    let mut found = vec![];
    let mut path = "/v1/search?q=integrationneedle&limit=1".to_owned();
    loop {
        let page = ok(get(s, b, &path).await);
        assert_schema(&doc, "/v1/search", &page);
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        let v = &page["items"][0];
        found.push((
            v["entity_type"].as_str().unwrap().to_owned(),
            v["id"].as_str().unwrap().to_owned(),
        ));
        if let Some(c) = page["next_cursor"].as_str() {
            problem(
                &get(
                    s,
                    a,
                    &format!("/v1/search?q=integrationneedle&limit=1&cursor={c}"),
                )
                .await,
                400,
                "invalid-request",
            );
            problem(
                &get(s, b, &format!("/v1/search?q=different&limit=1&cursor={c}")).await,
                400,
                "invalid-request",
            );
            path = format!("/v1/search?q=integrationneedle&limit=1&cursor={c}");
        } else {
            break;
        }
    }
    assert_eq!(found.len(), 5);
    assert!(found.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(found[0], ("item".into(), id(&item).into()));
    let notes = ok(get(s, b, "/v1/search?q=noteneedle").await);
    assert_eq!(notes["items"][0]["id"], entry_only["id"]);
    ok(write(
        s,
        b,
        "DELETE",
        &format!("/v1/list-entries/{}", id(&entry)),
        json!({}),
        &entry["version"],
    )
    .await);
    assert!(
        ok(get(s, b, "/v1/search?q=noteneedle").await)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    ok(write(
        s,
        a,
        "PUT",
        &format!("/v1/lists/{}/viewers", id(&selected)),
        json!({"viewers":[]}),
        &selected["version"],
    )
    .await);
    assert!(
        !ok(get(s, b, "/v1/snapshot").await)["lists"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == selected["id"])
    );
    assert!(
        !ok(get(s, b, "/v1/search?q=integrationneedle").await)["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == selected["id"])
    );
    let notifications:i64=sqlx::query_scalar("SELECT count(*) FROM porchlight.notification_events WHERE entity_type IN ('list','list_entry')").fetch_one(&pool).await.unwrap();
    assert_eq!(
        notifications, 0,
        "S7a registers no business notification kind"
    );
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn search_skips_list_rows_rejected_during_decoration(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let s = &f.server;
    let public = ok(post(
        s,
        &f.users[0].device,
        "/v1/lists",
        json!({"name":"skippableneedle public"}),
    )
    .await);
    let hidden = ok(post(
        s,
        &f.users[0].device,
        "/v1/lists",
        json!({"name":"skippableneedle hidden","visibility":"private"}),
    )
    .await);
    // RR prevents real mid-page visibility races. Force SQL/Rust disagreement in
    // this isolated test database to exercise the defensive decoration branch.
    sqlx::query("CREATE OR REPLACE FUNCTION porchlight.pl_list_can_read(p_list uuid,p_member uuid) RETURNS boolean LANGUAGE sql STABLE AS 'SELECT true'")
        .execute(&pool).await.unwrap();
    let page = ok(get(s, &f.users[1].device, "/v1/search?q=skippableneedle").await);
    let rows = page["items"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], public["id"]);
    assert!(!rows.iter().any(|row| row["id"] == hidden["id"]));
    f.server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn orphan_filter_never_exposes_another_owners_list(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let s = &f.server;
    let a = &f.users[0].device;
    let b = &f.users[1].device;
    let item = ok(post(s, a, "/v1/items", json!({"title":"parent"})).await);
    let l = ok(post(
        s,
        a,
        "/v1/lists",
        json!({"name":"orphan","item_id":item["id"]}),
    )
    .await);
    ok(write(
        s,
        a,
        "DELETE",
        &format!("/v1/items/{}", id(&item)),
        json!({}),
        &item["version"],
    )
    .await);
    for (d, expected) in [(a, true), (b, false), (&f.admin.device, true)] {
        let page = ok(get(s, d, "/v1/lists?include_orphaned=true").await);
        assert_eq!(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["id"] == l["id"]),
            expected
        );
    }
    // PAT owners must be super admins; scope remains independently read/write.
    sqlx::query("UPDATE porchlight.household_members SET role='super_admin' WHERE id=$1")
        .bind(f.users[0].member_id)
        .execute(&pool)
        .await
        .unwrap();
    for scope in ["read", "write"] {
        let (token_id, pat) = porchlight_api::testing::trust::seed_pat(&pool, s, &[scope]).await;
        sqlx::query("UPDATE porchlight.api_tokens SET owner_member_id=$2 WHERE id=$1")
            .bind(token_id)
            .bind(f.users[0].member_id)
            .execute(&pool)
            .await
            .unwrap();
        let page = ok(agent_request(
            &s.agent_socket,
            Some(&pat),
            a.request("GET", "/v1/lists?include_orphaned=true", Value::Null, None),
        )
        .await);
        let rows = page["items"].as_array().unwrap();
        assert_eq!(
            rows.len(),
            1,
            "owner cleanup discovery does not require admin scope"
        );
        assert_eq!(rows[0]["id"], l["id"]);
        assert_eq!(rows[0]["capabilities"], json!(["view", "delete"]));
    }
    let page = ok(get(s, a, "/v1/lists").await);
    assert!(page["items"].as_array().unwrap().is_empty());
    problem(
        &get(s, b, &format!("/v1/lists/{}", id(&l))).await,
        404,
        "not-found",
    );
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn api_lineage_cycles_are_bounded_in_both_directions(pool: PgPool) {
    let (s, d) = setup(&pool).await;
    let mut items = vec![];
    for title in ["first", "second", "unrelated"] {
        items.push(ok(post(&s,&d,"/v1/items",json!({"title":title,"kind":"deadline","time_mode":"fixed","due_at":"2026-10-05T12:00:00Z","tzid":"UTC","rrule":"FREQ=DAILY;COUNT=3","recurrence_mode":"calendar"})).await));
    }
    for (child, parent) in [(&items[0], &items[1]), (&items[1], &items[0])] {
        sqlx::query(
            "UPDATE porchlight.items SET split_from_item_id=$2,split_from_seq=1 WHERE id=$1",
        )
        .bind(Uuid::parse_str(id(child)).unwrap())
        .bind(Uuid::parse_str(id(parent)).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    }
    let l = ok(post(
        &s,
        &d,
        "/v1/lists",
        json!({"name":"cycle","mode":"per_occurrence","item_id":items[0]["id"]}),
    )
    .await);
    let unrelated_os = ok(get(&s, &d, &format!("/v1/items/{}/occurrences", id(&items[2]))).await);
    let c = Caller::Agent {
        token_id: Uuid::nil(),
        household_id: s.init.household_id,
        member_id: s.init.admin_member_id,
        scopes: vec!["admin".into()],
        label: "cycle".into(),
        request_id: Uuid::now_v7(),
    };
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("SET statement_timeout='500ms'")
        .execute(&mut *conn)
        .await
        .unwrap();
    let page = lists::collection(
        &s.state,
        &mut conn,
        &c,
        porchlight_api::list_types::ListsQuery {
            item_id: Some(Uuid::parse_str(id(&items[1])).unwrap()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].fields.id.to_string(), id(&l));
    let result = lists::response(
        &mut conn,
        &c,
        Uuid::parse_str(id(&l)).unwrap(),
        Some(Uuid::parse_str(unrelated_os["items"][0]["id"].as_str().unwrap()).unwrap()),
        false,
    )
    .await;
    assert!(
        matches!(result,Err(e) if e.problem()==porchlight_api::error::Problem::ValidationFailed)
    );
    drop(conn);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn list_snapshot_uses_the_same_mvcc_view_as_its_stream_cursor(pool: PgPool) {
    use tower::ServiceExt;
    let (s, d) = setup(&pool).await;
    let before = ok(post(&s, &d, "/v1/lists", json!({"name":"before"})).await);
    porchlight_db::outbox::assign_stream_seqs(&s.state.pool, 1000)
        .await
        .unwrap();
    let router = porchlight_api::routes::mtls_router(s.state.clone()).layer(axum::Extension(
        porchlight_api::server::ConnInfo {
            transport: porchlight_api::server::Transport::Lan,
            client_addr: None,
            client_cert_fingerprint: Some(d.fingerprint),
        },
    ));
    let mut writer = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE porchlight.items IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *writer)
        .await
        .unwrap();
    let request = d.request("GET", "/v1/snapshot", json!({}), None);
    let task = tokio::spawn(async move { router.oneshot(request).await.unwrap() });
    tokio::time::timeout(std::time::Duration::from_secs(3),async {loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation='porchlight.items'::regclass AND NOT granted AND mode='AccessShareLock')").fetch_one(&pool).await.unwrap();
        if waiting {break;} tokio::task::yield_now().await;
    }}).await.expect("snapshot did not establish MVCC cursor before item query");
    let after:Uuid=sqlx::query_scalar("INSERT INTO porchlight.lists(household_id,owner_id,name) VALUES($1,$2,'after') RETURNING id").bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&mut *writer).await.unwrap();
    writer.commit().await.unwrap();
    let response = task.await.unwrap();
    assert_eq!(response.status(), 200);
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    let snapshot: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(snapshot["lists"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["lists"][0]["id"], before["id"]);
    let cursor = snapshot["cursor"].as_i64().unwrap();
    porchlight_db::outbox::assign_stream_seqs(&s.state.pool, 1000)
        .await
        .unwrap();
    let seq:i64=sqlx::query_scalar("SELECT stream_seq FROM porchlight.outbox WHERE entity_id=$1 AND kind='list.changed' ORDER BY stream_seq DESC LIMIT 1").bind(after).fetch_one(&pool).await.unwrap();
    assert!(
        seq > cursor,
        "concurrent list write must publish strictly after snapshot cursor"
    );
    s.stop().await;
}

// Resolve flattened DTO schemas before checking every runtime field. Unlike the S4b-only
// conformance helper this supports heterogeneous sources and nested flatten/allOf shapes.
fn flattened_schema(doc: &Value, schema: &Value) -> Value {
    if let Some(reference) = schema["$ref"].as_str() {
        return flattened_schema(
            doc,
            doc.pointer(reference.strip_prefix('#').unwrap()).unwrap(),
        );
    }
    if let Some(parts) = schema["allOf"].as_array() {
        let mut properties = serde_json::Map::new();
        let mut required = vec![];
        for part in parts {
            let part = flattened_schema(doc, part);
            if let Some(fields) = part["properties"].as_object() {
                properties.extend(fields.clone());
            }
            if let Some(keys) = part["required"].as_array() {
                required.extend(keys.clone());
            }
        }
        return json!({"type":"object","properties":properties,"required":required});
    }
    schema.clone()
}
fn matches_schema(doc: &Value, schema: &Value, value: &Value) -> bool {
    let schema = flattened_schema(doc, schema);
    if let Some(variants) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
    {
        return variants.iter().any(|s| matches_schema(doc, s, value));
    }
    let types = schema["type"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_else(|| schema["type"].as_str().into_iter().collect());
    if value.is_null() {
        return types.contains(&"null");
    }
    if let Some(variants) = schema["enum"].as_array()
        && !variants.contains(value)
    {
        return false;
    }
    if types.contains(&"object") {
        let Some(object) = value.as_object() else {
            return false;
        };
        if schema["required"].as_array().is_some_and(|keys| {
            keys.iter()
                .any(|k| !object.contains_key(k.as_str().unwrap()))
        }) {
            return false;
        }
        return object.iter().all(|(key, value)| {
            schema["properties"]
                .get(key)
                .is_some_and(|s| matches_schema(doc, s, value))
        });
    }
    if types.contains(&"array") {
        return value
            .as_array()
            .is_some_and(|a| a.iter().all(|v| matches_schema(doc, &schema["items"], v)));
    }
    if types.contains(&"integer") {
        return value.is_i64() || value.is_u64();
    }
    if types.contains(&"number") {
        return value.is_number();
    }
    if types.contains(&"string") {
        return value.is_string();
    }
    if types.contains(&"boolean") {
        return value.is_boolean();
    }
    false
}
fn assert_schema(doc: &Value, path: &str, value: &Value) {
    let schema =
        &doc["paths"][path]["get"]["responses"]["200"]["content"]["application/json"]["schema"];
    assert!(
        matches_schema(doc, schema, value),
        "response does not conform to {path}"
    );
    let mut malformed = value.clone();
    malformed["undocumented"] = json!(true);
    assert!(
        !matches_schema(doc, schema, &malformed),
        "validator must reject extra fields"
    );
}
