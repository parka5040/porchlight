use porchlight_api::testing::{TestClient, TestOptions, TestServer, trust};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;

fn calendar(bad: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:bad\r\nDTSTART:20261012T090000Z\r\n{bad}\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:good\r\nDTSTART:20261013T090000Z\r\nDTEND:20261013T100000Z\r\nSUMMARY:Good\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    )
}

#[sqlx::test(migrations = "../db/migrations")]
async fn import_reports_each_invalid_event_without_whole_import_failure(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(|| "2026-10-06T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let cases = [
        format!("DTEND:20261012T100000Z\r\nSUMMARY:{}", "s".repeat(201)),
        format!(
            "DTEND:20261012T100000Z\r\nDESCRIPTION:{}",
            "d".repeat(20001)
        ),
        format!("DTEND:20261012T100000Z\r\nLOCATION:{}", "l".repeat(501)),
        "DTEND:20261012T100000\r\nSUMMARY:Bad".into(),
        "DTEND:20261012T080000Z\r\nSUMMARY:Bad end".into(),
    ];
    for (i, bad) in cases.iter().enumerate() {
        let response = c.request(s.lan, d.request("POST", "/v1/imports/ics", json!({"content":calendar(bad),"member_id":s.init.admin_member_id,"scope":format!("invalid-{i}")}), Some(&format!("import-{i}")))).await.unwrap();
        assert_eq!(
            response.status, 200,
            "event validation is local to the event"
        );
        assert_eq!(response.json()["created"], 1);
        assert!(
            response.json()["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|w| w["code"] == "InvalidEvent" && w["uid"] == "bad")
        );
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn feed_name_bounds_and_exact_username_precedence(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let body = json!({"name":"a".repeat(196),"url":"https://example.invalid/calendar"});
    let source = c
        .request(
            s.lan,
            d.request("POST", "/v1/feeds", body.clone(), Some("bound-ok")),
        )
        .await
        .unwrap();
    assert_eq!(source.status, 201);
    for (i, name) in ["a".repeat(197), "a".repeat(200)].into_iter().enumerate() {
        for method in ["POST", "PUT"] {
            let mut body = body.clone();
            body["name"] = json!(name);
            let path = if method == "PUT" {
                format!("/v1/feeds/{}", source.json()["id"].as_str().unwrap())
            } else {
                "/v1/feeds".into()
            };
            let mut req = d.request(method, &path, body, Some(&format!("name-{method}-{i}")));
            req.headers_mut().insert(
                "if-match",
                source.json()["version"].to_string().parse().unwrap(),
            );
            let response = c.request(s.lan, req).await.unwrap();
            assert_eq!(response.status, 422);
            assert!(
                response.json()["errors"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e["field"] == "name")
            );
        }
    }
    trust::user_named(&pool, &s, "other", "admin").await;
    let (_, write) = trust::seed_pat(&pool, &s, &["write"]).await;
    for token in [s.bootstrap_token(), write.as_str()] {
        let response = porchlight_api::testing::agent_request(
            &s.agent_socket,
            Some(token),
            trust::req(
                "GET",
                "/v1/admin/feeds/target?username=admin",
                json!({}),
                "exact-username",
            ),
        )
        .await;
        assert_eq!(
            response.status, 200,
            "exact username beats another display name even for a self-only write PAT"
        );
        assert_eq!(response.json()["member_id"], json!(s.init.admin_member_id));
    }
    s.stop().await;
}

#[test]
fn import_visibility_query_is_source_visibility_enum() {
    let doc = serde_json::to_value(porchlight_api::openapi::document()).unwrap();
    let params = doc["paths"]["/v1/imports/ics"]["post"]["parameters"]
        .as_array()
        .unwrap();
    let schema = &params.iter().find(|p| p["name"] == "visibility").unwrap()["schema"];
    assert!(schema.to_string().contains("SourceVisibility"));
}
