//! Transaction-local collaboration integration hooks and injected clock boundaries.
use porchlight_api::{
    collaboration::{self, Comment},
    extract::Caller,
    testing::{HouseholdFixture, TestClient, TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn comment_http_edit_window_uses_injected_clock(pool: PgPool) {
    let clock = std::sync::Arc::new(std::sync::Mutex::new(chrono::Utc::now()));
    let read_clock = clock.clone();
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(move || *read_clock.lock().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let item = c
        .request(
            s.lan,
            d.request("POST", "/v1/items", json!({"title":"Window"}), None),
        )
        .await
        .unwrap()
        .json();
    let path = format!("/v1/items/{}/comments", item["id"].as_str().unwrap());
    let created = c
        .request(
            s.lan,
            d.request("POST", &path, json!({"body":"Original"}), None),
        )
        .await
        .unwrap();
    assert_eq!(created.status, 201);
    let row: Comment = serde_json::from_slice(&created.body).unwrap();
    let path = format!("/v1/comments/{}", row.id);
    *clock.lock().unwrap() = row.created_at + chrono::Duration::minutes(15);
    let mut req = d.request("PATCH", &path, json!({"body":"Boundary"}), None);
    req.headers_mut().insert("if-match", "1".parse().unwrap());
    let edited = c.request(s.lan, req).await.unwrap();
    assert_eq!(edited.status, 200);
    *clock.lock().unwrap() += chrono::Duration::microseconds(1);
    let mut req = d.request("PATCH", &path, json!({"body":"Too late"}), None);
    req.headers_mut().insert("if-match", "2".parse().unwrap());
    let expired = c.request(s.lan, req).await.unwrap();
    assert_eq!(expired.status, 403);
    assert_eq!(
        expired.json()["type"],
        "urn:porchlight:problem:comment-edit-expired"
    );
    let stored: (String, i32) =
        sqlx::query_as("SELECT body,version FROM porchlight.comments WHERE id=$1")
            .bind(row.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, ("Boundary".into(), 2));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn notification_planning_search_and_snapshot_follow_live_acl(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let alice = &f.users[0];
    let client = TestClient::new(&f.server.ca_pem, Some(&alice.device));
    let item = client
        .request(
            f.server.lan,
            alice.device.request(
                "POST",
                "/v1/items",
                json!({"title":"Private","visibility":"private","assignees":[f.admin.member_id]}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(item.status, 201);
    let id: Uuid = item.json()["id"].as_str().unwrap().parse().unwrap();
    let response = client
        .request(
            f.server.lan,
            alice.device.request(
                "POST",
                &format!("/v1/items/{id}/comments"),
                json!({"body":"Searchword @Admin @Bob @Alice"}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 201);
    let comment: Comment = serde_json::from_slice(&response.body).unwrap();
    let caller = |user: &porchlight_api::testing::FixtureUser, role: &str| Caller::Member {
        member_id: user.member_id,
        user_id: user.user_id,
        role: role.into(),
        device_id: user.device.device_id,
        session_id: Uuid::nil(),
        household_id: f.server.init.household_id,
        client_addr: None,
        request_id: Uuid::now_v7(),
    };
    let alice_c = caller(alice, "user");
    let admin_c = caller(&f.admin, "super_admin");
    let mut conn = f.server.state.pool.acquire().await.unwrap();
    let recipients = collaboration::comment_notification_recipients(
        &mut conn,
        &alice_c,
        id,
        &comment,
        true,
        &[],
    )
    .await
    .unwrap();
    assert_eq!(
        recipients,
        vec![
            (f.admin.member_id, "comment"),
            (f.admin.member_id, "mention")
        ]
    );
    assert!(
        collaboration::comment_notification_recipients(
            &mut conn,
            &alice_c,
            id,
            &comment,
            false,
            &comment.mentions
        )
        .await
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        collaboration::snapshot_comments_s4b(&mut conn, &admin_c, &[id])
            .await
            .unwrap()
            .len(),
        1
    );
    drop(conn);
    // Authoritative ACL loss must apply immediately, even without background derivation.
    let mut req = alice.device.request(
        "PUT",
        &format!("/v1/items/{id}/assignees"),
        json!({"assignees":[]}),
        None,
    );
    req.headers_mut().insert(
        "if-match",
        item.json()["version"].to_string().parse().unwrap(),
    );
    let removed = client.request(f.server.lan, req).await.unwrap();
    assert_eq!(removed.status, 200);
    let mut conn = f.server.state.pool.acquire().await.unwrap();
    assert!(
        collaboration::comment_notification_recipients(
            &mut conn,
            &alice_c,
            id,
            &comment,
            true,
            &[]
        )
        .await
        .unwrap()
        .is_empty()
    );
    assert_eq!(
        collaboration::snapshot_comments_s4b(&mut conn, &admin_c, &[id])
            .await
            .err()
            .unwrap()
            .status(),
        axum::http::StatusCode::NOT_FOUND
    );
    drop(conn);
    f.server.stop().await;
}
