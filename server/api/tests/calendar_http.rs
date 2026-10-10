use porchlight_api::testing::{TestClient, TestOptions, TestServer, agent_request, trust};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;

#[test]
fn openapi_documents_text_import_and_feed_preconditions() {
    let doc = serde_json::to_value(porchlight_api::openapi::document()).unwrap();
    let import = &doc["paths"]["/v1/imports/ics"]["post"];
    for content in ["application/json", "text/calendar", "text/plain"] {
        assert!(
            import["requestBody"]["content"].get(content).is_some(),
            "missing upload content type"
        );
    }
    assert!(
        import["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "member_id" && p["in"] == "query")
    );
    for prefix in ["/v1/feeds", "/v1/admin/feeds"] {
        for (suffix, method) in [
            ("/{id}", "put"),
            ("/{id}", "delete"),
            ("/{id}/refresh", "post"),
        ] {
            assert!(
                doc["paths"][format!("{prefix}{suffix}")][method]["responses"]
                    .get("428")
                    .is_some()
            );
        }
    }
}

const EVENT: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:self\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nSUMMARY:Appointment\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

#[sqlx::test(migrations = "../db/migrations")]
async fn app_roles_import_self_other_and_feed_admin_policy(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            db_pool_size: 1,
            ..Default::default()
        },
    )
    .await;
    for (index, role) in ["super_admin", "user"].iter().enumerate() {
        let (_, member) =
            trust::user_named(&pool, &s, &format!("calendar{index}"), "Calendar user").await;
        let mut tx = porchlight_db::tx::begin_as(&pool, &porchlight_db::tx::Actor::system())
            .await
            .unwrap();
        sqlx::query("update porchlight.household_members set role=$2 where id=$1")
            .bind(member)
            .bind(role)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let device = s.session_device(member, Platform::Desktop).await;
        let client = TestClient::new(&s.ca_pem, Some(&device));
        let response = client
            .request(
                s.lan,
                device.request(
                    "POST",
                    "/v1/imports/ics",
                    json!({"content":EVENT,"member_id":member}),
                    Some("self"),
                ),
            )
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        let item = response.json()["item_ids"][0].as_str().unwrap().to_owned();
        let read = client
            .request(
                s.lan,
                device.request("GET", &format!("/v1/items/{item}"), json!({}), None),
            )
            .await
            .unwrap();
        assert!(
            read.json()["capabilities"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "edit"),
            "one-off import must remain editable"
        );
        let response = client
            .request(
                s.lan,
                device.request(
                    "POST",
                    "/v1/imports/ics",
                    json!({"content":EVENT,"member_id":s.init.admin_member_id}),
                    Some("other"),
                ),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status,
            if *role == "super_admin" { 200 } else { 403 }
        );
        let expected = if *role == "super_admin" { 200 } else { 403 };
        assert_eq!(
            client
                .request(s.lan, device.request("GET", "/v1/feeds", json!({}), None))
                .await
                .unwrap()
                .status,
            expected
        );
        for (method, path, body, success) in [
            (
                "POST",
                "/v1/feeds",
                json!({"name":"calendar","url":"https://example.invalid/calendar","target_member_id":null}),
                201,
            ),
            (
                "PUT",
                "/v1/feeds/00000000-0000-0000-0000-000000000001",
                json!({}),
                428,
            ),
            (
                "DELETE",
                "/v1/feeds/00000000-0000-0000-0000-000000000001",
                json!({}),
                428,
            ),
            (
                "POST",
                "/v1/feeds/00000000-0000-0000-0000-000000000001/refresh",
                json!({}),
                428,
            ),
        ] {
            let mut req = device.request(method, path, body, None);
            // PUT is decoded before authorization; send a valid body to exercise policy.
            if method == "PUT" {
                *req.body_mut() = http_body_util::Full::new(hyper::body::Bytes::from(serde_json::to_vec(&json!({"name":"calendar","url":"https://example.invalid/calendar","target_member_id":null})).unwrap()));
            }
            assert_eq!(
                client.request(s.lan, req).await.unwrap().status,
                if *role == "super_admin" { success } else { 403 }
            );
        }
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn agent_scopes_source_versions_pagination_and_text_import(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            db_pool_size: 1,
            ..Default::default()
        },
    )
    .await;
    let (_, read) = trust::seed_pat(&pool, &s, &["read"]).await;
    let (_, write) = trust::seed_pat(&pool, &s, &["write"]).await;
    for token in [&read, &write] {
        let response = agent_request(
            &s.agent_socket,
            Some(token),
            trust::req("GET", "/v1/admin/feeds", json!({}), "list"),
        )
        .await;
        assert_eq!(response.status, 403);
    }
    let body = json!({"name":"school","url":"https://example.invalid/calendar","target_member_id":s.init.admin_member_id});
    let source = agent_request(
        &s.agent_socket,
        Some(s.bootstrap_token()),
        trust::req("POST", "/v1/admin/feeds", body.clone(), "create"),
    )
    .await;
    assert_eq!(source.status, 201);
    let id = source.json()["id"].as_str().unwrap().to_owned();
    let path = format!("/v1/admin/feeds/{id}");
    let mut req = trust::req("PUT", &path, body.clone(), "update");
    req.headers_mut().insert(
        "if-match",
        source.json()["version"].to_string().parse().unwrap(),
    );
    let updated = agent_request(&s.agent_socket, Some(s.bootstrap_token()), req).await;
    assert_eq!(updated.status, 200);
    let mut req = trust::req("DELETE", &path, json!({}), "stale");
    req.headers_mut().insert(
        "if-match",
        source.json()["version"].to_string().parse().unwrap(),
    );
    let stale = agent_request(&s.agent_socket, Some(s.bootstrap_token()), req).await;
    assert_eq!(stale.status, 409);
    assert_eq!(
        stale.json()["current"]["version"],
        updated.json()["version"]
    );
    let second = agent_request(
        &s.agent_socket,
        Some(s.bootstrap_token()),
        trust::req("POST", "/v1/admin/feeds", body, "second"),
    )
    .await;
    assert_eq!(second.status, 201);
    let page = agent_request(
        &s.agent_socket,
        Some(s.bootstrap_token()),
        trust::req("GET", "/v1/admin/feeds?limit=1", json!({}), "page"),
    )
    .await;
    assert_eq!(page.status, 200);
    assert_eq!(page.json()["items"].as_array().unwrap().len(), 1);
    let cursor = page.json()["next_cursor"].as_str().unwrap().to_owned();
    let page = agent_request(
        &s.agent_socket,
        Some(s.bootstrap_token()),
        trust::req(
            "GET",
            &format!("/v1/admin/feeds?limit=1&cursor={cursor}"),
            json!({}),
            "next",
        ),
    )
    .await;
    assert_eq!(page.status, 200);
    assert!(page.json()["next_cursor"].is_null());
    for (token, expected) in [(&read, 403), (&write, 200)] {
        let mut req = trust::req(
            "POST",
            &format!(
                "/v1/imports/ics?member_id={}&scope=text",
                s.init.admin_member_id
            ),
            json!({}),
            "text",
        );
        req.headers_mut().insert(
            "content-type",
            "text/calendar; charset=utf-8".parse().unwrap(),
        );
        *req.body_mut() =
            http_body_util::Full::new(hyper::body::Bytes::from_static(EVENT.as_bytes()));
        assert_eq!(
            agent_request(&s.agent_socket, Some(token), req)
                .await
                .status,
            expected
        );
    }
    let mut req = trust::req("DELETE", &path, json!({}), "remove");
    req.headers_mut().insert(
        "if-match",
        updated.json()["version"].to_string().parse().unwrap(),
    );
    assert_eq!(
        agent_request(&s.agent_socket, Some(s.bootstrap_token()), req)
            .await
            .status,
        200
    );
    s.stop().await;
}
