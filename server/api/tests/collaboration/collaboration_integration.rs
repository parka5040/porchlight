//! S4b integration: immutable events, authoritative SQL ACL and merged reads/replays.
use porchlight_api::{
    blobs,
    testing::{HouseholdFixture, TestClient, TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

#[test]
fn filename_general_categories_are_removed() {
    assert_eq!(
        blobs::sanitize_name("a\u{2028}\u{2029}\u{e0020}\u{0007}b.txt", "text/plain"),
        "ab.txt"
    );
    assert_eq!(
        blobs::sanitize_name("家庭 café.txt", "text/plain"),
        "家庭 café.txt"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn comment_events_and_merged_reads_follow_acl(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let alice = &f.users[0];
    let bob = &f.users[1];
    let client = TestClient::new(&f.server.ca_pem, Some(&alice.device));
    let item = client
        .request(
            f.server.lan,
            alice.device.request(
                "POST",
                "/v1/items",
                json!({"title":"Needle","visibility":"private","assignees":[f.admin.member_id]}),
                None,
            ),
        )
        .await
        .unwrap()
        .json();
    let id: Uuid = item["id"].as_str().unwrap().parse().unwrap();
    let path = format!("/v1/items/{id}/comments");
    let comment_req = || {
        alice.device.request(
            "POST",
            &path,
            json!({"body":"Needle @Admin @Bob @Alice"}),
            Some("integrated-comment"),
        )
    };
    let created = client.request(f.server.lan, comment_req()).await.unwrap();
    assert_eq!(created.status, 201);
    let row = created.json();
    let comment: Uuid = row["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        client
            .request(f.server.lan, comment_req())
            .await
            .unwrap()
            .body,
        created.body
    );
    let events: Vec<(Uuid,String,Uuid,Uuid,Value)> = sqlx::query_as("select recipient_member_id,kind,item_id,actor_member_id,payload from porchlight.notification_events where entity_type='comment' and entity_id=$1 order by kind")
        .bind(comment).fetch_all(&pool).await.unwrap();
    assert_eq!(
        events.len(),
        2,
        "one comment + one mention; replay never emits, hidden Bob/self Alice excluded"
    );
    assert!(events.iter().all(
        |(member, _, scope, actor, payload)| *member == f.admin.member_id
            && *scope == id
            && *actor == alice.member_id
            && payload["item_id"] == id.to_string()
    ));
    assert_eq!(
        events.iter().map(|v| v.1.as_str()).collect::<Vec<_>>(),
        vec!["comment", "mention"]
    );
    let patch_path = format!("/v1/comments/{comment}");
    let mut patch = alice.device.request(
        "PATCH",
        &patch_path,
        json!({"body":"Needle again @Admin @Bob"}),
        None,
    );
    patch.headers_mut().insert("if-match", "1".parse().unwrap());
    assert_eq!(
        client.request(f.server.lan, patch).await.unwrap().status,
        200
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "select count(*) from porchlight.notification_events where entity_id=$1"
        )
        .bind(comment)
        .fetch_one(&pool)
        .await
        .unwrap(),
        2,
        "unchanged mention does not re-notify"
    );
    let upload_body = b"--int\r\nContent-Disposition: form-data; name=\"file\"; filename=\"x.txt\"\r\nContent-Type: text/plain\r\n\r\nx\r\n--int--\r\n";
    let mut upload = alice.device.request(
        "POST",
        &format!("/v1/items/{id}/attachments"),
        json!({}),
        None,
    );
    upload.headers_mut().insert(
        "content-type",
        "multipart/form-data; boundary=int".parse().unwrap(),
    );
    *upload.body_mut() = http_body_util::Full::new(hyper::body::Bytes::from_static(upload_body));
    let attached = client.request(f.server.lan, upload).await.unwrap();
    assert_eq!(attached.status, 201);
    for user in [&f.admin, bob] {
        let c = TestClient::new(&f.server.ca_pem, Some(&user.device));
        let snapshot = c
            .request(
                f.server.lan,
                user.device.request("GET", "/v1/snapshot", json!({}), None),
            )
            .await
            .unwrap()
            .json();
        let expected = usize::from(user.member_id == f.admin.member_id);
        assert_eq!(
            snapshot["comments"]
                .as_array()
                .expect("snapshot comments")
                .len(),
            expected
        );
        assert_eq!(
            snapshot["attachments"]
                .as_array()
                .expect("snapshot attachments")
                .len(),
            expected
        );
        let first = c
            .request(
                f.server.lan,
                user.device
                    .request("GET", "/v1/search?q=Needle&limit=1", json!({}), None),
            )
            .await
            .unwrap()
            .json();
        if expected == 0 {
            assert!(first["items"].as_array().unwrap().is_empty());
            continue;
        }
        assert_eq!(first["items"][0]["entity_type"], "comment");
        let cursor = first["next_cursor"].as_str().expect("composite cursor");
        let next = c
            .request(
                f.server.lan,
                user.device.request(
                    "GET",
                    &format!("/v1/search?q=Needle&limit=1&cursor={cursor}"),
                    json!({}),
                    None,
                ),
            )
            .await
            .unwrap()
            .json();
        assert_eq!(next["items"][0]["entity_type"], "item");
        assert!(next["next_cursor"].is_null());
    }
    let tickles: Vec<(String,Value)> = sqlx::query_as("select kind,payload from porchlight.outbox where kind in ('comment.changed','attachment.changed') and household_id=$1").bind(f.server.init.household_id).fetch_all(&pool).await.unwrap();
    assert!(tickles.iter().any(|v| v.0 == "attachment.changed"));
    assert!(
        tickles.iter().all(|v| v.1 == json!({"item_id":id})),
        "stream scope contains only item_id"
    );
    // ACL loss applies to every contributed source without any worker/derive dependency.
    let mut remove = alice.device.request(
        "PUT",
        &format!("/v1/items/{id}/assignees"),
        json!({"assignees":[]}),
        None,
    );
    remove
        .headers_mut()
        .insert("if-match", item["version"].to_string().parse().unwrap());
    assert_eq!(
        client.request(f.server.lan, remove).await.unwrap().status,
        200
    );
    let admin = TestClient::new(&f.server.ca_pem, Some(&f.admin.device));
    let snapshot = admin
        .request(
            f.server.lan,
            f.admin
                .device
                .request("GET", "/v1/snapshot", json!({}), None),
        )
        .await
        .unwrap()
        .json();
    assert!(snapshot["comments"].as_array().unwrap().is_empty());
    assert!(snapshot["attachments"].as_array().unwrap().is_empty());
    let search = admin
        .request(
            f.server.lan,
            f.admin
                .device
                .request("GET", "/v1/search?q=Needle", json!({}), None),
        )
        .await
        .unwrap()
        .json();
    assert!(search["items"].as_array().unwrap().is_empty());
    // New mentions on edits notify only newly introduced live readers, never repeat comment events.
    let mut grant = alice.device.request(
        "PUT",
        &format!("/v1/items/{id}/viewers"),
        json!({"viewers":[bob.member_id]}),
        None,
    );
    let current_version: i32 =
        sqlx::query_scalar("SELECT version FROM porchlight.items WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    grant
        .headers_mut()
        .insert("if-match", current_version.to_string().parse().unwrap());
    assert_eq!(
        client.request(f.server.lan, grant).await.unwrap().status,
        200
    );
    // Private viewers do not grant access: switch to selected with current version.
    let current_version: i32 =
        sqlx::query_scalar("SELECT version FROM porchlight.items WHERE id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut select = alice.device.request(
        "PATCH",
        &format!("/v1/items/{id}"),
        json!({"visibility":"selected"}),
        None,
    );
    select
        .headers_mut()
        .insert("if-match", current_version.to_string().parse().unwrap());
    assert_eq!(
        client.request(f.server.lan, select).await.unwrap().status,
        200
    );
    for (version, body) in [(2, "Needle no mentions"), (3, "Needle @Bob")] {
        let mut edit = alice
            .device
            .request("PATCH", &patch_path, json!({"body":body}), None);
        edit.headers_mut()
            .insert("if-match", version.to_string().parse().unwrap());
        assert_eq!(
            client.request(f.server.lan, edit).await.unwrap().status,
            200
        );
    }
    let added:Vec<(Uuid,String)> = sqlx::query_as("SELECT recipient_member_id,kind FROM porchlight.notification_events WHERE entity_id=$1 AND recipient_member_id=$2")
        .bind(comment).bind(bob.member_id).fetch_all(&pool).await.unwrap();
    assert_eq!(added, vec![(bob.member_id, "mention".into())]);
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn upload_replay_never_waits_for_ingest(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            db_pool_size: 1,
            upload_limits: blobs::UploadLimits {
                permit_wait: std::time::Duration::from_millis(30),
                ..Default::default()
            },
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
            d.request("POST", "/v1/items", json!({"title":"Replay"}), None),
        )
        .await
        .unwrap()
        .json();
    let path = format!("/v1/items/{}/attachments", item["id"].as_str().unwrap());
    let request = || {
        let mut req = d.request("POST", &path, json!({}), Some("busy-replay"));
        req.headers_mut().insert(
            "content-type",
            "multipart/form-data; boundary=int".parse().unwrap(),
        );
        *req.body_mut()=http_body_util::Full::new(hyper::body::Bytes::from_static(b"--int\r\nContent-Disposition: form-data; name=\"file\"; filename=\"x.txt\"\r\nContent-Type: text/plain\r\n\r\nx\r\n--int--\r\n"));
        req
    };
    let first = c.request(s.lan, request()).await.unwrap();
    assert_eq!(first.status, 201);
    let held = s
        .state
        .ingest_slots
        .clone()
        .acquire_many_owned(2)
        .await
        .unwrap();
    let replay = c.request(s.lan, request()).await.unwrap();
    assert_eq!(
        replay.status, 201,
        "sealed replay does not require decoder admission"
    );
    assert_eq!(first.body, replay.body);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from porchlight.attachments")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    drop(held);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn shared_sql_visibility_agrees_with_rust_capabilities(pool: PgPool) {
    use porchlight_api::{collaboration, extract::Caller, items};
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let h = f.server.init.household_id;
    for role in ["user", "super_admin", "agent"] {
        let reader = if role == "user" {
            &f.users[0]
        } else {
            &f.admin
        };
        let other = if role == "user" {
            f.admin.member_id
        } else {
            f.users[0].member_id
        };
        let caller = if role == "agent" {
            Caller::Agent {
                token_id: Uuid::nil(),
                member_id: reader.member_id,
                label: "test".into(),
                scopes: vec!["admin".into()],
                household_id: h,
                request_id: Uuid::nil(),
            }
        } else {
            Caller::Member {
                member_id: reader.member_id,
                user_id: reader.user_id,
                role: role.into(),
                device_id: reader.device.device_id,
                session_id: Uuid::nil(),
                household_id: h,
                client_addr: None,
                request_id: Uuid::nil(),
            }
        };
        for visibility in ["household", "private", "selected"] {
            for relationship in ["owner", "assignee", "viewer", "subject", "other"] {
                let id:Uuid=sqlx::query_scalar("INSERT INTO porchlight.items(household_id,owner_id,subject_member_id,title,kind,visibility) VALUES($1,$2,$3,'Parity','todo',$4) RETURNING id")
                    .bind(h).bind(if relationship=="owner" {reader.member_id} else {other})
                    .bind(if relationship=="subject" {Some(reader.member_id)} else {None})
                    .bind(visibility).fetch_one(&pool).await.unwrap();
                if relationship == "assignee" {
                    sqlx::query("INSERT INTO porchlight.item_assignees(household_id,item_id,member_id) VALUES($1,$2,$3)").bind(h).bind(id).bind(reader.member_id).execute(&pool).await.unwrap();
                }
                if relationship == "viewer" {
                    sqlx::query("INSERT INTO porchlight.item_viewers(household_id,item_id,member_id) VALUES($1,$2,$3)").bind(h).bind(id).bind(reader.member_id).execute(&pool).await.unwrap();
                }
                let mut conn = f.server.state.pool.acquire().await.unwrap();
                let sql = collaboration::read_item(&mut conn, &caller, id).await;
                let rust = items::response(&mut conn, &caller, id, false, vec![]).await;
                let expected =
                    porchlight_api::permissions::read_allowed(role, visibility, relationship);
                assert_eq!(
                    sql.is_ok(),
                    expected,
                    "SQL {role}/{visibility}/{relationship}"
                );
                assert_eq!(
                    rust.is_ok(),
                    expected,
                    "Rust {role}/{visibility}/{relationship}"
                );
                if !expected {
                    assert_eq!(sql.unwrap_err().status(), axum::http::StatusCode::NOT_FOUND);
                }
            }
        }
    }
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn collaboration_snapshot_uses_the_same_mvcc_view_and_cursor(pool: PgPool) {
    use porchlight_api::{
        routes,
        server::{ConnInfo, Transport},
    };
    use tower::ServiceExt;
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let h = s.init.household_id;
    let owner = s.init.admin_member_id;
    let id:Uuid=sqlx::query_scalar("INSERT INTO porchlight.items(household_id,owner_id,title,kind) VALUES($1,$2,'Existing parent','todo') RETURNING id")
        .bind(h).bind(owner).fetch_one(&pool).await.unwrap();
    porchlight_db::outbox::assign_stream_seqs(&s.state.pool, 1000)
        .await
        .unwrap();
    let mut writer = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE porchlight.comments IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *writer)
        .await
        .unwrap();
    let r = routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(d.fingerprint),
    }));
    let req = d.request("GET", "/v1/snapshot", json!({}), None);
    let task = tokio::spawn(async move { r.oneshot(req).await.unwrap() });
    tokio::time::timeout(std::time::Duration::from_secs(3),async {loop {
        let waiting:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation='porchlight.comments'::regclass AND NOT granted AND mode='AccessShareLock')").fetch_one(&pool).await.unwrap();
        if waiting {break;} tokio::task::yield_now().await;
    }}).await.expect("snapshot never reached its comment contribution");
    let comment:Uuid=sqlx::query_scalar("INSERT INTO porchlight.comments(household_id,item_id,author_id,body) VALUES($1,$2,$3,'Concurrent child') RETURNING id")
        .bind(h).bind(id).bind(owner).fetch_one(&mut *writer).await.unwrap();
    let hash = "a".repeat(64);
    sqlx::query("INSERT INTO porchlight.blobs(household_id,sha256,size,content_type) VALUES($1,$2,1,'text/plain')").bind(h).bind(&hash).execute(&mut *writer).await.unwrap();
    let attachment:Uuid=sqlx::query_scalar("INSERT INTO porchlight.attachments(household_id,item_id,comment_id,blob_sha256,original_name,uploader_id) VALUES($1,$2,$3,$4,'Concurrent.txt',$5) RETURNING id")
        .bind(h).bind(id).bind(comment).bind(&hash).bind(owner).fetch_one(&mut *writer).await.unwrap();
    writer.commit().await.unwrap();
    let response = task.await.unwrap();
    assert_eq!(response.status(), 200);
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    let snapshot: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(snapshot["items"].as_array().unwrap().len(), 1);
    assert!(snapshot["comments"].as_array().unwrap().is_empty());
    assert!(snapshot["attachments"].as_array().unwrap().is_empty());
    let cursor = snapshot["cursor"].as_i64().unwrap();
    porchlight_db::outbox::assign_stream_seqs(&s.state.pool, 1000)
        .await
        .unwrap();
    let events = porchlight_db::stream::read(&s.state.pool, h, owner, cursor)
        .await
        .unwrap();
    assert!(
        events
            .events
            .iter()
            .any(|e| e.entity_id == comment && e.seq > cursor)
    );
    assert!(
        events
            .events
            .iter()
            .any(|e| e.entity_id == attachment && e.seq > cursor)
    );
    s.stop().await;
}
