use porchlight_api::testing::{TestClient, TestOptions, TestServer};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;

#[sqlx::test(migrations = "../db/migrations")]
async fn comments_versions_mentions_soft_delete_and_single_connection(pool: PgPool) {
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
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let item = c
        .request(
            s.lan,
            d.request("POST", "/v1/items", json!({"title":"Collab"}), None),
        )
        .await
        .unwrap()
        .json();
    let path = format!("/v1/items/{}/comments", item["id"].as_str().unwrap());
    let r = c
        .request(
            s.lan,
            d.request(
                "POST",
                &path,
                json!({"body":"Hello @Admin"}),
                Some("comment"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 201);
    assert_eq!(r.json()["mentions"][0], s.init.admin_member_id.to_string());
    let replay = c
        .request(
            s.lan,
            d.request(
                "POST",
                &path,
                json!({"body":"Hello @Admin"}),
                Some("comment"),
            ),
        )
        .await
        .unwrap();
    assert_eq!(replay.body, r.body);
    let bad = c
        .request(
            s.lan,
            d.request("POST", &path, json!({"body":"@nonexistent"}), None),
        )
        .await
        .unwrap();
    assert_eq!(bad.status, 201);
    assert_eq!(bad.json()["mentions"], json!([]));
    let id = r.json()["id"].as_str().unwrap().to_owned();
    let p = format!("/v1/comments/{id}");
    let missing = c
        .request(
            s.lan,
            d.request("PATCH", &p, json!({"body":"Edited"}), None),
        )
        .await
        .unwrap();
    assert_eq!(missing.status, 428);
    let mut req = d.request("PATCH", &p, json!({"body":"Edited"}), None);
    req.headers_mut().insert("if-match", "1".parse().unwrap());
    let edited = c.request(s.lan, req).await.unwrap();
    assert_eq!(edited.status, 200);
    let mut req = d.request("DELETE", &p, json!({}), None);
    req.headers_mut().insert("if-match", "1".parse().unwrap());
    let stale = c.request(s.lan, req).await.unwrap();
    assert_eq!(stale.status, 409);
    assert_eq!(stale.json()["current"]["body"], "Edited");
    let mut req = d.request("DELETE", &p, json!({}), None);
    req.headers_mut().insert("if-match", "2".parse().unwrap());
    assert_eq!(c.request(s.lan, req).await.unwrap().status, 204);
    let list = c
        .request(s.lan, d.request("GET", &path, json!(null), None))
        .await
        .unwrap();
    assert_eq!(list.json()["items"].as_array().unwrap().len(), 1);
    assert_eq!(list.json()["items"][0]["body"], "@nonexistent");
    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM porchlight.outbox WHERE kind='comment.changed'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(events, 4);
    s.stop().await;
}

fn upload(
    d: &porchlight_api::testing::TestDevice,
    path: &str,
    key: &str,
    name: &str,
    mime: &str,
    bytes: &[u8],
) -> hyper::Request<http_body_util::Full<hyper::body::Bytes>> {
    let mut body=format!("--s7b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: {mime}\r\n\r\n").into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n--s7b--\r\n");
    let mut req = d
        .request("POST", path, json!({}), Some(key))
        .map(|_| http_body_util::Full::new(hyper::body::Bytes::from(body)));
    req.headers_mut().insert(
        "content-type",
        "multipart/form-data; boundary=s7b".parse().unwrap(),
    );
    req
}
#[sqlx::test(migrations = "../db/migrations")]
async fn upload_dedupe_download_and_30_day_gc_lease(pool: PgPool) {
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
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let item = c
        .request(
            s.lan,
            d.request("POST", "/v1/items", json!({"title":"Blobs"}), None),
        )
        .await
        .unwrap()
        .json();
    let path = format!("/v1/items/{}/attachments", item["id"].as_str().unwrap());
    let r = c
        .request(
            s.lan,
            upload(
                &d,
                &path,
                "u1",
                "../notes.txt",
                "text/plain",
                b"hello attachment",
            ),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["original_name"], "notes.txt");
    let replay = c
        .request(
            s.lan,
            upload(
                &d,
                &path,
                "u1",
                "../notes.txt",
                "text/plain",
                b"hello attachment",
            ),
        )
        .await
        .unwrap();
    assert_eq!(r.body, replay.body);
    let second = c
        .request(
            s.lan,
            upload(
                &d,
                &path,
                "u2",
                "notes.txt",
                "text/plain",
                b"hello attachment",
            ),
        )
        .await
        .unwrap();
    assert_eq!(second.status, 201);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.blobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let id = r.json()["id"].as_str().unwrap().to_owned();
    let get = c
        .request(
            s.lan,
            d.request("GET", &format!("/v1/attachments/{id}"), json!(null), None),
        )
        .await
        .unwrap();
    assert_eq!(get.status, 200);
    assert_eq!(get.body.as_ref(), b"hello attachment");
    assert_eq!(get.headers["x-content-type-options"], "nosniff");
    assert_eq!(get.headers["content-type"], "text/plain; charset=utf-8");
    let invalid = c
        .request(
            s.lan,
            upload(
                &d,
                &path,
                "bad",
                "file.exe",
                "application/octet-stream",
                b"MZbinary",
            ),
        )
        .await
        .unwrap();
    assert_eq!(invalid.status, 415);
    let now = chrono::Utc::now();
    let lease: uuid::Uuid =
        sqlx::query_scalar("SELECT porchlight.pl_take_blob_lease('test',interval '24 hours',$1)")
            .bind(now + chrono::Duration::days(31))
            .fetch_one(&pool)
            .await
            .unwrap();
    for id in [id, second.json()["id"].as_str().unwrap().to_owned()] {
        let mut req = d.request("DELETE", &format!("/v1/attachments/{id}"), json!({}), None);
        req.headers_mut().insert("if-match", "1".parse().unwrap());
        assert_eq!(c.request(s.lan, req).await.unwrap().status, 204);
    }
    assert_eq!(
        porchlight_api::blobs::gc(
            &pool,
            &pool,
            &s.state.settings.data_dir,
            now + chrono::Duration::days(29)
        )
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        porchlight_api::blobs::gc(
            &pool,
            &pool,
            &s.state.settings.data_dir,
            now + chrono::Duration::days(31)
        )
        .await
        .unwrap(),
        0
    );
    sqlx::query("DELETE FROM porchlight.blob_leases WHERE id=$1")
        .bind(lease)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        porchlight_api::blobs::gc(
            &pool,
            &pool,
            &s.state.settings.data_dir,
            now + chrono::Duration::days(31)
        )
        .await
        .unwrap(),
        1
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.blobs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    s.stop().await;
}
#[test]
fn comment_clock_and_mention_resolution() {
    use porchlight_api::{
        collaboration::{Comment, editable, parse_mentions},
        error::Problem,
        extract::Caller,
    };
    let id = uuid::Uuid::now_v7();
    let other = uuid::Uuid::now_v7();
    let now = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let caller = Caller::Member {
        member_id: id,
        user_id: id,
        role: "user".into(),
        device_id: id,
        session_id: id,
        household_id: id,
        client_addr: None,
        request_id: id,
    };
    let row = Comment {
        id,
        item_id: id,
        author_id: Some(id),
        author_token_id: None,
        body: "x".into(),
        mentions: vec![],
        created_at: now,
        edited_at: None,
        deleted_at: None,
        version: 1,
    };
    assert!(editable(&row, &caller, now + chrono::Duration::minutes(15)).is_ok());
    assert_eq!(
        editable(
            &row,
            &caller,
            now + chrono::Duration::minutes(15) + chrono::Duration::microseconds(1)
        )
        .err()
        .unwrap()
        .problem(),
        Problem::CommentEditExpired
    );
    assert_eq!(
        parse_mentions(
            "Hi @Alice @\"Bob Smith\" and alice@example.org",
            &[(id, "Alice".into()), (other, "Bob Smith".into())]
        )
        .unwrap()
        .len(),
        2
    );
    assert!(
        parse_mentions("@Alice", &[(id, "Alice".into()), (other, "Alice".into())])
            .unwrap()
            .is_empty()
    );
    assert!(parse_mentions(&"x".repeat(10001), &[]).is_err());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn every_collaboration_matrix_row_runs_through_http(pool: PgPool) {
    use porchlight_api::{
        permissions::{self, Relationship},
        routes::collaboration::ENDPOINTS,
        testing::{HouseholdFixture, agent_request},
    };
    use porchlight_db::{
        items::{self, ItemWrite, Relationships},
        tx::{Actor, begin_as},
    };
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let actor = Actor {
        member_id: Some(f.admin.member_id),
        device_id: Some(f.admin.device.device_id),
        request_id: uuid::Uuid::now_v7(),
        agent_label: None,
        token_id: None,
    };
    let image = porchlight_core::ingest::ingest(
        include_bytes!("../../../core/tests/fixtures/progressive-camera.jpg"),
        Some("camera.jpg"),
        None,
        &porchlight_core::ingest::Limits::production(),
    )
    .unwrap();
    porchlight_api::blobs::durable_write(
        &f.server.state.settings.data_dir,
        &image.sha256,
        &image.bytes,
    )
    .unwrap();
    let mut pats = Vec::new();
    for scope in porchlight_api::permissions::PatScope::ALL {
        let scopes = scope.scopes();
        let refs = scopes.iter().map(String::as_str).collect::<Vec<_>>();
        let (id, token) = porchlight_api::testing::trust::seed_pat(&pool, &f.server, &refs).await;
        pats.push((scope, id, token));
    }
    for row in permissions::matrix_rows(ENDPOINTS) {
        let who = if matches!(
            row.relationship,
            Relationship::SuperAdmin | Relationship::Agent
        ) {
            &f.admin
        } else {
            &f.users[0]
        };
        let owner = if matches!(row.relationship, Relationship::Owner) {
            who.member_id
        } else {
            f.users[1].member_id
        };
        let mut tx = begin_as(&f.server.state.pool, &actor).await.unwrap();
        let item = items::create(
            &mut tx,
            f.server.init.household_id,
            &ItemWrite {
                title: "ACL collab".into(),
                owner_id: owner,
                visibility: row.visibility.as_str().into(),
                subject_member_id: matches!(row.relationship, Relationship::Subject)
                    .then_some(who.member_id),
                ..Default::default()
            },
            &Relationships {
                assignees: if matches!(row.relationship, Relationship::Assignee) {
                    vec![who.member_id]
                } else {
                    vec![]
                },
                viewers: if matches!(row.relationship, Relationship::Viewer) {
                    vec![who.member_id]
                } else {
                    vec![]
                },
                ..Default::default()
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap()
        .item_id;
        let pat = row
            .pat_scope
            .map(|scope| pats.iter().find(|p| p.0 == scope).unwrap());
        let (resource_member, resource_token) = if row.is_self && pat.is_some() {
            (None, Some(pat.unwrap().1))
        } else {
            (
                Some(if row.is_self {
                    who.member_id
                } else {
                    f.users[1].member_id
                }),
                None,
            )
        };
        let comment:uuid::Uuid=sqlx::query_scalar("INSERT INTO porchlight.comments(household_id,item_id,author_id,author_token_id,body) VALUES($1,$2,$3,$4,'Seed') RETURNING id").bind(f.server.init.household_id).bind(item).bind(resource_member).bind(resource_token).fetch_one(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO porchlight.blobs(household_id,sha256,size,content_type) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING").bind(f.server.init.household_id).bind(&image.sha256).bind(image.bytes.len() as i64).bind(image.content_type).execute(&mut *tx).await.unwrap();
        let attachment:uuid::Uuid=sqlx::query_scalar("INSERT INTO porchlight.attachments(household_id,item_id,blob_sha256,thumbnail_sha256,original_name,uploader_id,uploader_token_id,comment_id) VALUES($1,$2,$3,$3,'camera.jpg',$4,$5,$6) RETURNING id").bind(f.server.init.household_id).bind(item).bind(&image.sha256).bind(resource_member).bind(resource_token).bind((row.endpoint.case == "comment-attachment").then_some(comment)).fetch_one(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        let id = if row.endpoint.path.starts_with("/v1/items") {
            item
        } else if row.endpoint.path.starts_with("/v1/comments") {
            comment
        } else {
            attachment
        };
        let path = row.endpoint.path.replace("{id}", &id.to_string());
        let mut req = if row.endpoint.handler.starts_with("upload") {
            upload(
                &who.device,
                &path,
                &uuid::Uuid::now_v7().to_string(),
                "notes.txt",
                "text/plain",
                b"matrix",
            )
        } else {
            who.device.request(
                row.endpoint.method,
                &path,
                if row.endpoint.method == "POST" || row.endpoint.method == "PATCH" {
                    json!({"body":"Matrix"})
                } else {
                    json!({})
                },
                None,
            )
        };
        if row.endpoint.method == "PATCH" || row.endpoint.method == "DELETE" {
            req.headers_mut().insert("if-match", "1".parse().unwrap());
        }
        let r = if matches!(row.relationship, Relationship::Agent) {
            agent_request(&f.server.agent_socket, Some(pat.unwrap().2.as_str()), req).await
        } else {
            TestClient::new(&f.server.ca_pem, Some(&who.device))
                .request(f.server.lan, req)
                .await
                .unwrap()
        };
        assert_eq!(
            r.status,
            row.status,
            "{} {} {} {:?} {:?} self={} scope={:?}",
            row.endpoint.method,
            row.endpoint.path,
            row.endpoint.case,
            row.relationship,
            row.visibility,
            row.is_self,
            row.pat_scope
        );
    }
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn http_ingest_limits_image_strip_and_thumbnail(pool: PgPool) {
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
    let c = TestClient::new(&s.ca_pem, Some(&d));
    let item = c
        .request(
            s.lan,
            d.request("POST", "/v1/items", json!({"title":"Image"}), None),
        )
        .await
        .unwrap()
        .json();
    let path = format!("/v1/items/{}/attachments", item["id"].as_str().unwrap());
    let small = porchlight_core::ingest::ingest(
        include_bytes!("../../../core/tests/fixtures/progressive-camera.jpg"),
        Some("camera.jpg"),
        None,
        &porchlight_core::ingest::Limits::production(),
    )
    .unwrap()
    .thumbnail
    .unwrap()
    .bytes;
    let mut image = vec![0xff, 0xd8, 0xff, 0xe1, 0, 12];
    image.extend_from_slice(b"Exif\0\0GPS\0");
    image.extend_from_slice(&small[2..]);
    let r = c
        .request(
            s.lan,
            upload(&d, &path, "image", "camera.jpg", "image/jpeg", &image),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 201);
    assert_eq!(r.json()["has_thumbnail"], true);
    let id = r.json()["id"].as_str().unwrap().to_owned();
    let get = c
        .request(
            s.lan,
            d.request("GET", &format!("/v1/attachments/{id}"), json!(null), None),
        )
        .await
        .unwrap();
    assert_eq!(get.status, 200);
    assert!(!get.body.windows(4).any(|w| w == b"Exif"));
    assert!(!get.body.windows(3).any(|w| w == b"GPS"));
    let thumb = c
        .request(
            s.lan,
            d.request(
                "GET",
                &format!("/v1/attachments/{id}/thumbnail"),
                json!(null),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(thumb.status, 200);
    assert_eq!(thumb.headers["x-content-type-options"], "nosniff");
    let mismatch = c
        .request(
            s.lan,
            upload(&d, &path, "mismatch", "text.txt", "text/plain", &image),
        )
        .await
        .unwrap();
    assert_eq!(mismatch.status, 422);
    let too_big = c
        .request(
            s.lan,
            upload(
                &d,
                &path,
                "large",
                "large.txt",
                "text/plain",
                &vec![b'x'; 25 * 1024 * 1024 + 1],
            ),
        )
        .await
        .unwrap();
    assert_eq!(too_big.status, 413);
    let broken = c
        .request(
            s.lan,
            upload(
                &d,
                &path,
                "broken",
                "broken.png",
                "image/png",
                b"\x89PNG\r\n\x1a\n",
            ),
        )
        .await
        .unwrap();
    assert_eq!(broken.status, 422);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn crash_orphan_collection_and_temp_never_downloadable(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let ingested = porchlight_core::ingest::ingest(
        b"orphan",
        Some("notes.txt"),
        None,
        &porchlight_core::ingest::Limits::production(),
    )
    .unwrap();
    let root = &s.state.settings.data_dir;
    porchlight_api::blobs::durable_write(root, &ingested.sha256, &ingested.bytes).unwrap();
    let path = porchlight_api::blobs::blob_path(root, &ingested.sha256).unwrap();
    assert!(path.starts_with(root.join("blobs")));
    assert_eq!(std::fs::read(&path).unwrap(), b"orphan");
    assert!(porchlight_api::blobs::blob_path(root, "../anything").is_err());
    let now = chrono::Utc::now();
    let old = std::time::SystemTime::from(now - chrono::Duration::days(31));
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(old))
        .unwrap();
    let temp = path.parent().unwrap().join(".crash.tmp");
    std::fs::write(&temp, b"partial").unwrap();
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let c = TestClient::new(&s.ca_pem, Some(&d));
    for name in [".crash.tmp", ingested.sha256.as_str()] {
        let r = c
            .request(
                s.lan,
                d.request("GET", &format!("/v1/attachments/{name}"), json!(null), None),
            )
            .await
            .unwrap();
        assert_ne!(
            r.status, 200,
            "neither a temp name nor a hash is a download ID"
        );
        assert_ne!(r.body.as_ref(), b"partial");
    }
    std::fs::File::options()
        .write(true)
        .open(&temp)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(old))
        .unwrap();
    assert_eq!(
        porchlight_api::blobs::gc(&s.state.pool, &pool, root, now)
            .await
            .unwrap(),
        1
    );
    assert!(!path.exists());
    assert!(!temp.exists());
    s.stop().await;
}
