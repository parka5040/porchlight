use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use porchlight_api::{
    routes,
    testing::{TestOptions, TestServer},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn call(
    s: &TestServer,
    method: &str,
    path: &str,
    value: Value,
    version: Option<i64>,
) -> (u16, Value) {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("porchlight-client-version", porchlight_core::VERSION)
        .header("authorization", format!("Bearer {}", s.bootstrap_token()))
        .header("content-type", "application/json")
        .header("idempotency-key", Uuid::now_v7().to_string());
    if let Some(v) = version {
        b = b.header("if-match", v.to_string());
    }
    let r = routes::agent_router(s.state.clone())
        .layer(axum::Extension(porchlight_api::server::ConnInfo {
            transport: porchlight_api::server::Transport::Agent,
            client_addr: None,
            client_cert_fingerprint: None,
        }))
        .oneshot(b.body(Body::from(value.to_string())).unwrap())
        .await
        .unwrap();
    let status = r.status().as_u16();
    let bytes = to_bytes(r.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}
#[sqlx::test(migrations = "../db/migrations")]
async fn admin_users_revert_purge_and_retry(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            db_pool_size: 1,
            ..Default::default()
        },
    )
    .await;
    let (code, m) = call(
        &s,
        "POST",
        "/v1/admin/users",
        json!({"display_name":"Kid","role":"dependent"}),
        None,
    )
    .await;
    assert_eq!(code, 201);
    let household_zone: String =
        sqlx::query_scalar("select tzid from porchlight.households where id=$1")
            .bind(s.init.household_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(m["tzid"], household_zone);
    let member = m["id"].as_str().unwrap();
    let (code, i) = call(
        &s,
        "POST",
        "/v1/items",
        json!({"title":"Original","assignees":[member],"viewers":[member],"reminder_offsets":[30]}),
        None,
    )
    .await;
    assert_eq!(code, 201);
    let id = i["id"].as_str().unwrap();
    let path = format!("/v1/items/{id}");
    let (code, revisions) = call(
        &s,
        "GET",
        &format!("/v1/admin/items/{id}/revisions"),
        json!({}),
        None,
    )
    .await;
    assert_eq!(code, 200);
    let document = serde_json::to_value(porchlight_api::openapi::document()).unwrap();
    let schemas = &document["components"]["schemas"];
    let snapshot = &revisions["items"].as_array().unwrap().last().unwrap()["snapshot"];
    let keys = |v: &Value| {
        v.as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(
        keys(snapshot),
        keys(&schemas["RevisionSnapshot"]["properties"])
    );
    let actor = &revisions["items"].as_array().unwrap().last().unwrap()["actor"];
    assert!(keys(actor).is_subset(&keys(&schemas["RevisionActor"]["properties"])));
    let revision: Uuid = sqlx::query_scalar(
        "select id from porchlight.item_revisions where item_id=$1 order by rev desc limit 1",
    )
    .bind(Uuid::parse_str(id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    let (_, changed) = call(
        &s,
        "PATCH",
        &path,
        json!({"title":"Changed"}),
        i["version"].as_i64(),
    )
    .await;
    let (_, changed) = call(
        &s,
        "PUT",
        &format!("{path}/assignees"),
        json!({"assignees":[]}),
        changed["version"].as_i64(),
    )
    .await;
    let (code, changed) = call(
        &s,
        "PUT",
        &format!("{path}/viewers"),
        json!({"viewers":[]}),
        changed["version"].as_i64(),
    )
    .await;
    assert_eq!(code, 200);
    let (code, restored) = call(
        &s,
        "POST",
        &format!("/v1/admin/items/{id}/revert"),
        json!({"revision_id":revision}),
        changed["version"].as_i64(),
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(restored["title"], "Original");
    assert_eq!(restored["assignees"], json!([member]));
    assert_eq!(restored["viewers"], json!([member]));
    let actor: Value = sqlx::query_scalar(
        "select actor from porchlight.item_revisions where item_id=$1 order by rev desc limit 1",
    )
    .bind(Uuid::parse_str(id).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    let pat_id: Uuid =
        sqlx::query_scalar("select id from porchlight.api_tokens where name='bootstrap'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(actor["token_id"], json!(pat_id));
    assert!(
        actor["application_name"]
            .as_str()
            .unwrap()
            .starts_with("agent:")
    );
    assert_eq!(restored["reminder_offsets"], json!([30]));
    assert_eq!(restored["pending_derivation"], false);
    let (code, _) = call(
        &s,
        "POST",
        "/v1/admin/purge",
        json!({"item_ids":[id]}),
        None,
    )
    .await;
    assert_eq!(code, 422);
    let (code, deleted) = call(&s, "DELETE", &path, json!({}), restored["version"].as_i64()).await;
    assert_eq!(code, 200);
    let mut probe = pool.begin().await.unwrap();
    sqlx::query("set local role porchlight_app")
        .execute(&mut *probe)
        .await
        .unwrap();
    sqlx::query("select porchlight.pl_purge_items($1,$2)")
        .bind(s.init.household_id)
        .bind(vec![Uuid::parse_str(id).unwrap()])
        .execute(&mut *probe)
        .await
        .expect("purge definer");
    probe.rollback().await.unwrap();
    let (code, p) = call(
        &s,
        "POST",
        "/v1/admin/purge",
        json!({"item_ids":[id]}),
        None,
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(p["counts"]["items"], 1);
    assert_eq!(
        keys(&p["counts"]),
        keys(&schemas["PurgeCounts"]["properties"])
    );
    let count: i64 = sqlx::query_scalar("select count(*) from porchlight.purge_log")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert!(!deleted.is_null());
    let (code, _) = call(&s, "GET", "/v1/admin/derive", json!({}), None).await;
    assert_eq!(code, 200);
    let (code, _) = call(
        &s,
        "POST",
        &format!("/v1/admin/derive/{id}/retry"),
        json!({}),
        None,
    )
    .await;
    assert_eq!(code, 404);
    let (code, _) = call(
        &s,
        "DELETE",
        &format!("/v1/admin/users/{member}"),
        json!({}),
        m["version"].as_i64(),
    )
    .await;
    assert_eq!(code, 200);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn user_delete_revokes_authority_devices_pat_and_keeps_items(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let(code,m)=call(&s,"POST","/v1/admin/users",json!({"display_name":"Second","role":"super_admin","username":"second","initial_password":"second safe password","tzid":"UTC"}),None).await;
    assert_eq!(code, 201);
    let id = m["id"].as_str().unwrap();
    let member = Uuid::parse_str(id).unwrap();
    let d = s
        .enroll(
            "second",
            "second safe password",
            porchlight_core::auth::session::Platform::Desktop,
        )
        .await;
    sqlx::query("update porchlight.household_members set tz_authority_device_id=$2 where id=$1")
        .bind(member)
        .bind(d.device_id)
        .execute(&pool)
        .await
        .unwrap();
    let version: i32 =
        sqlx::query_scalar("select version from porchlight.household_members where id=$1")
            .bind(member)
            .fetch_one(&pool)
            .await
            .unwrap();
    let token =
        porchlight_core::auth::token::Token::generate(porchlight_core::auth::token::TokenKind::Pat);
    sqlx::query("insert into porchlight.api_tokens(household_id,owner_member_id,name,token_hash,scopes,expires_at) values($1,$2,'second PAT',$3,array['admin'],now()+interval '1 day')").bind(s.init.household_id).bind(member).bind(token.hash().as_slice()).execute(&pool).await.unwrap();
    let (_, item) = call(
        &s,
        "POST",
        "/v1/items",
        json!({"title":"Must stay","owner_id":member}),
        None,
    )
    .await;
    let (code, _) = call(
        &s,
        "DELETE",
        &format!("/v1/admin/users/{id}"),
        json!({}),
        Some(version.into()),
    )
    .await;
    assert_eq!(code, 200);
    let revoked: bool =
        sqlx::query_scalar("select revoked_at is not null from porchlight.devices where id=$1")
            .bind(d.device_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(revoked);
    let authority: Option<Uuid> = sqlx::query_scalar(
        "select tz_authority_device_id from porchlight.household_members where id=$1",
    )
    .bind(member)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(authority.is_none());
    let request = Request::builder()
        .uri("/v1/admin/whoami")
        .header("porchlight-client-version", porchlight_core::VERSION)
        .header("authorization", format!("Bearer {}", token.expose()))
        .body(Body::empty())
        .unwrap();
    let response = routes::agent_router(s.state.clone())
        .layer(axum::Extension(porchlight_api::server::ConnInfo {
            transport: porchlight_api::server::Transport::Agent,
            client_addr: None,
            client_cert_fingerprint: None,
        }))
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    let pat = porchlight_db::tokens::authenticate(&pool, &token.hash())
        .await
        .unwrap();
    assert!(pat.is_none());
    let (code, _) = call(
        &s,
        "GET",
        &format!("/v1/items/{}", item["id"].as_str().unwrap()),
        json!({}),
        None,
    )
    .await;
    assert_eq!(code, 200);
    let client = porchlight_api::testing::TestClient::new(&s.ca_pem, Some(&d));
    // Post-commit publication is synchronous: do not wait for the polling projector.
    assert!(
        client
            .request(s.lan, d.request("GET", "/v1/connection", json!({}), None))
            .await
            .is_err()
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn admin_scopes_references_versions_and_retry(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let (_, i) = call(&s, "POST", "/v1/items", json!({"title":"Retry"}), None).await;
    let id = i["id"].as_str().unwrap();
    let uuid = Uuid::parse_str(id).unwrap();
    let (code, _) = call(
        &s,
        "POST",
        &format!("/v1/admin/items/{id}/revert"),
        json!({"revision_id":Uuid::now_v7()}),
        None,
    )
    .await;
    assert_eq!(code, 428);
    let (code, v) = call(
        &s,
        "POST",
        &format!("/v1/admin/items/{id}/revert"),
        json!({"revision_id":Uuid::now_v7()}),
        i["version"].as_i64(),
    )
    .await;
    assert_eq!(code, 422);
    assert_eq!(v["errors"][0]["code"], "invalid-reference");
    let (code, _) = call(
        &s,
        "PATCH",
        &format!("/v1/items/{id}"),
        json!({"title":"Changed retry"}),
        i["version"].as_i64(),
    )
    .await;
    assert_eq!(code, 200);
    let (code, v) = call(
        &s,
        "POST",
        &format!("/v1/admin/items/{id}/revert"),
        json!({"revision_id":Uuid::now_v7()}),
        Some(1),
    )
    .await;
    assert_eq!(code, 409);
    assert_eq!(v["current"]["id"], id);
    sqlx::query("update porchlight.item_derivation set requested_gen=applied_gen+1,attempts=5,last_error='invalid recurrence',retry_at=now()+interval '1 day' where item_id=$1").bind(uuid).execute(&pool).await.unwrap();
    let (code, v) = call(&s, "GET", "/v1/admin/derive", json!({}), None).await;
    assert_eq!(code, 200);
    assert_eq!(v["pending_count"], 1);
    assert_eq!(v["items"][0]["attempts"], 5);
    let (code, _) = call(
        &s,
        "POST",
        &format!("/v1/admin/derive/{id}/retry"),
        json!({}),
        None,
    )
    .await;
    assert_eq!(code, 200);
    let (attempts, error): (i32, Option<String>) = sqlx::query_as(
        "select attempts,last_error from porchlight.item_derivation where item_id=$1",
    )
    .bind(uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attempts, 0);
    assert!(error.is_none());
    sqlx::query("update porchlight.api_tokens set scopes=array['read'] where name='bootstrap'")
        .execute(&pool)
        .await
        .unwrap();
    for (method, path, b) in [
        (
            "POST",
            "/v1/admin/users",
            json!({"display_name":"Forbidden","role":"dependent"}),
        ),
        ("POST", "/v1/admin/purge", json!({"item_ids":[id]})),
        ("POST", &format!("/v1/admin/derive/{id}/retry"), json!({})),
    ] {
        let (code, _) = call(&s, method, path, b, None).await;
        assert_eq!(code, 403);
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn revert_undeletes_and_restores_completion_interval(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let(code,item)=call(&s,"POST","/v1/items",json!({"title":"Repeat","kind":"deadline","due_at":"2027-01-01T12:00:00Z","tzid":"UTC","recurrence_mode":"completion","completion_interval":{"months":1,"days":2,"microseconds":3600000000i64}}),None).await;
    assert_eq!(code, 201);
    let id = item["id"].as_str().unwrap();
    let uuid = Uuid::parse_str(id).unwrap();
    let rev: Uuid = sqlx::query_scalar(
        "select id from porchlight.item_revisions where item_id=$1 order by rev desc limit 1",
    )
    .bind(uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (code, deleted) = call(
        &s,
        "DELETE",
        &format!("/v1/items/{id}"),
        json!({}),
        item["version"].as_i64(),
    )
    .await;
    assert_eq!(code, 200);
    let (code, restored) = call(
        &s,
        "POST",
        &format!("/v1/admin/items/{id}/revert"),
        json!({"revision_id":rev}),
        deleted["version"].as_i64(),
    )
    .await;
    assert_eq!(code, 200);
    assert!(restored["deleted_at"].is_null());
    assert_eq!(restored["completion_interval"], item["completion_interval"]);
    assert_eq!(restored["pending_derivation"], false);
    let (_, other) = call(&s, "POST", "/v1/items", json!({"title":"Other"}), None).await;
    let (code, _) = call(
        &s,
        "POST",
        &format!("/v1/admin/items/{}/revert", other["id"].as_str().unwrap()),
        json!({"revision_id":rev}),
        other["version"].as_i64(),
    )
    .await;
    assert_eq!(code, 422);
    s.stop().await;
}

// M3 gate regression: `porch revert` without --version reads the item collection
// for the current version. Batched item reads (collections and conflict-counterpart
// masking) must decode a completion item's interval exactly like single-item reads.
#[sqlx::test(migrations = "../db/migrations")]
async fn gate_revert_with_completion_items_in_household(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let admin = s.init.admin_member_id;
    let (code, completion) = call(&s, "POST", "/v1/items", json!({"title":"Completion","kind":"deadline","due_at":"2027-01-01T12:00:00Z","tzid":"UTC","recurrence_mode":"completion","completion_interval":{"months":0,"days":2,"microseconds":0}}), None).await;
    assert_eq!(code, 201);
    let (code, original) = call(
        &s,
        "POST",
        "/v1/items",
        json!({"title":"Revert original","assignees":[admin]}),
        None,
    )
    .await;
    assert_eq!(code, 201);
    let id = original["id"].as_str().unwrap();
    let path = format!("/v1/items/{id}");
    let (code, revisions) = call(
        &s,
        "GET",
        &format!("/v1/admin/items/{id}/revisions"),
        json!({}),
        None,
    )
    .await;
    assert_eq!(code, 200);
    let rev = revisions["items"].as_array().unwrap().last().unwrap()["id"].clone();
    let (code, changed) = call(
        &s,
        "PATCH",
        &path,
        json!({"title":"Revert changed"}),
        original["version"].as_i64(),
    )
    .await;
    assert_eq!(code, 200);
    let (code, stale) = call(
        &s,
        "PATCH",
        &path,
        json!({"title":"Stale"}),
        original["version"].as_i64(),
    )
    .await;
    assert_eq!(code, 409);
    assert_eq!(stale["current"]["version"], changed["version"]);
    // The CLI's version lookup: the whole household collection, deleted included.
    let (code, listed) = call(&s, "GET", "/v1/items?include_deleted=true", json!({}), None).await;
    assert_eq!(code, 200, "{listed}");
    let find = |page: &Value, id: &Value| {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == *id)
            .cloned()
            .unwrap()
    };
    let (code, shown) = call(
        &s,
        "GET",
        &format!("/v1/items/{}", completion["id"].as_str().unwrap()),
        json!({}),
        None,
    )
    .await;
    assert_eq!(code, 200);
    let listed_completion = find(&listed, &completion["id"]);
    assert_eq!(
        listed_completion["completion_interval"],
        json!({"months":0,"days":2,"microseconds":0})
    );
    assert_eq!(listed_completion, shown);
    let version = find(&listed, &original["id"])["version"].as_i64();
    assert_eq!(version, changed["version"].as_i64());
    let (code, restored) = call(
        &s,
        "POST",
        &format!("/v1/admin/items/{id}/revert"),
        json!({"revision_id":rev}),
        version,
    )
    .await;
    assert_eq!(code, 200, "{restored}");
    assert_eq!(restored["title"], "Revert original");
    assert_eq!(restored["assignees"], json!([admin]));
    assert!(restored["version"].as_i64() > changed["version"].as_i64());
    assert_eq!(restored["pending_derivation"], false);
    // Mutation responses mask conflict counterparts through the same batched read.
    let (code, recurring) = call(&s, "POST", "/v1/items", json!({"title":"Recurring slot","kind":"appointment","starts_at":"2027-01-01T12:00:00Z","ends_at":"2027-01-01T13:00:00Z","tzid":"UTC","recurrence_mode":"completion","completion_interval":{"months":0,"days":7,"microseconds":0}}), None).await;
    assert_eq!(code, 201, "{recurring}");
    let (code, overlap) = call(&s, "POST", "/v1/items", json!({"title":"Overlap","kind":"appointment","starts_at":"2027-01-01T12:30:00Z","ends_at":"2027-01-01T13:30:00Z","tzid":"UTC"}), None).await;
    assert_eq!(code, 201, "{overlap}");
    assert!(
        overlap["conflicts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["other_item_id"] == recurring["id"] && c["kind"] == "hard"),
        "{overlap}"
    );
    s.stop().await;
}

// S5 fix1: WRITE must reach the handler's admin gate, not only the mutation write gate.
#[sqlx::test(migrations = "../db/migrations")]
async fn write_pat_cannot_administer_users_or_purge(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let (status, member) = call(
        &s,
        "POST",
        "/v1/admin/users",
        json!({"display_name":"Scope child","role":"dependent"}),
        None,
    )
    .await;
    assert_eq!(status, 201);
    let id = member["id"].as_str().unwrap();
    sqlx::query("update porchlight.api_tokens set scopes=ARRAY['write']")
        .execute(&pool)
        .await
        .unwrap();
    for (method, path, body, version) in [
        ("GET", "/v1/admin/users".to_owned(), json!({}), None),
        (
            "POST",
            "/v1/admin/users".to_owned(),
            json!({"display_name":"Denied","role":"dependent"}),
            None,
        ),
        (
            "PATCH",
            format!("/v1/admin/users/{id}"),
            json!({"display_name":"Denied"}),
            member["version"].as_i64(),
        ),
        (
            "DELETE",
            format!("/v1/admin/users/{id}"),
            json!({}),
            member["version"].as_i64(),
        ),
        (
            "POST",
            "/v1/admin/purge".to_owned(),
            json!({"item_ids":[Uuid::now_v7()]}),
            None,
        ),
    ] {
        assert_eq!(
            call(&s, method, &path, body, version).await.0,
            403,
            "{method} {path}"
        );
    }
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn member_writers_emit_tickles(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let (status, m) = call(
        &s,
        "POST",
        "/v1/admin/users",
        json!({"display_name":"Tickle","role":"dependent"}),
        None,
    )
    .await;
    assert_eq!(status, 201);
    let id: Uuid = m["id"].as_str().unwrap().parse().unwrap();
    let count: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.outbox where kind='member.changed' and entity_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "add tickle missing");
    let (code, edited) = call(
        &s,
        "PATCH",
        &format!("/v1/admin/users/{id}"),
        json!({"display_name":"Tickled","color":"#FABD2F"}),
        m["version"].as_i64(),
    )
    .await;
    assert_eq!(code, 200);
    let count: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.outbox where kind='member.changed' and entity_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 2, "edit tickle missing");
    assert_eq!(
        call(
            &s,
            "DELETE",
            &format!("/v1/admin/users/{id}"),
            json!({}),
            edited["version"].as_i64()
        )
        .await
        .0,
        200
    );
    let count: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.outbox where kind='member.changed' and entity_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 3, "delete tickle missing");
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn purge_guard_checks_rows_not_just_catalog(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    for (parent, action) in [
        ("items", "CASCADE"),
        ("items", "NO ACTION"),
        ("items", "SET NULL"),
        ("item_occurrences", "CASCADE"),
        ("item_occurrences", "NO ACTION"),
        ("item_occurrences", "SET NULL"),
    ] {
        let mut ids = Vec::new();
        for title in ["Guard has rows", "Guard no rows"] {
            let (code, i) = call(&s, "POST", "/v1/items", json!({"title":title}), None).await;
            assert_eq!(code, 201);
            ids.push(i["id"].as_str().unwrap().parse::<Uuid>().unwrap());
        }
        let dependent_id: Uuid = if parent == "items" {
            ids[0]
        } else {
            sqlx::query_scalar(
                "select id from porchlight.item_occurrences where item_id=$1 limit 1",
            )
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap()
        };
        // The owner-only test fixture intentionally grants maintenance NO SELECT on this relation.
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE TABLE porchlight.unhandled_purge (parent uuid REFERENCES porchlight.{parent}(id) ON DELETE {action}); ALTER TABLE porchlight.unhandled_purge OWNER TO porchlight_owner"))).execute(&pool).await.unwrap();
        sqlx::query("insert into porchlight.unhandled_purge(parent) values($1)")
            .bind(dependent_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("update porchlight.items set deleted_at=now() where id=any($1)")
            .bind(&ids)
            .execute(&pool)
            .await
            .unwrap();
        let (code, p) = call(
            &s,
            "POST",
            "/v1/admin/purge",
            json!({"item_ids":[ids[1]]}),
            None,
        )
        .await;
        assert_eq!(
            code, 200,
            "unhandled {parent}/{action} has no rows for this item: {p}"
        );
        let before: (i64,i64,i64,i64) = sqlx::query_as("select (select count(*) from porchlight.items),(select count(*) from porchlight.item_revisions),(select count(*) from porchlight.purge_log),(select count(*) from porchlight.item_occurrences)").fetch_one(&pool).await.unwrap();
        let (code, p) = call(
            &s,
            "POST",
            "/v1/admin/purge",
            json!({"item_ids":[ids[0]]}),
            None,
        )
        .await;
        assert_eq!(code, 409, "{parent}/{action}: {p}");
        assert_eq!(
            p["type"],
            "urn:porchlight:problem:purge-dependency-unhandled"
        );
        assert!(
            p["detail"]
                .as_str()
                .unwrap()
                .contains("porchlight.unhandled_purge"),
            "{p}"
        );
        let after: (i64,i64,i64,i64) = sqlx::query_as("select (select count(*) from porchlight.items),(select count(*) from porchlight.item_revisions),(select count(*) from porchlight.purge_log),(select count(*) from porchlight.item_occurrences)").fetch_one(&pool).await.unwrap();
        assert_eq!(before, after, "refusal must be atomic");
        let retained: Uuid = sqlx::query_scalar("select parent from porchlight.unhandled_purge")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(retained, dependent_id, "no silent cascade or SET NULL");
        sqlx::query("delete from porchlight.unhandled_purge")
            .execute(&pool)
            .await
            .unwrap();
        let (code, p) = call(
            &s,
            "POST",
            "/v1/admin/purge",
            json!({"item_ids":[ids[0]]}),
            None,
        )
        .await;
        assert_eq!(code, 200, "empty unhandled {parent}/{action}: {p}");
        sqlx::query("DROP TABLE porchlight.unhandled_purge")
            .execute(&pool)
            .await
            .unwrap();
    }
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn purge_guard_checks_composite_and_new_fk_shapes(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let mut ids = Vec::new();
    for title in ["Composite target", "Unrelated survivor"] {
        let (code, i) = call(&s, "POST", "/v1/items", json!({"title":title}), None).await;
        assert_eq!(code, 201);
        ids.push(i["id"].as_str().unwrap().parse::<Uuid>().unwrap());
    }
    let (household, member): (Uuid, Uuid) =
        sqlx::query_as("select household_id,owner_id from porchlight.items where id=$1")
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::raw_sql("CREATE TABLE porchlight.unhandled_composite (household_id uuid, parent uuid, FOREIGN KEY(household_id,parent) REFERENCES porchlight.items(household_id,id) ON DELETE CASCADE); ALTER TABLE porchlight.unhandled_composite OWNER TO porchlight_owner;").execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.unhandled_composite values($1,$2)")
        .bind(household)
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update porchlight.items set deleted_at=now() where id=$1")
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
    let (code, p) = call(
        &s,
        "POST",
        "/v1/admin/purge",
        json!({"item_ids":[ids[0]]}),
        None,
    )
    .await;
    assert_eq!(code, 409, "{p}");
    assert!(
        p["detail"]
            .as_str()
            .unwrap()
            .contains("porchlight.unhandled_composite")
    );
    sqlx::query("delete from porchlight.unhandled_composite")
        .execute(&pool)
        .await
        .unwrap();
    // A new FK on an already handled table must NOT be accepted by table name.
    sqlx::query("alter table porchlight.item_viewers add column guard_parent uuid references porchlight.items(id) on delete cascade").execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.item_viewers(household_id,item_id,member_id,guard_parent) values($1,$2,$3,$4)").bind(household).bind(ids[1]).bind(member).bind(ids[0]).execute(&pool).await.unwrap();
    let (code, p) = call(
        &s,
        "POST",
        "/v1/admin/purge",
        json!({"item_ids":[ids[0]]}),
        None,
    )
    .await;
    assert_eq!(code, 409, "{p}");
    assert!(
        p["detail"]
            .as_str()
            .unwrap()
            .contains("porchlight.item_viewers")
    );
    let viewer: Uuid =
        sqlx::query_scalar("select guard_parent from porchlight.item_viewers where item_id=$1")
            .bind(ids[1])
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(viewer, ids[0]);
    let audit: i64 = sqlx::query_scalar("select count(*) from porchlight.purge_log")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(audit, 0);
    sqlx::query("delete from porchlight.item_viewers where item_id=$1")
        .bind(ids[1])
        .execute(&pool)
        .await
        .unwrap();
    let (code, p) = call(
        &s,
        "POST",
        "/v1/admin/purge",
        json!({"item_ids":[ids[0]]}),
        None,
    )
    .await;
    assert_eq!(
        code, 200,
        "empty unknown FKs on new/handled relations must not block: {p}"
    );
    let allowed: (bool,bool,bool) = sqlx::query_as("select has_function_privilege('porchlight_app','porchlight.pl_purge_unhandled_relation(uuid,uuid[])','EXECUTE'),has_function_privilege('porchlight_agent','porchlight.pl_purge_unhandled_relation(uuid,uuid[])','EXECUTE'),has_function_privilege('porchlight_maintenance','porchlight.pl_purge_unhandled_relation(uuid,uuid[])','EXECUTE')").fetch_one(&pool).await.unwrap();
    assert_eq!(allowed, (false, false, true), "probe is maintenance-only");
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn purge_counts_calendar_and_planning_dependents(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let mut ids = Vec::new();
    for title in ["Purge calendar/planning", "Keep calendar/planning"] {
        let (code, i) = call(&s, "POST", "/v1/items", json!({"title":title}), None).await;
        assert_eq!(code, 201);
        ids.push(i["id"].as_str().unwrap().parse::<Uuid>().unwrap());
    }
    let (household, member): (Uuid, Uuid) =
        sqlx::query_as("select household_id,owner_id from porchlight.items where id=$1")
            .bind(ids[0])
            .fetch_one(&pool)
            .await
            .unwrap();
    let source: Uuid = sqlx::query_scalar("insert into porchlight.ics_sources(household_id,owner_member_id,kind,import_key,name) values($1,$2,'import','purge-test','Purge source') returning id").bind(household).bind(member).fetch_one(&pool).await.unwrap();
    for (n, id) in ids.iter().enumerate() {
        let occurrence: Uuid = sqlx::query_scalar(
            "select id from porchlight.item_occurrences where item_id=$1 limit 1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("insert into porchlight.ics_uid_map(household_id,source_id,uid,recurrence_key,item_id,occurrence_id) values($1,$2,$3,'master',$4,$5)").bind(household).bind(source).bind(format!("purge-{n}")).bind(id).bind(occurrence).execute(&pool).await.unwrap();
        sqlx::query("insert into porchlight.schedule_suggestions(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id) values($1,$2,$3,$4,'2026-10-12 13:00Z','2026-10-12 14:00Z',$5)").bind(household).bind(member).bind(id).bind(occurrence).bind(Uuid::now_v7()).execute(&pool).await.unwrap();
        sqlx::query("update porchlight.items set ics_source_id=$1 where id=$2")
            .bind(source)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    let history_before: (i64,i64) = sqlx::query_as("select (select count(*) from porchlight.ics_source_revisions),(select count(*) from porchlight.planning_audit)").fetch_one(&pool).await.unwrap();
    sqlx::query("update porchlight.items set deleted_at=now() where id=$1")
        .bind(ids[0])
        .execute(&pool)
        .await
        .unwrap();
    let (code, p) = call(
        &s,
        "POST",
        "/v1/admin/purge",
        json!({"item_ids":[ids[0]]}),
        None,
    )
    .await;
    assert_eq!(code, 200, "{p}");
    assert_eq!(p["counts"]["ics_uid_map"], 1);
    assert_eq!(p["counts"]["schedule_suggestions"], 1);
    let audit: Value = sqlx::query_scalar("select counts from porchlight.purge_log where id=$1")
        .bind(p["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(audit, p["counts"]);
    for table in ["ics_uid_map", "schedule_suggestions"] {
        let survivors: Vec<Uuid> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "select item_id from porchlight.{table}"
        )))
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(survivors, vec![ids[1]], "{table}");
    }
    let sources: i64 =
        sqlx::query_scalar("select count(*) from porchlight.ics_sources where id=$1")
            .bind(source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(sources, 1, "source is not an item dependent");
    let history_after: (i64,i64) = sqlx::query_as("select (select count(*) from porchlight.ics_source_revisions),(select count(*) from porchlight.planning_audit)").fetch_one(&pool).await.unwrap();
    assert_eq!(
        history_before.0, history_after.0,
        "calendar source history survives"
    );
    assert_eq!(
        history_before.1 + 1,
        history_after.1,
        "planning history survives plus the audited suggestion deletion"
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn purge_counts_attached_lists_and_children(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let (_, i) = call(
        &s,
        "POST",
        "/v1/items",
        json!({"title":"Purge lists"}),
        None,
    )
    .await;
    let id = i["id"].as_str().unwrap();
    let (code, l) = call(
        &s,
        "POST",
        "/v1/lists",
        json!({"name":"Checklist","item_id":id}),
        None,
    )
    .await;
    assert_eq!(code, 201);
    let lid = l["id"].as_str().unwrap();
    let (code, e) = call(
        &s,
        "POST",
        &format!("/v1/lists/{lid}/entries"),
        json!({"label":"Entry"}),
        None,
    )
    .await;
    assert_eq!(code, 201);
    let eid = e["id"].as_str().unwrap();
    assert_eq!(
        call(
            &s,
            "POST",
            &format!("/v1/list-entries/{eid}/check"),
            json!({}),
            e["version"].as_i64()
        )
        .await
        .0,
        200
    );
    assert_eq!(
        call(
            &s,
            "DELETE",
            &format!("/v1/items/{id}"),
            json!({}),
            i["version"].as_i64()
        )
        .await
        .0,
        200
    );
    let (code, p) = call(
        &s,
        "POST",
        "/v1/admin/purge",
        json!({"item_ids":[id]}),
        None,
    )
    .await;
    assert_eq!(code, 200, "{}", p["type"]);
    for table in [
        "lists",
        "list_entries",
        "list_entry_checks",
        "list_revisions",
        "list_entry_revisions",
    ] {
        assert!(p["counts"][table].as_i64().unwrap() > 0, "{table}");
    }
    assert_eq!(p["counts"]["list_viewers"], 0);
    let audit: Value = sqlx::query_scalar("select counts from porchlight.purge_log where id=$1")
        .bind(p["id"].as_str().unwrap().parse::<Uuid>().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(audit, p["counts"]);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn admin_defaults_last_admin_revision_pages_and_retry_truth(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    sqlx::query("update porchlight.households set tzid='Pacific/Auckland' where id=$1")
        .bind(s.init.household_id)
        .execute(&pool)
        .await
        .unwrap();
    for body in [
        json!({"display_name":"Dependent","role":"dependent"}),
        json!({"display_name":"User","username":"zoneuser","initial_password":"another safe password"}),
    ] {
        let (code, member) = call(&s, "POST", "/v1/admin/users", body, None).await;
        assert_eq!(code, 201);
        assert_eq!(member["tzid"], "Pacific/Auckland");
    }
    let (code, _) = call(
        &s,
        "POST",
        "/v1/admin/users",
        json!({"display_name":"Bad dependent","role":"dependent","tzid":"UTC"}),
        None,
    )
    .await;
    assert_eq!(code, 422);
    let (_, users) = call(&s, "GET", "/v1/admin/users", json!({}), None).await;
    let admin = users["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "super_admin")
        .unwrap();
    let path = format!("/v1/admin/users/{}", admin["id"].as_str().unwrap());
    for (method, body) in [("PATCH", json!({"role":"user"})), ("DELETE", json!({}))] {
        assert_eq!(
            call(&s, method, &path, body, admin["version"].as_i64())
                .await
                .0,
            422
        );
    }
    let (_, item) = call(
        &s,
        "POST",
        "/v1/items",
        json!({"title":"Revision ordering"}),
        None,
    )
    .await;
    let id = item["id"].as_str().unwrap();
    let uuid = Uuid::parse_str(id).unwrap();
    let mut version = item["version"].as_i64();
    for title in ["Second", "Third"] {
        let (code, edited) = call(
            &s,
            "PATCH",
            &format!("/v1/items/{id}"),
            json!({"title":title}),
            version,
        )
        .await;
        assert_eq!(code, 200);
        version = edited["version"].as_i64();
    }
    let mut path = format!("/v1/admin/items/{id}/revisions?limit=1");
    let mut revs = vec![];
    loop {
        let (code, page) = call(&s, "GET", &path, json!({}), None).await;
        assert_eq!(code, 200);
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        revs.push(page["items"][0]["rev"].as_i64().unwrap());
        if let Some(cursor) = page["next_cursor"].as_str() {
            path = format!("/v1/admin/items/{id}/revisions?limit=1&cursor={cursor}");
        } else {
            break;
        }
        assert!(revs.len() < 10);
    }
    assert_eq!(revs, vec![1, 2, 3]);
    let path = format!("/v1/admin/derive/{id}/retry");
    let (code, response) = call(&s, "POST", &path, json!({}), None).await;
    assert_eq!(code, 200);
    assert_eq!(response["queued"], false);
    sqlx::query("update porchlight.items set due_date='2027-01-01' where id=$1")
        .bind(uuid)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(&s, "POST", &path, json!({}), None).await.1["queued"],
        true
    );
    sqlx::query("delete from porchlight.item_derivation where item_id=$1")
        .bind(uuid)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        call(&s, "POST", &path, json!({}), None).await.1["queued"],
        false
    );
    s.stop().await;
}

// Snapshot complete domain/history rows, not just counts: refusal must not change
// parent versions, delete audit/history, or silently cascade a synthetic delivery.
async fn purge_snapshot(pool: &PgPool) -> Vec<Value> {
    let mut rows = Vec::new();
    for table in [
        "items",
        "item_occurrences",
        "item_derivation",
        "item_assignees",
        "item_viewers",
        "item_reminders",
        "member_agenda",
        "scheduled_reminders",
        "item_revisions",
        "task_events",
        "notification_events",
        "notifications",
        "notification_sources",
        "notification_event_delivery",
        "notification_delivery_retry",
        "notification_prefs",
        "lists",
        "list_entries",
        "list_viewers",
        "list_entry_checks",
        "list_revisions",
        "list_entry_revisions",
        "ics_uid_map",
        "schedule_suggestions",
        "planning_audit",
        "purge_log",
        "comments",
        "attachments",
        "collaboration_audit",
        "blobs",
    ] {
        rows.push(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "select coalesce(jsonb_agg(to_jsonb(t) order by to_jsonb(t)::text),'[]') from porchlight.{table} t"
        ))).fetch_one(pool).await.unwrap());
    }
    rows
}

#[sqlx::test(migrations = "../db/migrations")]
async fn purge_guard_closure_blocks_second_order_cascades(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    for parent in [
        "notification_events",
        "lists",
        "list_entries",
        "list_entry_revisions",
        "schedule_suggestions",
        "item_revisions",
    ] {
        let mut ids = Vec::new();
        let mut parents = Vec::new();
        for title in ["Closure selected", "Closure unrelated"] {
            let (code, item) = call(&s, "POST", "/v1/items", json!({"title":title}), None).await;
            assert_eq!(code, 201, "{item}");
            let id: Uuid = item["id"].as_str().unwrap().parse().unwrap();
            let (household, member): (Uuid, Uuid) =
                sqlx::query_as("select household_id,owner_id from porchlight.items where id=$1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            let parent_id: Uuid = match parent {
                "notification_events" => sqlx::query_scalar("insert into porchlight.notification_events(household_id,recipient_member_id,kind,item_id,entity_type,entity_id) values($1,$2,'assigned',$3,'item',$3) returning id")
                    .bind(household).bind(member).bind(id).fetch_one(&pool).await.unwrap(),
                "schedule_suggestions" => sqlx::query_scalar("insert into porchlight.schedule_suggestions(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id) select $1,$2,$3,id,'2026-10-12 13:00Z','2026-10-12 14:00Z',$4 from porchlight.item_occurrences where item_id=$3 limit 1 returning id")
                    .bind(household).bind(member).bind(id).bind(Uuid::now_v7()).fetch_one(&pool).await.unwrap(),
                "item_revisions" => sqlx::query_scalar("select id from porchlight.item_revisions where item_id=$1 order by rev limit 1")
                    .bind(id).fetch_one(&pool).await.unwrap(),
                _ => {
                    let (code, list) = call(&s, "POST", "/v1/lists", json!({"name":"Closure list","item_id":id}), None).await;
                    assert_eq!(code, 201, "{list}");
                    let lid: Uuid = list["id"].as_str().unwrap().parse().unwrap();
                    if parent == "lists" { lid } else {
                        let (code, entry) = call(&s, "POST", &format!("/v1/lists/{lid}/entries"), json!({"label":"Closure entry"}), None).await;
                        assert_eq!(code, 201, "{entry}");
                        let eid: Uuid = entry["id"].as_str().unwrap().parse().unwrap();
                        if parent == "list_entries" { eid } else {
                            sqlx::query_scalar("select id from porchlight.list_entry_revisions where entry_id=$1 order by rev limit 1")
                                .bind(eid).fetch_one(&pool).await.unwrap()
                        }
                    }
                }
            };
            ids.push(id);
            parents.push(parent_id);
        }
        // Owner-owned but maintenance cannot read it, like a future S6a delivery
        // source. CASCADE would silently delete it without the catalog closure.
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE TABLE porchlight.unhandled_closure(parent uuid REFERENCES porchlight.{parent}(id) ON DELETE CASCADE); ALTER TABLE porchlight.unhandled_closure OWNER TO porchlight_owner"))).execute(&pool).await.unwrap();
        sqlx::query("insert into porchlight.unhandled_closure values($1)")
            .bind(parents[0])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("update porchlight.items set deleted_at=now() where id=any($1)")
            .bind(&ids)
            .execute(&pool)
            .await
            .unwrap();
        let (code, p) = call(
            &s,
            "POST",
            "/v1/admin/purge",
            json!({"item_ids":[ids[1]]}),
            None,
        )
        .await;
        assert_eq!(code, 200, "unrelated {parent} rows: {p}");
        let before = purge_snapshot(&pool).await;
        let (code, p) = call(
            &s,
            "POST",
            "/v1/admin/purge",
            json!({"item_ids":[ids[0]]}),
            None,
        )
        .await;
        assert_eq!(
            code, 409,
            "second-order {parent} CASCADE must be guarded: {p}"
        );
        assert_eq!(
            p["type"],
            "urn:porchlight:problem:purge-dependency-unhandled"
        );
        assert!(
            p["detail"]
                .as_str()
                .unwrap()
                .contains("porchlight.unhandled_closure"),
            "{p}"
        );
        assert_eq!(
            purge_snapshot(&pool).await,
            before,
            "all domain rows/history/audit retained for {parent}"
        );
        let retained: Uuid = sqlx::query_scalar("select parent from porchlight.unhandled_closure")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(retained, parents[0], "no silent cascade");
        sqlx::query("delete from porchlight.unhandled_closure")
            .execute(&pool)
            .await
            .unwrap();
        // Every selected deletion parent, including notification/history and deep
        // list rows, is locked before the guard. An FK insert cannot race it.
        let household: Uuid =
            sqlx::query_scalar("select household_id from porchlight.items where id=$1")
                .bind(ids[0])
                .fetch_one(&pool)
                .await
                .unwrap();
        let mut guarded = pool.begin().await.unwrap();
        let unknown: Option<String> =
            sqlx::query_scalar("select porchlight.pl_purge_unhandled_relation($1,$2)")
                .bind(household)
                .bind(vec![ids[0]])
                .fetch_one(&mut *guarded)
                .await
                .unwrap();
        assert!(unknown.is_none());
        let mut inserter = pool.begin().await.unwrap();
        sqlx::query("set local lock_timeout='100ms'")
            .execute(&mut *inserter)
            .await
            .unwrap();
        let error = sqlx::query("insert into porchlight.unhandled_closure values($1)")
            .bind(parents[0])
            .execute(&mut *inserter)
            .await
            .unwrap_err();
        assert_eq!(
            error.as_database_error().unwrap().code().as_deref(),
            Some("55P03"),
            "{parent} FK insertion must wait for purge locks"
        );
        inserter.rollback().await.unwrap();
        guarded.rollback().await.unwrap();
        let (code, p) = call(
            &s,
            "POST",
            "/v1/admin/purge",
            json!({"item_ids":[ids[0]]}),
            None,
        )
        .await;
        assert_eq!(code, 200, "empty unhandled {parent} dependency: {p}");
        sqlx::query("drop table porchlight.unhandled_closure")
            .execute(&pool)
            .await
            .unwrap();
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn purge_guard_retains_s7b_comments_and_attachments(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    for table in ["comments", "attachments"] {
        let (code, i) = call(
            &s,
            "POST",
            "/v1/items",
            json!({"title":"Collaboration retention"}),
            None,
        )
        .await;
        assert_eq!(code, 201);
        let id: Uuid = i["id"].as_str().unwrap().parse().unwrap();
        let (household, member): (Uuid, Uuid) =
            sqlx::query_as("select household_id,owner_id from porchlight.items where id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        if table == "comments" {
            let (code, c) = call(
                &s,
                "POST",
                &format!("/v1/items/{id}/comments"),
                json!({"body":"Keep comment history"}),
                None,
            )
            .await;
            assert_eq!(code, 201, "{c}");
        } else {
            // Metadata only: purge is DB-only, not a blob-download/GC operation.
            sqlx::query("insert into porchlight.blobs(household_id,sha256,size,content_type) values($1,repeat('a',64),1,'text/plain')")
                .bind(household).execute(&pool).await.unwrap();
            sqlx::query("insert into porchlight.attachments(household_id,item_id,blob_sha256,original_name,uploader_id) values($1,$2,repeat('a',64),'retained.txt',$3)")
                .bind(household).bind(id).bind(member).execute(&pool).await.unwrap();
        }
        sqlx::query("update porchlight.items set deleted_at=now() where id=$1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        let (code, p) = call(
            &s,
            "POST",
            "/v1/admin/purge",
            json!({"item_ids":[id]}),
            None,
        )
        .await;
        assert_eq!(code, 200, "{p}");
        assert_eq!(p["counts"][table], 1);
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(
            format!("select count(*) from porchlight.{table} where item_id=$1").as_str(),
        ))
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 0);
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn purge_guard_retains_real_s6a_dependents(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    for dependent in [
        "notification_event_delivery",
        "notification_sources",
        "notifications",
    ] {
        let mut ids = Vec::new();
        let mut events = Vec::new();
        for title in ["S6a selected", "S6a unrelated"] {
            let (code, item) = call(&s, "POST", "/v1/items", json!({"title":title}), None).await;
            assert_eq!(code, 201, "{item}");
            let id: Uuid = item["id"].as_str().unwrap().parse().unwrap();
            let event: Uuid = sqlx::query_scalar("insert into porchlight.notification_events(household_id,recipient_member_id,kind,item_id,entity_type,entity_id) values($1,$2,'assigned',$3,'item',$3) returning id")
                .bind(s.init.household_id).bind(s.init.admin_member_id).bind(id).fetch_one(&pool).await.unwrap();
            ids.push(id);
            events.push(event);
        }
        let notification = if dependent == "notification_event_delivery" {
            sqlx::query("insert into porchlight.notification_event_delivery(event_id,attempts,retry_at,last_error) values($1,1,now(),'constraint')")
                .bind(events[0]).execute(&pool).await.unwrap();
            None
        } else {
            // Sources exercise only the event CASCADE, with no direct item FK;
            // the third case independently proves SET NULL inbox refs are guarded.
            let n: Uuid = sqlx::query_scalar("insert into porchlight.notifications(household_id,recipient_member_id,kind,category,item_id,entity_type,entity_id,window_start,release_at) values($1,$2,'assigned','assignments',$3,'item',$4,now(),now()) returning id")
                .bind(s.init.household_id).bind(s.init.admin_member_id)
                .bind(if dependent == "notifications" {Some(ids[0])} else {None})
                .bind(ids[0]).fetch_one(&pool).await.unwrap();
            if dependent == "notification_sources" {
                sqlx::query("insert into porchlight.notification_sources(notification_id,event_id) values($1,$2)")
                    .bind(n).bind(events[0]).execute(&pool).await.unwrap();
            }
            Some(n)
        };
        sqlx::query("update porchlight.items set deleted_at=now() where id=any($1)")
            .bind(&ids)
            .execute(&pool)
            .await
            .unwrap();
        let (code, p) = call(
            &s,
            "POST",
            "/v1/admin/purge",
            json!({"item_ids":[ids[1]]}),
            None,
        )
        .await;
        assert_eq!(
            code, 200,
            "unrelated real {dependent} rows must not block: {p}"
        );
        let (code, p) = call(
            &s,
            "POST",
            "/v1/admin/purge",
            json!({"item_ids":[ids[0]]}),
            None,
        )
        .await;
        assert_eq!(
            code, 200,
            "selected {dependent} must be explicitly handled: {p}"
        );
        assert_eq!(p["counts"][dependent], 1);
        if let Some(n) = notification {
            let item: Option<Uuid> =
                sqlx::query_scalar("select item_id from porchlight.notifications where id=$1")
                    .bind(n)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(item, None);
        }
        assert_eq!(p["counts"]["items"], 1);
        assert_eq!(p["counts"]["notification_events"], 1);
        assert!(
            !sqlx::query_scalar::<_, bool>(
                "select exists(select 1 from porchlight.notification_events where id=$1)"
            )
            .bind(events[0])
            .fetch_one(&pool)
            .await
            .unwrap()
        );
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn revert_s6a_assignment_producer_and_unchanged_suppression(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let (code, member) = call(&s,"POST","/v1/admin/users",json!({"display_name":"Recipient","role":"user","username":"recipient","initial_password":"recipient safe password"}),None).await;
    assert_eq!(code, 201, "{member}");
    let recipient: Uuid = member["id"].as_str().unwrap().parse().unwrap();
    let (code, item) = call(
        &s,
        "POST",
        "/v1/items",
        json!({"title":"Revert notifications","assignees":[recipient]}),
        None,
    )
    .await;
    assert_eq!(code, 201, "{item}");
    let id: Uuid = item["id"].as_str().unwrap().parse().unwrap();
    let revision: Uuid = sqlx::query_scalar(
        "select id from porchlight.item_revisions where item_id=$1 order by rev desc limit 1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let path = format!("/v1/items/{id}");
    let (code, changed) = call(
        &s,
        "PUT",
        &format!("{path}/assignees"),
        json!({"assignees":[]}),
        item["version"].as_i64(),
    )
    .await;
    assert_eq!(code, 200, "{changed}");
    let before: i64 =
        sqlx::query_scalar("select count(*) from porchlight.notification_events where item_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let (code, restored) = call(
        &s,
        "POST",
        &format!("/v1/admin/items/{id}/revert"),
        json!({"revision_id":revision}),
        changed["version"].as_i64(),
    )
    .await;
    assert_eq!(code, 200, "{restored}");
    let events: Vec<Value> = sqlx::query_scalar("select to_jsonb(e) from porchlight.notification_events e where item_id=$1 order by created_at,id").bind(id).fetch_all(&pool).await.unwrap();
    assert_eq!(
        events.len() as i64,
        before + 1,
        "one normal task-event assignment producer, not a bespoke revert producer"
    );
    let event = events.last().unwrap();
    assert_eq!(event["kind"], "assigned");
    assert_eq!(event["recipient_member_id"], json!(recipient));
    assert_eq!(
        event["actor_member_id"],
        json!(s.init.admin_member_id),
        "HTTP PAT attribution remains bound to its owner, not an arbitrary member"
    );
    let task: Value = sqlx::query_scalar(
        "select to_jsonb(t) from porchlight.task_events t where 'task:'||id::text=$1",
    )
    .bind(event["dedupe_key"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(event["actor_label"], task["actor"]["application_name"]);
    assert!(event["actor_label"].as_str().unwrap().starts_with("agent:"));
    assert_eq!(task["kind"], "assigned");
    assert_eq!(task["data"]["member_id"], json!(recipient));
    let pending: bool = sqlx::query_scalar(
        "select applied_gen<requested_gen from porchlight.item_derivation where item_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!pending, "ordinary revert finishes derivation");
    // A field-only revert and a truly unchanged revert must not reassign an
    // already assigned recipient, nor generate a business notification.
    let (code, changed) = call(
        &s,
        "PATCH",
        &path,
        json!({"title":"Changed title"}),
        restored["version"].as_i64(),
    )
    .await;
    assert_eq!(code, 200, "{changed}");
    let (code, changed) = call(
        &s,
        "PUT",
        &format!("{path}/reminders"),
        json!({"reminder_offsets":[15]}),
        changed["version"].as_i64(),
    )
    .await;
    assert_eq!(code, 200, "{changed}");
    let mut version = changed["version"].as_i64();
    for _ in 0..2 {
        let before: Value = sqlx::query_scalar("select coalesce(jsonb_agg(to_jsonb(e) order by id),'[]') from porchlight.notification_events e where item_id=$1").bind(id).fetch_one(&pool).await.unwrap();
        let (code, restored) = call(
            &s,
            "POST",
            &format!("/v1/admin/items/{id}/revert"),
            json!({"revision_id":revision}),
            version,
        )
        .await;
        assert_eq!(code, 200, "{restored}");
        assert_eq!(restored["title"], "Revert notifications");
        let after: Value = sqlx::query_scalar("select coalesce(jsonb_agg(to_jsonb(e) order by id),'[]') from porchlight.notification_events e where item_id=$1").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(
            after, before,
            "unchanged assignees must not emit duplicate assignment events on revert"
        );
        version = restored["version"].as_i64();
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn revert_s6a_derivation_produces_and_dedupes_conflict(pool: PgPool) {
    let now: chrono::DateTime<chrono::Utc> = "2026-10-06T00:00:00Z".parse().unwrap();
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(move || now),
            ..Default::default()
        },
    )
    .await;
    let (code,item) = call(&s,"POST","/v1/items",json!({"title":"Revert time","kind":"appointment","tzid":"UTC","starts_at":"2026-10-07T10:00:00Z","ends_at":"2026-10-07T11:00:00Z"}),None).await;
    assert_eq!(code, 201, "{item}");
    let id: Uuid = item["id"].as_str().unwrap().parse().unwrap();
    let revision: Uuid = sqlx::query_scalar(
        "select id from porchlight.item_revisions where item_id=$1 order by rev desc limit 1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (code, changed) = call(
        &s,
        "PATCH",
        &format!("/v1/items/{id}"),
        json!({"starts_at":"2026-10-07T12:00:00Z","ends_at":"2026-10-07T13:00:00Z"}),
        item["version"].as_i64(),
    )
    .await;
    assert_eq!(code, 200, "{changed}");
    let (code,other) = call(&s,"POST","/v1/items",json!({"title":"New counterpart","kind":"appointment","tzid":"UTC","starts_at":"2026-10-07T10:30:00Z","ends_at":"2026-10-07T11:30:00Z"}),None).await;
    assert_eq!(code, 201, "{other}");
    let conflicts = || {
        sqlx::query_scalar::<_, i64>(
            "select count(*) from porchlight.notification_events where kind='conflict'",
        )
    };
    assert_eq!(conflicts().fetch_one(&pool).await.unwrap(), 0);
    let mut version = changed["version"].as_i64();
    for _ in 0..2 {
        let (code, restored) = call(
            &s,
            "POST",
            &format!("/v1/admin/items/{id}/revert"),
            json!({"revision_id":revision}),
            version,
        )
        .await;
        assert_eq!(code, 200, "{restored}");
        assert_eq!(
            conflicts().fetch_one(&pool).await.unwrap(),
            1,
            "ordinary derivation emits once per canonical occurrence pair/member"
        );
        version = restored["version"].as_i64();
    }
    let event: Value = sqlx::query_scalar(
        "select to_jsonb(e) from porchlight.notification_events e where kind='conflict'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(event["item_id"], json!(id));
    assert_eq!(event["recipient_member_id"], json!(s.init.admin_member_id));
    assert!(event["actor_label"].as_str().unwrap().starts_with("agent:"));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn revert_s6a_retains_occurrence_completion_without_extra_notifications(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let (code, item) = call(
        &s,
        "POST",
        "/v1/items",
        json!({"title":"Keep completed history"}),
        None,
    )
    .await;
    assert_eq!(code, 201, "{item}");
    let id: Uuid = item["id"].as_str().unwrap().parse().unwrap();
    let revision: Uuid = sqlx::query_scalar(
        "select id from porchlight.item_revisions where item_id=$1 order by rev desc limit 1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (occurrence, version): (Uuid, i32) =
        sqlx::query_as("select id,version from porchlight.item_occurrences where item_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let (code, completed) = call(
        &s,
        "POST",
        &format!("/v1/occurrences/{occurrence}/complete"),
        json!({}),
        Some(i64::from(version)),
    )
    .await;
    assert_eq!(code, 200, "{completed}");
    let before: Value = sqlx::query_scalar("select coalesce(jsonb_agg(to_jsonb(e) order by id),'[]') from porchlight.notification_events e where item_id=$1").bind(id).fetch_one(&pool).await.unwrap();
    // HTTP PAT owner is the recipient, so the shared completion self-suppression
    // rule may suppress the notification, but the immutable task still exists.
    assert_eq!(sqlx::query_scalar::<_,i64>("select count(*) from porchlight.task_events where occurrence_id=$1 and kind='completed'").bind(occurrence).fetch_one(&pool).await.unwrap(),1);
    let status_before: Value = sqlx::query_scalar("select jsonb_build_object('status',status,'completed_at',completed_at,'completed_by',completed_by) from porchlight.item_occurrences where id=$1").bind(occurrence).fetch_one(&pool).await.unwrap();
    let version: i32 = sqlx::query_scalar("select version from porchlight.items where id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let (code, restored) = call(
        &s,
        "POST",
        &format!("/v1/admin/items/{id}/revert"),
        json!({"revision_id":revision}),
        Some(i64::from(version)),
    )
    .await;
    assert_eq!(code, 200, "{restored}");
    assert_eq!(status_before["status"], "done");
    let status_after: Value = sqlx::query_scalar("select jsonb_build_object('status',status,'completed_at',completed_at,'completed_by',completed_by) from porchlight.item_occurrences where id=$1").bind(occurrence).fetch_one(&pool).await.unwrap();
    assert_eq!(
        status_after, status_before,
        "item snapshot does not overwrite authoritative occurrence completion"
    );
    let after: Value = sqlx::query_scalar("select coalesce(jsonb_agg(to_jsonb(e) order by id),'[]') from porchlight.notification_events e where item_id=$1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(
        after, before,
        "revert manufactures no status/completion notification"
    );
    s.stop().await;
}
