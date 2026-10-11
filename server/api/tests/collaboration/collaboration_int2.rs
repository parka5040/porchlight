//! Round-two merged search and independent collaboration permission dimensions.
use porchlight_api::{
    permissions::{self, PatScope, Relationship},
    routes::collaboration::ENDPOINTS,
    testing::{HouseholdFixture, TestClient, TestOptions},
};
use porchlight_core::access::Visibility;
use serde_json::json;
use sqlx::PgPool;

#[test]
fn collaboration_rows_cross_item_acl_resource_self_and_exact_pat_scope() {
    let rows = permissions::matrix_rows(ENDPOINTS);
    // Nine routes plus the distinct comment-attachment deletion branch.
    assert_eq!(rows.len(), 540);
    assert_eq!(ENDPOINTS.len(), 10);
    for endpoint in ENDPOINTS {
        for visibility in Visibility::ALL {
            for relationship in Relationship::ALL {
                let scopes = if matches!(relationship, Relationship::Agent) {
                    PatScope::ALL.into_iter().map(Some).collect::<Vec<_>>()
                } else {
                    vec![None]
                };
                for pat_scope in scopes {
                    for is_self in [true, false] {
                        let row = rows
                            .iter()
                            .find(|r| {
                                r.endpoint.handler == endpoint.handler
                                    && r.endpoint.case == endpoint.case
                                    && r.visibility == visibility
                                    && r.relationship.as_str() == relationship.as_str()
                                    && r.is_self == is_self
                                    && r.pat_scope == pat_scope
                            })
                            .expect("missing collaboration item/resource/scope row");
                        assert!(row.item_dimension);
                        assert_eq!(
                            row.context.is_owner,
                            matches!(relationship, Relationship::Owner)
                        );
                        if pat_scope == Some(PatScope::Read) && endpoint.method != "GET" {
                            assert_eq!(row.status, 403);
                        }
                        if relationship.as_str() == "other" && visibility == Visibility::Private {
                            assert_eq!(row.status, 404);
                        }
                        if endpoint.handler == "patch_comment"
                            && visibility == Visibility::Household
                            && pat_scope != Some(PatScope::Read)
                        {
                            assert_eq!(row.status, if is_self { 200 } else { 403 });
                        }
                        if endpoint.handler == "delete_attachment"
                            && endpoint.case.is_empty()
                            && matches!(relationship, Relationship::Owner)
                        {
                            assert_eq!(
                                row.status, 204,
                                "item editor can delete another uploader's attachment"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn combined_search_pages_comments_items_lists_and_binds_every_cursor(pool: PgPool) {
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
                json!({"title":"Mergeword", "visibility":"private"}),
                None,
            ),
        )
        .await
        .unwrap()
        .json();
    let comment = client
        .request(
            f.server.lan,
            alice.device.request(
                "POST",
                &format!("/v1/items/{}/comments", item["id"].as_str().unwrap()),
                json!({"body":"Mergeword comment"}),
                None,
            ),
        )
        .await
        .unwrap()
        .json();
    let list = client
        .request(
            f.server.lan,
            alice.device.request(
                "POST",
                "/v1/lists",
                json!({"name":"Mergeword list", "visibility":"private"}),
                None,
            ),
        )
        .await
        .unwrap()
        .json();
    assert!(list["id"].is_string(), "list fixture: {list}");
    let mut cursor = None;
    for (source, id) in [
        ("comment", &comment["id"]),
        ("item", &item["id"]),
        ("list", &list["id"]),
    ] {
        let path = match &cursor {
            Some(c) => format!("/v1/search?q=Mergeword&limit=1&cursor={c}"),
            None => "/v1/search?q=Mergeword&limit=1".into(),
        };
        let response = client
            .request(
                f.server.lan,
                alice.device.request("GET", &path, json!({}), None),
            )
            .await
            .unwrap();
        assert_eq!(response.status, 200);
        let page = response.json();
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        assert_eq!(page["items"][0]["entity_type"], source);
        assert_eq!(&page["items"][0]["id"], id);
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if source != "list" {
            let c = cursor.as_ref().expect("cross-source cursor");
            let wrong_query = client
                .request(
                    f.server.lan,
                    alice.device.request(
                        "GET",
                        &format!("/v1/search?q=Otherword&limit=1&cursor={c}"),
                        json!({}),
                        None,
                    ),
                )
                .await
                .unwrap();
            assert_eq!(wrong_query.status, 400);
            let wrong_caller = TestClient::new(&f.server.ca_pem, Some(&bob.device))
                .request(
                    f.server.lan,
                    bob.device.request(
                        "GET",
                        &format!("/v1/search?q=Mergeword&limit=1&cursor={c}"),
                        json!({}),
                        None,
                    ),
                )
                .await
                .unwrap();
            assert_eq!(wrong_caller.status, 400);
        }
    }
    assert!(cursor.is_none());
    let hidden = TestClient::new(&f.server.ca_pem, Some(&bob.device))
        .request(
            f.server.lan,
            bob.device
                .request("GET", "/v1/search?q=Mergeword", json!({}), None),
        )
        .await
        .unwrap()
        .json();
    assert!(hidden["items"].as_array().unwrap().is_empty());
    let mut request = alice.device.request(
        "DELETE",
        &format!("/v1/items/{}", item["id"].as_str().unwrap()),
        json!({}),
        None,
    );
    request
        .headers_mut()
        .insert("if-match", item["version"].to_string().parse().unwrap());
    assert_eq!(
        client.request(f.server.lan, request).await.unwrap().status,
        200
    );
    let remaining = client
        .request(
            f.server.lan,
            alice
                .device
                .request("GET", "/v1/search?q=Mergeword", json!({}), None),
        )
        .await
        .unwrap()
        .json();
    assert_eq!(remaining["items"].as_array().unwrap().len(), 1);
    assert_eq!(remaining["items"][0]["entity_type"], "list");
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn pat_comment_excludes_token_owner_from_comment_and_mention_events(pool: PgPool) {
    use porchlight_api::testing::{agent_request, trust};
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let admin = &f.admin;
    let bob = &f.users[1];
    let client = TestClient::new(&f.server.ca_pem, Some(&admin.device));
    let item = client
        .request(
            f.server.lan,
            admin.device.request(
                "POST",
                "/v1/items",
                json!({"title":"PAT comment", "assignees":[bob.member_id]}),
                None,
            ),
        )
        .await
        .unwrap()
        .json();
    let (_, token) = trust::seed_pat(&pool, &f.server, &["write"]).await;
    let response = agent_request(
        &f.server.agent_socket,
        Some(&token),
        admin.device.request(
            "POST",
            &format!("/v1/items/{}/comments", item["id"].as_str().unwrap()),
            json!({"body":"@Admin @Bob"}),
            None,
        ),
    )
    .await;
    assert_eq!(response.status, 201);
    let comment = response.json();
    assert!(comment["author_id"].is_null());
    assert!(comment["author_token_id"].is_string());
    let recipients: Vec<(uuid::Uuid, String)> = sqlx::query_as("SELECT recipient_member_id,kind FROM porchlight.notification_events WHERE entity_id=$1 ORDER BY kind")
        .bind(comment["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap()).fetch_all(&pool).await.unwrap();
    assert_eq!(
        recipients,
        vec![
            (bob.member_id, "comment".into()),
            (bob.member_id, "mention".into())
        ]
    );
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn collaboration_stream_rejects_hidden_and_deleted_parent_tickles(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let alice = f.users[0].member_id;
    let bob = f.users[1].member_id;
    let h = f.server.init.household_id;
    let item: uuid::Uuid = sqlx::query_scalar("INSERT INTO porchlight.items(household_id,owner_id,title,kind,visibility) VALUES($1,$2,'Stream','todo','private') RETURNING id").bind(h).bind(alice).fetch_one(&pool).await.unwrap();
    let comment: uuid::Uuid = sqlx::query_scalar("INSERT INTO porchlight.comments(household_id,item_id,author_id,body) VALUES($1,$2,$3,'Stream') RETURNING id").bind(h).bind(item).bind(alice).fetch_one(&pool).await.unwrap();
    let hash = "b".repeat(64);
    sqlx::query("INSERT INTO porchlight.blobs(household_id,sha256,size,content_type) VALUES($1,$2,1,'text/plain')").bind(h).bind(&hash).execute(&pool).await.unwrap();
    let attachment: uuid::Uuid = sqlx::query_scalar("INSERT INTO porchlight.attachments(household_id,item_id,blob_sha256,original_name,uploader_id) VALUES($1,$2,$3,'Stream.txt',$4) RETURNING id").bind(h).bind(item).bind(&hash).bind(alice).fetch_one(&pool).await.unwrap();
    porchlight_db::outbox::assign_stream_seqs(&f.server.state.pool, 1000)
        .await
        .unwrap();
    let owner = porchlight_db::stream::read(&f.server.state.pool, h, alice, 0)
        .await
        .unwrap();
    for id in [comment, attachment] {
        assert!(owner.events.iter().any(|e| e.entity_id == id));
    }
    let hidden = porchlight_db::stream::read(&f.server.state.pool, h, bob, 0)
        .await
        .unwrap();
    assert!(
        !hidden
            .events
            .iter()
            .any(|e| [comment, attachment].contains(&e.entity_id))
    );
    assert!(hidden.scanned > 0);
    // Production flushing uses S6b's batched current-ACL statement, not queued decisions.
    let seqs: Vec<i64> = owner
        .events
        .iter()
        .filter(|e| [comment, attachment].contains(&e.entity_id))
        .map(|e| e.seq)
        .collect();
    assert_eq!(seqs.len(), 2);
    let current = porchlight_db::stream::deliveries(&f.server.state.pool, h, alice, &seqs)
        .await
        .unwrap();
    assert_eq!(current.len(), 2);
    assert!(
        porchlight_db::stream::deliveries(&f.server.state.pool, h, bob, &seqs)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("UPDATE porchlight.items SET deleted_at=now() WHERE id=$1")
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        porchlight_db::stream::deliveries(&f.server.state.pool, h, alice, &seqs)
            .await
            .unwrap()
            .is_empty()
    );
    porchlight_db::outbox::assign_stream_seqs(&f.server.state.pool, 1000)
        .await
        .unwrap();
    let eviction_payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM porchlight.outbox WHERE entity_id=$1 AND kind='item.access_changed' ORDER BY id DESC LIMIT 1",
    ).bind(item).fetch_one(&pool).await.unwrap();
    assert!(
        eviction_payload["recipients"]
            .as_array()
            .unwrap()
            .contains(&json!(alice))
    );
    let after_delete = porchlight_db::stream::read(&f.server.state.pool, h, alice, 0)
        .await
        .unwrap();
    assert!(
        after_delete
            .events
            .iter()
            .any(|e| e.entity_id == item && e.evicted)
    );
    let deleted = porchlight_db::stream::read(&f.server.state.pool, h, alice, 0)
        .await
        .unwrap();
    assert!(
        !deleted
            .events
            .iter()
            .any(|e| [comment, attachment].contains(&e.entity_id))
    );
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn merged_search_decoration_retains_the_source_query_mvcc_view(pool: PgPool) {
    use porchlight_api::{
        routes,
        server::{ConnInfo, Transport},
    };
    use tower::ServiceExt;
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let d = &f.admin.device;
    let client = TestClient::new(&f.server.ca_pem, Some(d));
    let item = client
        .request(
            f.server.lan,
            d.request("POST", "/v1/items", json!({"title":"Mvccword"}), None),
        )
        .await
        .unwrap()
        .json();
    let comment = client
        .request(
            f.server.lan,
            d.request(
                "POST",
                &format!("/v1/items/{}/comments", item["id"].as_str().unwrap()),
                json!({"body":"Mvccword before"}),
                None,
            ),
        )
        .await
        .unwrap()
        .json();
    let list = client
        .request(
            f.server.lan,
            d.request("POST", "/v1/lists", json!({"name":"Mvccword"}), None),
        )
        .await
        .unwrap()
        .json();
    let entry = client
        .request(
            f.server.lan,
            d.request(
                "POST",
                &format!("/v1/lists/{}/entries", list["id"].as_str().unwrap()),
                json!({"label":"Before"}),
                None,
            ),
        )
        .await
        .unwrap()
        .json();
    let mut writer = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE porchlight.list_entry_checks IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *writer)
        .await
        .unwrap();
    let router = routes::mtls_router(f.server.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(d.fingerprint),
    }));
    let request = d.request("GET", "/v1/search?q=Mvccword", json!({}), None);
    let task = tokio::spawn(async move { router.oneshot(request).await.unwrap() });
    tokio::time::timeout(std::time::Duration::from_secs(3), async { loop {
        let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation='porchlight.list_entry_checks'::regclass AND NOT granted AND mode='AccessShareLock')").fetch_one(&pool).await.unwrap();
        if waiting { break; }
        tokio::task::yield_now().await;
    }}).await.expect("search did not reach list decoration");
    sqlx::query("INSERT INTO porchlight.list_entry_checks(household_id,entry_id,checked_by) VALUES($1,$2,$3)")
        .bind(f.server.init.household_id).bind(entry["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap()).bind(f.admin.member_id).execute(&mut *writer).await.unwrap();
    sqlx::query("UPDATE porchlight.comments SET body='Mvccword after',edited_at=now() WHERE id=$1")
        .bind(
            comment["id"]
                .as_str()
                .unwrap()
                .parse::<uuid::Uuid>()
                .unwrap(),
        )
        .execute(&mut *writer)
        .await
        .unwrap();
    writer.commit().await.unwrap();
    let response = task.await.unwrap();
    assert_eq!(response.status(), 200);
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    let page: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let rows = page["items"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["body"], "Mvccword before");
    assert_eq!(rows[2]["entries"].as_array().unwrap().len(), 1);
    assert!(rows[2]["entries"][0]["checked_by"].is_null());
    assert!(
        rows[2]["got_it"].as_array().unwrap().is_empty(),
        "list decoration must not observe later checks"
    );
    f.server.stop().await;
}
