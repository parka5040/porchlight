use porchlight_api::testing::{TestClient, TestOptions, TestServer, trust};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;
const CAL: &str = include_str!("../../../core/tests/fixtures/ics/school.ics");

#[sqlx::test(migrations = "../db/migrations")]
async fn school_import_dependent_display_name_and_ambiguity(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-06T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let mut tx = porchlight_db::tx::begin_as(&pool, &porchlight_db::tx::Actor::system())
        .await
        .unwrap();
    let dependent: Uuid = sqlx::query_scalar("insert into porchlight.household_members(household_id,role,display_name,color,tzid) values($1,'dependent','School Child','#AABBCC','America/New_York') returning id").bind(s.init.household_id).fetch_one(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let (_, write) = trust::seed_pat(&pool, &s, &["write"]).await;
    for (name, status) in [("admin", 200), ("School%20Child", 403)] {
        let resolved = porchlight_api::testing::agent_request(
            &s.agent_socket,
            Some(&write),
            trust::req(
                "GET",
                &format!("/v1/admin/feeds/target?username={name}"),
                json!({}),
                "write-resolve",
            ),
        )
        .await;
        assert_eq!(
            resolved.status, status,
            "write PAT may resolve self, not another member"
        );
    }
    let resolved = porchlight_api::testing::agent_request(
        &s.agent_socket,
        Some(s.bootstrap_token()),
        trust::req(
            "GET",
            "/v1/admin/feeds/target?username=School%20Child",
            json!({}),
            "resolve",
        ),
    )
    .await;
    assert_eq!(resolved.status, 200);
    assert_eq!(resolved.json()["member_id"], json!(dependent));
    let imported = c
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/imports/ics",
                json!({"content":CAL,"member_id":dependent}),
                Some("school"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(imported.status, 200);
    assert!(imported.json()["created"].as_u64().unwrap() > 0);
    for id in imported.json()["item_ids"].as_array().unwrap() {
        let item = c
            .request(
                s.lan,
                d.request(
                    "GET",
                    &format!("/v1/items/{}", id.as_str().unwrap()),
                    json!({}),
                    None,
                ),
            )
            .await
            .unwrap();
        assert_eq!(item.status, 200);
        assert_eq!(item.json()["owner_id"], json!(s.init.admin_member_id));
        assert_eq!(item.json()["subject_member_id"], json!(dependent));
        assert_eq!(item.json()["visibility"], "household");
    }
    let (_, second) = trust::user_named(&pool, &s, "schoolchild", "School Child").await;
    let ambiguous = porchlight_api::testing::agent_request(
        &s.agent_socket,
        Some(s.bootstrap_token()),
        trust::req(
            "GET",
            "/v1/admin/feeds/target?username=School%20Child",
            json!({}),
            "resolve",
        ),
    )
    .await;
    assert_eq!(ambiguous.status, 422);
    let message = ambiguous.json().to_string();
    assert!(
        message.contains(&dependent.to_string()) && message.contains(&second.to_string()),
        "ambiguity lists candidate IDs"
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn visibility_and_feed_update_references_are_field_errors(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let body = json!({"name":"valid","url":"https://example.invalid/calendar","target_member_id":s.init.admin_member_id});
    let source = c
        .request(
            s.lan,
            d.request("POST", "/v1/feeds", body.clone(), Some("valid")),
        )
        .await
        .unwrap();
    assert_eq!(source.status, 201);
    let path = format!("/v1/feeds/{}", source.json()["id"].as_str().unwrap());
    for (index, value) in ["selected", "explicit", "bogus"].iter().enumerate() {
        for (method, path, mut request_body) in [
            (
                "POST",
                "/v1/imports/ics",
                json!({"content":CAL,"member_id":s.init.admin_member_id}),
            ),
            ("POST", "/v1/feeds", body.clone()),
            ("PUT", path.as_str(), body.clone()),
        ] {
            request_body["visibility"] = json!(value);
            let mut req = d.request(
                method,
                path,
                request_body,
                Some(&format!("{method}-{path}-{index}")),
            );
            req.headers_mut().insert(
                "if-match",
                source.json()["version"].to_string().parse().unwrap(),
            );
            let response = c.request(s.lan, req).await.unwrap();
            assert_eq!(response.status, 422, "{method} {value}");
            assert!(
                response.json().to_string().contains("visibility"),
                "{method} {value}: {}",
                response.json()
            );
        }
    }
    let mut bad = body;
    bad["target_member_id"] = json!(Uuid::now_v7());
    let mut req = d.request("PUT", &path, bad, Some("unknown-target"));
    req.headers_mut().insert(
        "if-match",
        source.json()["version"].to_string().parse().unwrap(),
    );
    let response = c.request(s.lan, req).await.unwrap();
    assert_eq!(response.status, 422);
    assert!(response.json().to_string().contains("target_member_id"));
    s.stop().await;
}
