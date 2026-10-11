use crate::common;
use common::*;
use porchlight_api::testing::{TestClient, TestDevice, TestOptions, TestResponse, TestServer};
use porchlight_core::auth::session::Platform;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

async fn write(
    s: &TestServer,
    d: &TestDevice,
    method: &str,
    path: &str,
    body: Value,
    version: &Value,
) -> TestResponse {
    let mut req = d.request(method, path, body, None);
    req.headers_mut()
        .insert("if-match", version.to_string().parse().unwrap());
    TestClient::new(&s.ca_pem, Some(d))
        .request(s.lan, req)
        .await
        .unwrap()
}
fn id(v: &Value) -> &str {
    v["id"].as_str().unwrap()
}
fn ok(r: TestResponse) -> Value {
    assert!(r.status < 300, "status {}", r.status);
    r.json()
}
async fn setup(pool: &PgPool) -> (TestServer, TestDevice) {
    let s = TestServer::start_with(
        pool,
        TestOptions {
            db_pool_size: 1,
            background_derivation: false,
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
async fn persistent_got_it_clear_versions_replay_and_soft_delete(pool: PgPool) {
    let (s, d) = setup(&pool).await;
    let created = send(
        &s,
        &d,
        "POST",
        "/v1/lists",
        json!({"name":"Groceries"}),
        Some("create-list"),
    )
    .await;
    assert_eq!(created.status, 201);
    let replay = send(
        &s,
        &d,
        "POST",
        "/v1/lists",
        json!({"name":"Groceries"}),
        Some("create-list"),
    )
    .await;
    assert_eq!(replay.body, created.body);
    let list = created.json();
    let path = format!("/v1/lists/{}", id(&list));
    let entry = ok(post(
        &s,
        &d,
        &format!("{path}/entries"),
        json!({"label":"Oat milk","qty":2,"unit":"cartons"}),
    )
    .await);
    let ep = format!("/v1/list-entries/{}", id(&entry));
    let checked = ok(write(
        &s,
        &d,
        "POST",
        &format!("{ep}/check"),
        json!({}),
        &entry["version"],
    )
    .await);
    assert_eq!(checked["checked_by"], s.init.admin_member_id.to_string());
    let got = ok(get(&s, &d, &path).await);
    assert_eq!(got["got_it"][0]["id"], entry["id"]);
    assert!(got["entries"].as_array().unwrap().is_empty());
    problem(
        &write(
            &s,
            &d,
            "PATCH",
            &ep,
            json!({"label":"stale"}),
            &entry["version"],
        )
        .await,
        409,
        "version-conflict",
    );
    problem(
        &send(
            &s,
            &d,
            "PATCH",
            &ep,
            json!({"label":"missing"}),
            Some("missing"),
        )
        .await,
        428,
        "if-match-required",
    );
    let cleared = ok(write(
        &s,
        &d,
        "POST",
        &format!("{path}/clear-checked"),
        json!({}),
        &got["version"],
    )
    .await);
    assert!(cleared["got_it"].as_array().unwrap().is_empty());
    let archived: bool = sqlx::query_scalar(
        "SELECT archived_at IS NOT NULL FROM porchlight.list_entries WHERE id=$1",
    )
    .bind(Uuid::parse_str(id(&entry)).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(archived);
    ok(write(&s, &d, "DELETE", &path, json!({}), &cleared["version"]).await);
    problem(&get(&s, &d, &path).await, 404, "not-found");
    assert!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM porchlight.list_revisions")
            .fetch_one(&pool)
            .await
            .unwrap()
            >= 3
    );
    assert!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM porchlight.outbox WHERE kind='list_entry.changed' AND payload ? 'list_id'").fetch_one(&pool).await.unwrap()>0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn occurrence_checks_are_fresh_and_validate_parent(pool: PgPool) {
    let (s, d) = setup(&pool).await;
    let item = ok(post(&s,&d,"/v1/items",json!({"title":"Meds","kind":"deadline","time_mode":"fixed","due_at":"2026-10-05T12:00:00Z","tzid":"UTC","rrule":"FREQ=DAILY;COUNT=3","recurrence_mode":"calendar"})).await);
    let occurrences = ok(get(&s, &d, &format!("/v1/items/{}/occurrences", id(&item))).await);
    let a = &occurrences["items"][0]["id"];
    let b = &occurrences["items"][1]["id"];
    let list = ok(post(
        &s,
        &d,
        "/v1/lists",
        json!({"name":"Refill","mode":"per_occurrence","item_id":item["id"]}),
    )
    .await);
    let lp = format!("/v1/lists/{}", id(&list));
    let entry = ok(post(&s, &d, &format!("{lp}/entries"), json!({"label":"Order"})).await);
    let ep = format!("/v1/list-entries/{}", id(&entry));
    problem(
        &write(
            &s,
            &d,
            "POST",
            &format!("{ep}/check"),
            json!({}),
            &entry["version"],
        )
        .await,
        422,
        "validation-failed",
    );
    let check = ok(write(
        &s,
        &d,
        "POST",
        &format!("{ep}/check"),
        json!({"occurrence_id":a}),
        &entry["version"],
    )
    .await);
    let first = ok(get(
        &s,
        &d,
        &format!("{lp}?occurrence_id={}", a.as_str().unwrap()),
    )
    .await);
    assert!(first["entries"][0]["checked_at"].is_string());
    let second = ok(get(
        &s,
        &d,
        &format!("{lp}?occurrence_id={}", b.as_str().unwrap()),
    )
    .await);
    assert!(second["entries"][0]["checked_at"].is_null());
    problem(
        &write(
            &s,
            &d,
            "POST",
            &format!("{ep}/check"),
            json!({"occurrence_id":Uuid::now_v7()}),
            &check["version"],
        )
        .await,
        422,
        "validation-failed",
    );
    let unchecked = ok(write(
        &s,
        &d,
        "POST",
        &format!("{ep}/uncheck"),
        json!({"occurrence_id":a}),
        &check["version"],
    )
    .await);
    assert!(unchecked["checked_at"].is_null());
    problem(
        &post(
            &s,
            &d,
            "/v1/lists",
            json!({"name":"invalid","mode":"per_occurrence"}),
        )
        .await,
        422,
        "validation-failed",
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn fractional_moves_and_signed_list_pages(pool: PgPool) {
    let (s, d) = setup(&pool).await;
    let list = ok(post(&s, &d, "/v1/lists", json!({"name":"Order"})).await);
    let lp = format!("/v1/lists/{}", id(&list));
    let mut entries = Vec::new();
    for label in ["A", "B", "C"] {
        entries.push(ok(post(
            &s,
            &d,
            &format!("{lp}/entries"),
            json!({"label":label}),
        )
        .await));
    }
    let mover = &entries[2];
    ok(write(
        &s,
        &d,
        "PATCH",
        &format!("/v1/list-entries/{}", id(mover)),
        json!({"after_id":entries[0]["id"],"before_id":entries[1]["id"]}),
        &mover["version"],
    )
    .await);
    let shown = ok(get(&s, &d, &lp).await);
    assert_eq!(shown["entries"][1]["label"], "C");
    problem(
        &write(
            &s,
            &d,
            "PATCH",
            &format!("/v1/list-entries/{}", id(&entries[0])),
            json!({"before_id":Uuid::now_v7()}),
            &entries[0]["version"],
        )
        .await,
        422,
        "validation-failed",
    );
    ok(post(&s, &d, "/v1/lists", json!({"name":"second"})).await);
    let page = ok(get(&s, &d, "/v1/lists?limit=1").await);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let cursor = page["next_cursor"].as_str().unwrap();
    let next = ok(get(&s, &d, &format!("/v1/lists?limit=1&cursor={cursor}")).await);
    assert_ne!(page["items"][0]["id"], next["items"][0]["id"]);
    problem(
        &get(&s, &d, "/v1/lists?cursor=broken").await,
        400,
        "invalid-request",
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn list_contributions_acl_eviction_and_parent_visibility_changes(pool: PgPool) {
    use porchlight_api::{
        extract::Caller,
        lists,
        testing::{HouseholdFixture, TestOptions},
    };
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            db_pool_size: 1,
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let s = &f.server;
    let a = &f.users[0];
    let b = &f.users[1];
    let c = Caller::Member {
        member_id: b.member_id,
        user_id: b.user_id,
        role: "user".into(),
        device_id: b.device.device_id,
        session_id: Uuid::nil(),
        household_id: s.init.household_id,
        client_addr: None,
        request_id: Uuid::now_v7(),
    };
    let list = ok(post(
        s,
        &a.device,
        "/v1/lists",
        json!({"name":"Saffron groceries","visibility":"selected","viewers":[b.member_id]}),
    )
    .await);
    let lp = format!("/v1/lists/{}", id(&list));
    ok(post(
        s,
        &a.device,
        &format!("{lp}/entries"),
        json!({"label":"Turmeric"}),
    )
    .await);
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(
        lists::search_contribution(&mut conn, &c, "Turmeric")
            .await
            .unwrap(),
        [Uuid::parse_str(id(&list)).unwrap()]
    );
    assert_eq!(
        lists::snapshot_contribution(&mut conn, &c)
            .await
            .unwrap()
            .len(),
        1
    );
    drop(conn);
    let shown = ok(get(s, &a.device, &lp).await);
    ok(write(
        s,
        &a.device,
        "PUT",
        &format!("{lp}/viewers"),
        json!({"viewers":[]}),
        &shown["version"],
    )
    .await);
    problem(&get(s, &b.device, &lp).await, 404, "not-found");
    let mut conn = pool.acquire().await.unwrap();
    assert!(
        lists::search_contribution(&mut conn, &c, "Saffron")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        lists::snapshot_contribution(&mut conn, &c)
            .await
            .unwrap()
            .is_empty()
    );
    drop(conn);
    let item = ok(post(
        s,
        &a.device,
        "/v1/items",
        json!({"title":"Inherited","visibility":"selected","viewers":[b.member_id]}),
    )
    .await);
    let attached = ok(post(
        s,
        &a.device,
        "/v1/lists",
        json!({"name":"Attached saffron","item_id":item["id"]}),
    )
    .await);
    let ap = format!("/v1/lists/{}", id(&attached));
    let entry = ok(post(
        s,
        &a.device,
        &format!("{ap}/entries"),
        json!({"label":"Spice"}),
    )
    .await);
    let ep = format!("/v1/list-entries/{}/check", id(&entry));
    problem(
        &write(s, &b.device, "POST", &ep, json!({}), &entry["version"]).await,
        403,
        "forbidden",
    );
    problem(&get(s, &f.admin.device, &ap).await, 404, "not-found"); // super admins follow visibility
    let ip = format!("/v1/items/{}", id(&item));
    let item = ok(write(
        s,
        &a.device,
        "PUT",
        &format!("{ip}/assignees"),
        json!({"assignees":[b.member_id]}),
        &item["version"],
    )
    .await);
    let checked = ok(write(s, &b.device, "POST", &ep, json!({}), &entry["version"]).await);
    assert!(checked["checked_at"].is_string());
    let item = ok(write(
        s,
        &a.device,
        "PUT",
        &format!("{ip}/assignees"),
        json!({"assignees":[]}),
        &item["version"],
    )
    .await);
    ok(write(
        s,
        &a.device,
        "PUT",
        &format!("{ip}/viewers"),
        json!({"viewers":[]}),
        &item["version"],
    )
    .await);
    problem(&get(s, &b.device, &ap).await, 404, "not-found");
    let mut conn = pool.acquire().await.unwrap();
    assert!(
        lists::search_contribution(&mut conn, &c, "saffron")
            .await
            .unwrap()
            .is_empty()
    );
    drop(conn);
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn occurrence_conflict_current_includes_check_and_invalid_body_rolls_back(pool: PgPool) {
    let (s, d) = setup(&pool).await;
    let item = ok(post(&s, &d, "/v1/items", json!({"title":"Refill","kind":"deadline","time_mode":"fixed","due_at":"2026-10-05T12:00:00Z","tzid":"UTC","rrule":"FREQ=DAILY;COUNT=3","recurrence_mode":"calendar"})).await);
    let occs = ok(get(&s, &d, &format!("/v1/items/{}/occurrences", id(&item))).await);
    let occ = &occs["items"][0]["id"];
    let list = ok(post(
        &s,
        &d,
        "/v1/lists",
        json!({"name":"Per dose","mode":"per_occurrence","item_id":item["id"]}),
    )
    .await);
    let lp = format!("/v1/lists/{}", id(&list));
    let entry = ok(post(&s, &d, &format!("{lp}/entries"), json!({"label":"Refill"})).await);
    let ep = format!("/v1/list-entries/{}", id(&entry));
    let checked = ok(write(
        &s,
        &d,
        "POST",
        &format!("{ep}/check"),
        json!({"occurrence_id":occ}),
        &entry["version"],
    )
    .await);
    let stale = write(
        &s,
        &d,
        "POST",
        &format!("{ep}/uncheck"),
        json!({"occurrence_id":occ}),
        &entry["version"],
    )
    .await;
    problem(&stale, 409, "version-conflict");
    assert!(stale.json()["current"]["checked_at"].is_string());
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.outbox")
        .fetch_one(&pool)
        .await
        .unwrap();
    problem(
        &write(
            &s,
            &d,
            "PATCH",
            &ep,
            json!({"label":"bad","qty":-1}),
            &checked["version"],
        )
        .await,
        422,
        "validation-failed",
    );
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.outbox")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, after);
    let shown = ok(get(&s, &d, &lp).await);
    assert_eq!(shown["entries"][0]["label"], "Refill");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn visible_admin_can_remove_own_selected_access(pool: PgPool) {
    use porchlight_api::testing::HouseholdFixture;
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            db_pool_size: 1,
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let list = ok(post(
        &f.server,
        &f.users[0].device,
        "/v1/lists",
        json!({"name":"Selected list","visibility":"selected","viewers":[f.admin.member_id]}),
    )
    .await);
    let path = format!("/v1/lists/{}", id(&list));
    let r = write(
        &f.server,
        &f.admin.device,
        "PUT",
        &format!("{path}/viewers"),
        json!({"viewers":[]}),
        &list["version"],
    )
    .await;
    assert_eq!(
        r.status, 204,
        "removing the writer's access must commit rather than roll back"
    );
    problem(
        &get(&f.server, &f.admin.device, &path).await,
        404,
        "not-found",
    );
    let shown = ok(get(&f.server, &f.users[0].device, &path).await);
    assert_eq!(shown["viewers"], json!([]));
    f.server.stop().await;
}
