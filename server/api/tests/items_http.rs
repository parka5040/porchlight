use porchlight_api::testing::{TestClient, TestOptions, TestServer};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrations = "../db/migrations")]
async fn crud_versions_validation_and_replay_on_one_connection(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
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
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let body = json!({"title":"HTTP task"});
    let created = client
        .request(
            s.lan,
            d.request("POST", "/v1/items", body.clone(), Some("create")),
        )
        .await
        .unwrap();
    assert_eq!(
        created.status,
        201,
        "{}",
        String::from_utf8_lossy(&created.body)
    );
    let item = created.json();
    assert_eq!(item["title"], "HTTP task");
    assert_eq!(item["pending_derivation"], false);
    assert!(
        item["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x == "edit")
    );
    let replay = client
        .request(s.lan, d.request("POST", "/v1/items", body, Some("create")))
        .await
        .unwrap();
    assert_eq!(replay.body, created.body);
    let mismatch = client
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/items",
                json!({"title":"other"}),
                Some("create"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(mismatch.status, 422);
    let path = format!("/v1/items/{}", item["id"].as_str().unwrap());
    let missing = client
        .request(
            s.lan,
            d.request("PATCH", &path, json!({"title":"changed"}), None),
        )
        .await
        .unwrap();
    assert_eq!(missing.status, 428);
    let mut req = d.request("PATCH", &path, json!({"title":"changed"}), Some("patch"));
    req.headers_mut()
        .insert("if-match", item["version"].to_string().parse().unwrap());
    let changed = client.request(s.lan, req).await.unwrap();
    assert_eq!(
        changed.status,
        200,
        "{}",
        String::from_utf8_lossy(&changed.body)
    );
    let mut req = d.request("PATCH", &path, json!({"title":"stale"}), None);
    req.headers_mut()
        .insert("if-match", item["version"].to_string().parse().unwrap());
    let stale = client.request(s.lan, req).await.unwrap();
    assert_eq!(stale.status, 409);
    assert_eq!(stale.json()["current"]["title"], "changed");
    let invalid = client
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/items",
                json!({"title":"bad","kind":"deadline","time_mode":"floating"}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(invalid.status, 422);
    assert_eq!(invalid.json()["errors"][0]["field"], "local_due");
    let fetched = client
        .request(s.lan, d.request("GET", &path, json!({}), None))
        .await
        .unwrap();
    assert_eq!(fetched.status, 200);
    let mut req = d.request("DELETE", &path, json!({}), None);
    req.headers_mut().insert(
        "if-match",
        changed.json()["version"].to_string().parse().unwrap(),
    );
    let deleted = client.request(s.lan, req).await.unwrap();
    assert_eq!(deleted.status, 200);
    assert_eq!(
        client
            .request(s.lan, d.request("GET", &path, json!({}), None))
            .await
            .unwrap()
            .status,
        404
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn malformed_read_parameters_use_problem_json(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
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
    let client = TestClient::new(&s.ca_pem, Some(&d));
    for path in [
        "/v1/items/not-a-uuid",
        "/v1/items?updated_since=invalid",
        "/v1/items?limit=invalid",
        "/v1/items/00000000-0000-0000-0000-000000000000/occurrences?from=invalid",
    ] {
        let r = client
            .request(s.lan, d.request("GET", path, json!({}), None))
            .await
            .unwrap();
        assert_eq!(r.status, 400);
        assert_eq!(
            r.headers.get("content-type").unwrap(),
            "application/problem+json"
        );
        assert_eq!(r.json()["type"], "urn:porchlight:problem:invalid-request");
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn agent_read_and_write_scopes_are_enforced_on_both_kinds_of_endpoint(pool: PgPool) {
    use porchlight_api::testing::{agent_request, trust};
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let (_, read) = trust::seed_pat(&pool, &s, &["read"]).await;
    let (_, write) = trust::seed_pat(&pool, &s, &["write"]).await;
    let made = agent_request(
        &s.agent_socket,
        Some(&write),
        trust::req(
            "POST",
            "/v1/items",
            json!({"title":"scoped"}),
            "write-create",
        ),
    )
    .await;
    assert_eq!(made.status, 201);
    let path = format!("/v1/items/{}", made.json()["id"].as_str().unwrap());
    for path in [
        "/v1/items".into(),
        path.clone(),
        format!("{path}/occurrences"),
    ] {
        let permitted = agent_request(
            &s.agent_socket,
            Some(&write),
            trust::req("GET", &path, json!({}), "live-token-read"),
        )
        .await;
        assert_eq!(permitted.status, 200, "{permitted:?}");
        let allowed = agent_request(
            &s.agent_socket,
            Some(&read),
            trust::req("GET", &path, json!({}), "read"),
        )
        .await;
        assert_eq!(allowed.status, 200);
    }
    let denied = agent_request(
        &s.agent_socket,
        Some(&read),
        trust::req(
            "POST",
            "/v1/items",
            json!({"title":"forbidden"}),
            "no-write",
        ),
    )
    .await;
    assert_eq!(denied.status, 403);
    s.stop().await;
}
