use porchlight_api::{
    pagination::{CursorKeys, PageParams},
    testing::{TestClient, TestServer, agent_request},
};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
#[test]
fn cursors_are_signed_context_bound_and_limits_bounded() {
    let keys = CursorKeys::new(&[1; 32]);
    let tuple = ("2026-10-05T00:00:00Z", uuid::Uuid::now_v7());
    let cursor = keys.encode("devices:household", &tuple).unwrap();
    let decoded: (String, uuid::Uuid) = keys.decode("devices:household", &cursor).unwrap();
    assert_eq!(decoded.0, tuple.0);
    assert!(
        keys.decode::<serde_json::Value>("devices:other", &cursor)
            .is_err()
    );
    assert!(
        CursorKeys::new(&[2; 32])
            .decode::<serde_json::Value>("devices:household", &cursor)
            .is_err()
    );
    let mut corrupt = cursor.into_bytes();
    corrupt[0] = if corrupt[0] == b'A' { b'B' } else { b'A' };
    assert!(
        keys.decode::<serde_json::Value>(
            "devices:household",
            std::str::from_utf8(&corrupt).unwrap()
        )
        .is_err()
    );
    assert_eq!(PageParams::default().limit().unwrap(), 50);
    for n in [0, 201] {
        assert!(
            PageParams {
                limit: Some(n),
                cursor: None
            }
            .limit()
            .is_err()
        );
    }
    assert_eq!(
        PageParams {
            limit: Some(200),
            cursor: None
        }
        .limit()
        .unwrap(),
        200
    );
}
#[sqlx::test(migrations = "../db/migrations")]
async fn device_pages_are_additive_and_have_no_duplicates(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    for _ in 0..3 {
        s.issue_device(s.init.admin_member_id, Platform::Desktop)
            .await;
    }
    let client = TestClient::new(&s.ca_pem, Some(&d));
    for admin in [false, true] {
        let mut cursor = None::<String>;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let mut path = if admin {
                "/v1/admin/devices?limit=2".to_owned()
            } else {
                "/v1/devices?limit=2".to_owned()
            };
            if let Some(c) = cursor {
                path.push_str("&cursor=");
                path.push_str(&c);
            }
            let req = d.request("GET", &path, json!({}), None);
            let r = if admin {
                agent_request(&s.agent_socket, Some(s.bootstrap_token()), req).await
            } else {
                client.request(s.lan, req).await.unwrap()
            };
            assert_eq!(r.status, 200);
            let body = r.json();
            assert_eq!(body["devices"], body["items"]);
            for row in body["items"].as_array().unwrap() {
                assert!(seen.insert(row["id"].as_str().unwrap().to_owned()));
            }
            cursor = body["next_cursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(seen.len(), 4);
    }
    s.stop().await;
}
