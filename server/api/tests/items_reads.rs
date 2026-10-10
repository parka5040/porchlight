use axum::{Router, body::to_bytes};
use porchlight_api::{
    routes,
    server::{ConnInfo, Transport},
    testing::{TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

async fn body(r: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(r.into_body(), 8 * 1024 * 1024).await.unwrap()).unwrap()
}
fn router(s: &TestServer, fp: [u8; 32]) -> Router {
    routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(fp),
    }))
}
#[sqlx::test(migrations = "../db/migrations")]
async fn reads_filter_private_data_and_search_pages(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let other:uuid::Uuid=sqlx::query_scalar("insert into porchlight.household_members(household_id,display_name,role,color,tzid) values($1,'Other','dependent','#ABCDEF','UTC') returning id").bind(s.init.household_id).fetch_one(&pool).await.unwrap();
    for (owner, vis, title) in [
        (other, "private", "SECRET-plumbing"),
        (s.init.admin_member_id, "household", "Visible-plumbing"),
        (s.init.admin_member_id, "private", "Another-plumbing"),
    ] {
        let item:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,visibility) values($1,$2,$3,'todo',$4) returning id").bind(s.init.household_id).bind(owner).bind(title).bind(vis).fetch_one(&pool).await.unwrap();
        sqlx::query(
            "insert into porchlight.item_occurrences(household_id,item_id,seq) values($1,$2,0)",
        )
        .bind(s.init.household_id)
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    }
    let r = router(&s, d.fingerprint);
    for path in [
        "/v1/board?scope=household",
        "/v1/snapshot",
        "/v1/agenda?from=2026-01-01T00:00:00Z&to=2027-01-01T00:00:00Z",
        "/v1/search?q=plumbing",
    ] {
        let response = r
            .clone()
            .oneshot(d.request("GET", path, json!({}), None))
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{path}");
        let v = body(response).await;
        assert!(!v.to_string().contains("SECRET"), "{path}: {v}");
    }
    let v = body(
        r.clone()
            .oneshot(d.request("GET", "/v1/search?q=plumbing&limit=1", json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let c = v["next_cursor"].as_str().unwrap();
    let next = body(
        r.clone()
            .oneshot(d.request(
                "GET",
                &format!("/v1/search?q=plumbing&limit=1&cursor={c}"),
                json!({}),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_ne!(v["items"][0]["id"], next["items"][0]["id"]);
    assert!(next["next_cursor"].is_null());
    for tz in ["CET", "UTC", "GMT"] {
        let mut req = d.request("GET", "/v1/members", json!({}), None);
        req.headers_mut()
            .insert("porchlight-tz", tz.parse().unwrap());
        assert_eq!(r.clone().oneshot(req).await.unwrap().status(), 200);
    }
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn settings_and_member_writes_use_claim_connection_and_match(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            db_pool_size: 1,
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let r = router(&s, d.fingerprint);
    let initial = body(
        r.clone()
            .oneshot(d.request("GET", "/v1/me/settings", json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    let value = json!({"today_sections":["overdue","today","this_week","needs_owner"],"scope":"mine","this_week_days":7,"pinned_list_id":null});
    let missing = r
        .clone()
        .oneshot(d.request(
            "PUT",
            "/v1/me/settings",
            value.clone(),
            Some("settings-missing"),
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), 428);
    let mut req = d.request(
        "PUT",
        "/v1/me/settings",
        value.clone(),
        Some("settings-write"),
    );
    req.headers_mut()
        .insert("if-match", initial["version"].to_string().parse().unwrap());
    let response = tokio::time::timeout(std::time::Duration::from_secs(2), r.clone().oneshot(req))
        .await
        .expect("callback escaped IdemTx")
        .unwrap();
    assert_eq!(response.status(), 200);
    let saved = body(response).await;
    assert_eq!(saved["settings"], value);
    let mut req = d.request("PUT", "/v1/me/settings", value, Some("settings-stale"));
    req.headers_mut().insert("if-match", "1".parse().unwrap());
    let stale = r.clone().oneshot(req).await.unwrap();
    assert_eq!(stale.status(), 409);
    assert_eq!(body(stale).await["current"], saved);
    let member = body(
        r.clone()
            .oneshot(d.request("GET", "/v1/members", json!({}), None))
            .await
            .unwrap(),
    )
    .await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == s.init.admin_member_id.to_string())
        .unwrap()
        .clone();
    let path = format!("/v1/members/{}", s.init.admin_member_id);
    let mut req = d.request(
        "PATCH",
        &path,
        json!({"color":"#123ABC","tz_source":"manual","tzid":"Etc/UTC"}),
        Some("member-zone"),
    );
    req.headers_mut()
        .insert("if-match", member["version"].to_string().parse().unwrap());
    let response = tokio::time::timeout(std::time::Duration::from_secs(2), r.clone().oneshot(req))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), 200);
    let member = body(response).await;
    assert_eq!(member["tzid"], "UTC");
    let mut req = d.request(
        "PATCH",
        &path,
        json!({"make_zone_authority":true}),
        Some("authority"),
    );
    req.headers_mut()
        .insert("if-match", member["version"].to_string().parse().unwrap());
    let response = r.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), 200);
    let current = body(response).await;
    assert_eq!(current["tz_source"], "device");
    assert_eq!(current["tz_authority_device_id"], d.device_id.to_string());
    let mut req = d.request(
        "PATCH",
        &path,
        json!({"color":"#ABCDEF"}),
        Some("member-stale"),
    );
    req.headers_mut()
        .insert("if-match", member["version"].to_string().parse().unwrap());
    let response = r.oneshot(req).await.unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(body(response).await["current"], current);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn relationship_visibility_matrix_all_read_models(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let user: uuid::Uuid = sqlx::query_scalar(
        "insert into porchlight.users(household_id,username) values($1,'reader') returning id",
    )
    .bind(s.init.household_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let member:uuid::Uuid=sqlx::query_scalar("insert into porchlight.household_members(household_id,user_id,role,display_name,color,tzid) values($1,$2,'user','Reader','#ABCDEF','UTC') returning id").bind(s.init.household_id).bind(user).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.user_credentials(household_id,user_id,password_hash) select household_id,$1,password_hash from porchlight.user_credentials where user_id=(select user_id from porchlight.household_members where id=$2)").bind(user).bind(s.init.admin_member_id).execute(&pool).await.unwrap();
    let owner = s.init.admin_member_id;
    let mut item_ids = Vec::new();
    for role in ["super_admin", "user", "agent"] {
        let reader = if role == "user" { member } else { owner };
        let other = if reader == owner { member } else { owner };
        let d = s.session_device(reader, Platform::Desktop).await;
        let r = if role == "agent" {
            routes::agent_router(s.state.clone()).layer(axum::Extension(ConnInfo {
                transport: Transport::Agent,
                client_addr: None,
                client_cert_fingerprint: None,
            }))
        } else {
            router(&s, d.fingerprint)
        };
        for vis in ["household", "private", "selected"] {
            for relationship in ["owner", "assignee", "viewer", "subject", "other"] {
                let title = format!("matrix-{role}-{vis}-{relationship}");
                let id:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,subject_member_id,title,kind,visibility) values($1,$2,$3,$4,'todo',$5) returning id").bind(s.init.household_id).bind(if relationship=="owner"{reader}else{other}).bind(if relationship=="subject"{Some(reader)}else{None}).bind(&title).bind(vis).fetch_one(&pool).await.unwrap();
                item_ids.push(id);
                if relationship == "assignee" {
                    sqlx::query("insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)").bind(s.init.household_id).bind(id).bind(reader).execute(&pool).await.unwrap();
                }
                if relationship == "viewer" {
                    sqlx::query("insert into porchlight.item_viewers(household_id,item_id,member_id) values($1,$2,$3)").bind(s.init.household_id).bind(id).bind(reader).execute(&pool).await.unwrap();
                }
                let occ:uuid::Uuid=sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq,due_date) values($1,$2,0,'2026-10-05') returning id").bind(s.init.household_id).bind(id).fetch_one(&pool).await.unwrap();
                sqlx::query("insert into porchlight.member_agenda(household_id,member_id,item_id,occurrence_id,due_date) values($1,$2,$3,$4,'2026-10-05')").bind(s.init.household_id).bind(reader).bind(id).bind(occ).execute(&pool).await.unwrap();
                let allowed = porchlight_api::permissions::read_allowed(role, vis, relationship);
                for path in [
                    "/v1/board?scope=household&limit=200".to_owned(),
                    "/v1/snapshot".into(),
                    format!(
                        "/v1/agenda?from=2026-10-01T00:00:00Z&to=2026-10-10T00:00:00Z&member_id={reader}&limit=200"
                    ),
                    format!("/v1/search?q={title}&limit=200"),
                ] {
                    let mut request = d.request("GET", &path, json!({}), None);
                    if role == "agent" {
                        request.headers_mut().insert(
                            "authorization",
                            format!("Bearer {}", s.bootstrap_token()).parse().unwrap(),
                        );
                    }
                    let response = r.clone().oneshot(request).await.unwrap();
                    assert_eq!(response.status(), 200, "{path}");
                    let v = body(response).await;
                    assert_eq!(
                        v.to_string().contains(&title),
                        allowed,
                        "{role}/{vis}/{relationship}/{path}"
                    );
                }
            }
        }
    }
    // Pending derivation cannot keep a formerly public item visible.
    sqlx::query("update porchlight.items set visibility='private',owner_id=$2,subject_member_id=null where id=$1").bind(item_ids[0]).bind(member).execute(&pool).await.unwrap();
    let d = s.session_device(owner, Platform::Desktop).await;
    let snapshot = body(
        router(&s, d.fingerprint)
            .oneshot(d.request("GET", "/v1/snapshot", json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    assert!(
        !snapshot
            .to_string()
            .contains("matrix-super_admin-household-owner")
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn board_projection_has_injected_seven_day_cutoff_and_byte_order(pool: PgPool) {
    use porchlight_api::{extract::Caller, routes::reads::board_rows};
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let now: chrono::DateTime<chrono::Utc> = "2026-10-05T12:00:00Z".parse().unwrap();
    let id:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind) values($1,$2,'series','todo') returning id").bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    for (seq, status, done, pos) in [
        (0, "todo", None, "80"),
        (1, "doing", None, "81"),
        (2, "done", Some(now - chrono::Duration::days(7)), "80"),
        (
            3,
            "done",
            Some(now - chrono::Duration::days(7) - chrono::Duration::seconds(1)),
            "81",
        ),
    ] {
        sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,status,completed_at,board_position) values($1,$2,$3,$4,$5,$6)").bind(s.init.household_id).bind(id).bind(seq).bind(status).bind(done).bind(pos).execute(&pool).await.unwrap();
    }
    sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,status,completed_at,disposition) values($1,$2,4,'done',$3,'expired')").bind(s.init.household_id).bind(id).bind(now).execute(&pool).await.unwrap();
    let caller = Caller::Agent {
        token_id: uuid::Uuid::nil(),
        member_id: s.init.admin_member_id,
        label: "test".into(),
        scopes: vec![],
        household_id: s.init.household_id,
        request_id: uuid::Uuid::nil(),
    };
    let mut c = s.state.pool.acquire().await.unwrap();
    let cards = board_rows(&mut c, &caller, "household", now, None, 200)
        .await
        .unwrap();
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0]["occurrence"]["seq"], 2);
    assert_eq!(cards[1]["occurrence"]["seq"], 0);
    let cards = board_rows(
        &mut c,
        &caller,
        "household",
        now + chrono::Duration::seconds(1),
        None,
        200,
    )
    .await
    .unwrap();
    assert_eq!(cards.len(), 1);
    drop(c);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn floating_agenda_in_two_zones_and_conflict_masking(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let reader = s.init.admin_member_id;
    let h = s.init.household_id;
    sqlx::query("update porchlight.household_members set tzid='Europe/London',tz_source='manual' where id=$1").bind(reader).execute(&pool).await.unwrap();
    let other:uuid::Uuid=sqlx::query_scalar("insert into porchlight.household_members(household_id,display_name,role,color,tzid) values($1,'Dependent','dependent','#ABCDEF','Asia/Tokyo') returning id").bind(h).fetch_one(&pool).await.unwrap();
    sqlx::query("update porchlight.households set tzid='Asia/Tokyo' where id=$1")
        .bind(h)
        .execute(&pool)
        .await
        .unwrap();
    let id:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,time_mode,local_start,local_end) values($1,$2,'Floating','appointment','floating','2026-10-05 09:00','2026-10-05 10:00') returning id").bind(h).bind(reader).fetch_one(&pool).await.unwrap();
    for member in [reader, other] {
        sqlx::query("insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)").bind(h).bind(id).bind(member).execute(&pool).await.unwrap();
    }
    sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,15)").bind(h).bind(id).execute(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    porchlight_db::derive::derive_item(&mut tx, id, "2026-10-05T00:00:00Z".parse().unwrap())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let hidden:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,visibility) values($1,$2,'HIDDEN-TITLE','todo','private') returning id").bind(h).bind(other).fetch_one(&pool).await.unwrap();
    let occ:uuid::Uuid=sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq) values($1,$2,0) returning id").bind(h).bind(hidden).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.member_agenda(household_id,member_id,item_id,occurrence_id,starts_at,ends_at,busy) values($1,$2,$3,$4,'2026-10-05 08:15Z','2026-10-05 09:15Z',tstzrange('2026-10-05 08:15Z','2026-10-05 09:15Z','[)'))").bind(h).bind(reader).bind(hidden).bind(occ).execute(&pool).await.unwrap();
    let d = s.session_device(reader, Platform::Desktop).await;
    let r = router(&s, d.fingerprint);
    let v = body(
        r.clone()
            .oneshot(d.request(
                "GET",
                "/v1/agenda?from=2026-10-05T00:00:00Z&to=2026-10-06T00:00:00Z",
                json!({}),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let row = &v["items"][0];
    assert!(
        row["agenda"]["starts_at"]
            .as_str()
            .unwrap()
            .starts_with("2026-10-05T08:00:00")
    );
    assert_eq!(row["conflicts"][0]["other_title"], "Busy");
    assert!(row["conflicts"][0]["other_item_id"].is_null());
    assert!(row["conflicts"][0]["other_occurrence_id"].is_null());
    assert!(!v.to_string().contains("HIDDEN-TITLE"));
    let snapshot = body(
        r.clone()
            .oneshot(d.request("GET", "/v1/snapshot", json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(snapshot["scheduled_reminders"].as_array().unwrap().len(), 1);
    let v = body(
        r.oneshot(d.request(
            "GET",
            &format!(
                "/v1/agenda?from=2026-10-05T00:00:00Z&to=2026-10-06T00:00:00Z&member_id={other}"
            ),
            json!({}),
            None,
        ))
        .await
        .unwrap(),
    )
    .await;
    assert!(
        v["items"][0]["agenda"]["starts_at"]
            .as_str()
            .unwrap()
            .starts_with("2026-10-05T00:00:00")
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn snapshot_cursor_does_not_lose_concurrent_commit(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    porchlight_db::outbox::assign_stream_seqs(&s.state.pool, 1000)
        .await
        .unwrap();
    let r = router(&s, d.fingerprint);
    // Hold a relation lock so snapshot establishes its MVCC view at the cursor query,
    // then waits at the item SELECT while this transaction inserts and commits.
    let mut writer = pool.begin().await.unwrap();
    sqlx::query("lock table porchlight.items in access exclusive mode")
        .execute(&mut *writer)
        .await
        .unwrap();
    let req = d.request("GET", "/v1/snapshot", json!({}), None);
    let task = tokio::spawn(async move { r.oneshot(req).await.unwrap() });
    tokio::time::timeout(std::time::Duration::from_secs(3),async {loop{
  let waiting:bool=sqlx::query_scalar("select exists(select 1 from pg_locks where relation='porchlight.items'::regclass and not granted and mode='AccessShareLock')").fetch_one(&pool).await.unwrap();
  if waiting{break}tokio::task::yield_now().await;
 }}).await.expect("snapshot never reached item read");
    let item:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind) values($1,$2,'Concurrent commit','todo') returning id").bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&mut *writer).await.unwrap();
    writer.commit().await.unwrap();
    let response = task.await.unwrap();
    assert_eq!(response.status(), 200);
    let snapshot = body(response).await;
    let cursor = snapshot["cursor"].as_i64().unwrap();
    assert!(!snapshot["items"].to_string().contains("Concurrent commit"));
    porchlight_db::outbox::assign_stream_seqs(&s.state.pool, 1000)
        .await
        .unwrap();
    let events = porchlight_db::stream::read(
        &s.state.pool,
        s.init.household_id,
        s.init.admin_member_id,
        cursor,
    )
    .await
    .unwrap();
    assert!(
        events
            .events
            .iter()
            .any(|e| e.entity_id == item && e.seq > cursor)
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn settings_schema_replay_private_stream_and_member_forbidden(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
            db_pool_size: 1,
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let r = router(&s, d.fingerprint);
    let before = body(
        r.clone()
            .oneshot(d.request("GET", "/v1/me/settings", json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    let ver = before["version"].to_string();
    for (n,value) in [json!({"today_sections":["today","today"],"scope":"mine","this_week_days":7,"pinned_list_id":null}),json!({"today_sections":["today"],"scope":"mine","this_week_days":0,"pinned_list_id":null}),json!({"today_sections":["pinned_list"],"scope":"household","this_week_days":7,"pinned_list_id":null})].into_iter().enumerate(){
  let mut req=d.request("PUT","/v1/me/settings",value,Some(&format!("invalid-{n}")));req.headers_mut().insert("if-match",ver.parse().unwrap());assert_eq!(r.clone().oneshot(req).await.unwrap().status(),422);
 }
    let value = json!({"today_sections":["today"],"scope":"household","this_week_days":14,"pinned_list_id":null});
    let mut responses = Vec::new();
    for _ in 0..2 {
        let mut req = d.request(
            "PUT",
            "/v1/me/settings",
            value.clone(),
            Some("replay-settings"),
        );
        req.headers_mut().insert("if-match", ver.parse().unwrap());
        let response = r.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), 200);
        responses.push(body(response).await);
    }
    assert_eq!(responses[0], responses[1]);
    assert_eq!(
        responses[0]["version"].as_i64().unwrap(),
        before["version"].as_i64().unwrap() + 1
    );
    let other:uuid::Uuid=sqlx::query_scalar("insert into porchlight.household_members(household_id,display_name,role,color,tzid) values($1,'Other','dependent','#ABCDEF','UTC') returning id").bind(s.init.household_id).fetch_one(&pool).await.unwrap();
    let mut req = d.request(
        "PATCH",
        &format!("/v1/members/{other}"),
        json!({"color":"#123ABC"}),
        Some("not-self"),
    );
    req.headers_mut().insert("if-match", "1".parse().unwrap());
    assert_eq!(r.oneshot(req).await.unwrap().status(), 403);
    porchlight_db::outbox::assign_stream_seqs(&s.state.pool, 1000)
        .await
        .unwrap();
    let own = porchlight_db::stream::read(
        &s.state.pool,
        s.init.household_id,
        s.init.admin_member_id,
        0,
    )
    .await
    .unwrap();
    assert!(
        own.events
            .iter()
            .any(|e| e.kind == "settings.changed" && e.entity_id == s.init.admin_member_id)
    );
    assert!(
        !own.events
            .iter()
            .any(|e| e.kind == "settings.changed" && e.entity_id == other)
    );
    let other_events = porchlight_db::stream::read(&s.state.pool, s.init.household_id, other, 0)
        .await
        .unwrap();
    assert!(
        !other_events
            .events
            .iter()
            .any(|e| e.kind == "settings.changed" && e.entity_id == s.init.admin_member_id)
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn malformed_collection_queries_are_problem_json(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let r = router(&s, d.fingerprint);
    for path in [
        "/v1/agenda?from=not-a-date&to=2026-10-05T00:00:00Z",
        "/v1/members?limit=no",
        "/v1/search",
        "/v1/board?unexpected=1",
    ] {
        let response = r
            .clone()
            .oneshot(d.request("GET", path, json!({}), None))
            .await
            .unwrap();
        assert_eq!(response.status(), 400);
        assert_eq!(
            response.headers()["content-type"],
            "application/problem+json"
        );
        assert_eq!(
            body(response).await["type"],
            "urn:porchlight:problem:invalid-request"
        );
    }
    s.stop().await;
}

async fn self_member(s: &TestServer, d: &porchlight_api::testing::TestDevice) -> Value {
    body(
        router(s, d.fingerprint)
            .oneshot(d.request("GET", "/v1/members", json!({}), None))
            .await
            .unwrap(),
    )
    .await["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == s.init.admin_member_id.to_string())
        .unwrap()
        .clone()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn fix1_header_authority_and_invalid_zone_reports(pool: PgPool) {
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
    let other = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let r = router(&s, d.fingerprint);
    let current = self_member(&s, &d).await;
    let mut req = d.request(
        "PATCH",
        &format!("/v1/members/{}", s.init.admin_member_id),
        json!({"make_zone_authority":true}),
        Some("authority-fix1"),
    );
    req.headers_mut()
        .insert("if-match", current["version"].to_string().parse().unwrap());
    let response = r.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), 200);
    let authority = body(response).await;
    assert_eq!(authority["tz_source"], "device");
    assert_eq!(authority["tz_authority_device_id"], d.device_id.to_string());
    for (device, zone, expected) in [
        (&d, "Europe/Berlin", "Europe/Berlin"),
        (&other, "Asia/Tokyo", "Europe/Berlin"),
        (&d, "Mars/Olympus_Mons", "Europe/Berlin"),
        (&d, "CET", "Europe/Brussels"),
        (&d, "GMT", "UTC"),
    ] {
        let before = self_member(&s, &d).await;
        let mut req = device.request("GET", "/v1/members", json!({}), None);
        req.headers_mut()
            .insert("porchlight-tz", zone.parse().unwrap());
        assert_eq!(
            router(&s, device.fingerprint)
                .oneshot(req)
                .await
                .unwrap()
                .status(),
            200
        );
        let after = self_member(&s, &d).await;
        assert_eq!(after["tzid"], expected);
        if device.device_id != d.device_id || zone.starts_with("Mars") {
            assert_eq!(after, before);
        }
    }
    let current = self_member(&s, &d).await;
    let mut req = d.request(
        "PATCH",
        &format!("/v1/members/{}", s.init.admin_member_id),
        json!({"tz_source":"manual","tzid":"Mars/Olympus_Mons"}),
        Some("invalid-zone-patch"),
    );
    req.headers_mut()
        .insert("if-match", current["version"].to_string().parse().unwrap());
    let response = r.oneshot(req).await.unwrap();
    assert_eq!(response.status(), 400);
    assert!(
        body(response).await["type"]
            .as_str()
            .unwrap()
            .ends_with("invalid-time-zone")
    );
    assert_eq!(self_member(&s, &d).await, current);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn fix1_agenda_local_dates_chronological_pages(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let h = s.init.household_id;
    let member = s.init.admin_member_id;
    sqlx::query("update porchlight.household_members set tzid='America/New_York',tz_source='manual' where id=$1").bind(member).execute(&pool).await.unwrap();
    for date in ["2026-10-07", "2026-10-06"] {
        let item:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,due_date) values($1,$2,$3,'todo',$3::text::date) returning id").bind(h).bind(member).bind(date).fetch_one(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        porchlight_db::derive::derive_item(&mut tx, item, "2026-10-05T12:00:00Z".parse().unwrap())
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let d = s.session_device(member, Platform::Desktop).await;
    let r = router(&s, d.fingerprint);
    let path = "/v1/agenda?from=2026-10-06T04:00:00Z&to=2026-10-07T04:00:00Z";
    let rows = body(
        r.oneshot(d.request("GET", path, json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(rows["items"].as_array().unwrap().len(), 1);
    assert_eq!(rows["items"][0]["due_date"], "2026-10-06");
    let page = body(
        router(&s, d.fingerprint)
            .oneshot(d.request(
                "GET",
                "/v1/agenda?from=2026-10-06T04:00:00Z&to=2026-10-08T04:00:00Z&limit=1",
                json!({}),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(page["items"][0]["due_date"], "2026-10-06");
    let path = format!(
        "/v1/agenda?from=2026-10-06T04:00:00Z&to=2026-10-08T04:00:00Z&limit=1&cursor={}",
        page["next_cursor"].as_str().unwrap()
    );
    let page = body(
        router(&s, d.fingerprint)
            .oneshot(d.request("GET", &path, json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(page["items"][0]["due_date"], "2026-10-07");
    assert!(page["next_cursor"].is_null());
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn fix1_member_zone_postcommit_drain_and_transient_failure(pool: PgPool) {
    let now: chrono::DateTime<chrono::Utc> = "2026-10-05T12:00:00Z".parse().unwrap();
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            db_pool_size: 1,
            background_derivation: false,
            item_clock: std::sync::Arc::new(move || now),
            ..Default::default()
        },
    )
    .await;
    let member = s.init.admin_member_id;
    let item:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,time_mode,local_start,local_end) values($1,$2,'Drain','appointment','floating','2026-10-06 09:00','2026-10-06 10:00') returning id").bind(s.init.household_id).bind(member).fetch_one(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    porchlight_db::derive::derive_item(&mut tx, item, now)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let d = s.session_device(member, Platform::Desktop).await;
    let r = router(&s, d.fingerprint);
    let path = format!("/v1/members/{member}");
    for (index, zone, pending) in [(0, "Europe/Berlin", false), (1, "Asia/Tokyo", true)] {
        if pending {
            sqlx::raw_sql("create function public.fail_drain() returns trigger language plpgsql as $$ begin raise exception 'injected transient' using errcode='40001'; end $$; create trigger fail_drain before insert or update on porchlight.member_agenda for each row execute function public.fail_drain()").execute(&pool).await.unwrap();
        }
        let current = self_member(&s, &d).await;
        let mut req = d.request(
            "PATCH",
            &path,
            json!({"tz_source":"manual","tzid":zone}),
            Some(&format!("drain-{index}")),
        );
        req.headers_mut()
            .insert("if-match", current["version"].to_string().parse().unwrap());
        let replay = req.headers().clone();
        let response =
            tokio::time::timeout(std::time::Duration::from_secs(3), r.clone().oneshot(req))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(response.status(), 200);
        let result = body(response).await;
        assert_eq!(result["pending_derivation"], pending);
        let actual: bool = sqlx::query_scalar(
            "select applied_gen<requested_gen from porchlight.item_derivation where item_id=$1",
        )
        .bind(item)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(actual, pending);
        if !pending {
            let starts: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
                "select starts_at from porchlight.member_agenda where item_id=$1 and member_id=$2",
            )
            .bind(item)
            .bind(member)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                starts,
                "2026-10-06T07:00:00Z"
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .unwrap()
            );
        }
        if pending {
            sqlx::query("drop trigger fail_drain on porchlight.member_agenda")
                .execute(&pool)
                .await
                .unwrap();
            // Replays must not drain, even now that the injected failure has gone.
            let mut req = d.request(
                "PATCH",
                &path,
                json!({"tz_source":"manual","tzid":zone}),
                Some(&format!("drain-{index}")),
            );
            *req.headers_mut() = replay;
            assert_eq!(body(r.clone().oneshot(req).await.unwrap()).await, result);
            assert!(sqlx::query_scalar::<_,bool>("select applied_gen<requested_gen from porchlight.item_derivation where item_id=$1").bind(item).fetch_one(&pool).await.unwrap());
            porchlight_api::derive_worker::run_batch(
                &s.state.pool,
                now + chrono::Duration::minutes(1),
            )
            .await
            .unwrap();
            assert!(!sqlx::query_scalar::<_,bool>("select applied_gen<requested_gen from porchlight.item_derivation where item_id=$1").bind(item).fetch_one(&pool).await.unwrap());
        }
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn fix1_settings_normalized_authority_private_and_range_error(pool: PgPool) {
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
    let r = router(&s, d.fingerprint);
    let other:uuid::Uuid=sqlx::query_scalar("insert into porchlight.household_members(household_id,display_name,role,color,tzid) values($1,'Other','dependent','#ABCDEF','UTC') returning id").bind(s.init.household_id).fetch_one(&pool).await.unwrap();
    let user: uuid::Uuid = sqlx::query_scalar(
        "insert into porchlight.users(household_id,username) values($1,'other-fix1') returning id",
    )
    .bind(s.init.household_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("update porchlight.household_members set user_id=$2,role='user' where id=$1")
        .bind(other)
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into porchlight.user_credentials(household_id,user_id,password_hash) select household_id,$1,password_hash from porchlight.user_credentials where user_id=(select user_id from porchlight.household_members where id=$2)").bind(user).bind(s.init.admin_member_id).execute(&pool).await.unwrap();
    let od = s.session_device(other, Platform::Desktop).await;
    sqlx::query("update porchlight.household_members set tz_authority_device_id=$2 where id=$1")
        .bind(other)
        .bind(od.device_id)
        .execute(&pool)
        .await
        .unwrap();
    for stored in [json!({}), json!({"this_week_days":0})] {
        sqlx::query("update porchlight.member_settings set settings=$2 where member_id=$1")
            .bind(s.init.admin_member_id)
            .bind(sqlx::types::Json(stored))
            .execute(&pool)
            .await
            .unwrap();
        let response = body(
            r.clone()
                .oneshot(d.request("GET", "/v1/me/settings", json!({}), None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            response["settings"],
            serde_json::to_value(porchlight_api::routes::members::TodaySettings::default())
                .unwrap()
        );
    }
    let snapshot = body(
        r.clone()
            .oneshot(d.request("GET", "/v1/snapshot", json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    let profile = snapshot["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == other.to_string())
        .unwrap();
    assert!(profile["tz_authority_device_id"].is_null());
    for days in [json!(300), json!(-1), json!(18446744073709551615u64)] {
        let mut req = d.request(
            "PUT",
            "/v1/me/settings",
            json!({"today_sections":[],"scope":"mine","this_week_days":days,"pinned_list_id":null}),
            Some(&format!("range-{days}")),
        );
        req.headers_mut().insert("if-match", "1".parse().unwrap());
        let response = r.clone().oneshot(req).await.unwrap();
        assert_eq!(response.status(), 422);
        assert!(
            body(response).await["type"]
                .as_str()
                .unwrap()
                .ends_with("validation-failed")
        );
    }
    s.stop().await;
}

// Flatten only the outer object schema, retaining its nested field constraints.
fn flat_schema(doc: &Value, schema: &Value) -> Value {
    if let Some(reference) = schema["$ref"].as_str() {
        return flat_schema(
            doc,
            doc.pointer(reference.strip_prefix('#').unwrap()).unwrap(),
        );
    }
    if let Some(parts) = schema["allOf"].as_array() {
        let mut merged = json!({"type":"object","properties":{},"required":[]});
        for part in parts {
            let part = flat_schema(doc, part);
            merged["properties"]
                .as_object_mut()
                .unwrap()
                .extend(part["properties"].as_object().unwrap().clone());
            if let Some(required) = part["required"].as_array() {
                merged["required"]
                    .as_array_mut()
                    .unwrap()
                    .extend(required.iter().cloned());
            }
        }
        return merged;
    }
    schema.clone()
}
// Strict schema traversal: unknown runtime columns fail as well as missing/wrong fields.
fn conforms(doc: &Value, schema: &Value, value: &Value) {
    if let Some(reference) = schema["$ref"].as_str() {
        return conforms(
            doc,
            doc.pointer(reference.strip_prefix('#').unwrap()).unwrap(),
            value,
        );
    }
    if schema.get("allOf").is_some() {
        return conforms(doc, &flat_schema(doc, schema), value);
    }
    if let Some(variants) = schema
        .get("anyOf")
        .or_else(|| schema.get("oneOf"))
        .and_then(Value::as_array)
    {
        let chosen = variants
            .iter()
            .find(|s| s["type"] == "null" && value.is_null())
            .or_else(|| {
                variants.iter().find(|s| {
                    flat_schema(doc, s)["properties"]["entity_type"]["enum"]
                        .as_array()
                        .is_some_and(|tags| tags.contains(&value["entity_type"]))
                })
            })
            // S7a's untagged union: choose by complete object shape, not source order.
            .or_else(|| {
                variants.iter().find(|s| {
                    let shape = flat_schema(doc, s);
                    shape["type"] == "object"
                        && value.as_object().is_some_and(|object| {
                            object
                                .keys()
                                .all(|key| shape["properties"].get(key).is_some())
                                && shape["required"].as_array().is_none_or(|required| {
                                    required
                                        .iter()
                                        .all(|key| object.contains_key(key.as_str().unwrap()))
                                })
                        })
                })
            })
            .or_else(|| variants.iter().find(|s| s["type"] != "null"))
            .unwrap();
        return conforms(doc, chosen, value);
    }
    let types = schema["type"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_else(|| schema["type"].as_str().into_iter().collect());
    if value.is_null() {
        assert!(types.contains(&"null"), "not nullable: {schema}");
        return;
    }
    if let Some(values) = schema["enum"].as_array() {
        assert!(values.contains(value), "enum {value}");
    }
    if types.contains(&"object") {
        let object = value.as_object().expect("object");
        if let Some(required) = schema["required"].as_array() {
            for key in required {
                assert!(object.contains_key(key.as_str().unwrap()), "missing {key}");
            }
        }
        for (key, val) in object {
            let property = schema["properties"]
                .get(key)
                .unwrap_or_else(|| panic!("undocumented field {key}"));
            conforms(doc, property, val);
        }
    } else if types.contains(&"array") {
        for val in value.as_array().expect("array") {
            conforms(doc, &schema["items"], val);
        }
    } else if types.contains(&"integer") {
        assert!(value.is_i64() || value.is_u64());
    } else if types.contains(&"boolean") {
        assert!(value.is_boolean());
    } else if types.contains(&"string") {
        let text = value.as_str().expect("string");
        match schema["format"].as_str() {
            Some("uuid") => {
                text.parse::<uuid::Uuid>().unwrap();
            }
            Some("date-time") => {
                chrono::DateTime::parse_from_rfc3339(text)
                    .unwrap_or_else(|e| panic!("invalid RFC3339 {text}: {e}"));
            }
            Some("local-date-time") => {
                text.parse::<chrono::NaiveDateTime>().unwrap();
            }
            Some("date") => {
                text.parse::<chrono::NaiveDate>().unwrap();
            }
            _ => {}
        }
    } else {
        panic!("unhandled schema {schema}");
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn fix1_board_domain_snapshot_spans_and_schema_conformance(pool: PgPool) {
    let now: chrono::DateTime<chrono::Utc> = "2026-10-05T12:00:00Z".parse().unwrap();
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(move || now),
            ..Default::default()
        },
    )
    .await;
    let h = s.init.household_id;
    let member = s.init.admin_member_id;
    let mut ids = Vec::new();
    for (title, pos) in [("First", "B"), ("Second", "a"), ("Mover", "z")] {
        let item:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,missed_policy) values($1,$2,$3,'todo','skip') returning id").bind(h).bind(member).bind(title).fetch_one(&pool).await.unwrap();
        if title == "First" {
            sqlx::query("INSERT INTO porchlight.comments(household_id,item_id,author_id,body) VALUES($1,$2,$3,'First comment')")
                .bind(h).bind(item).bind(member).execute(&pool).await.unwrap();
            sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,disposition,board_position) values($1,$2,0,'expired','A')").bind(h).bind(item).execute(&pool).await.unwrap();
        }
        let occurrence:uuid::Uuid=sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq,board_position,due_date) values($1,$2,1,$3,'2026-10-06') returning id").bind(h).bind(item).bind(pos).fetch_one(&pool).await.unwrap();
        sqlx::query("insert into porchlight.member_agenda(household_id,member_id,item_id,occurrence_id,due_date) values($1,$2,$3,$4,'2026-10-06')").bind(h).bind(member).bind(item).bind(occurrence).execute(&pool).await.unwrap();
        ids.push(occurrence);
    }
    let d = s.session_device(member, Platform::Desktop).await;
    let r = router(&s, d.fingerprint);
    let get = |path: &str| d.request("GET", path, json!({}), None);
    let board = body(
        r.clone()
            .oneshot(get("/v1/board?scope=household"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        board["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["occurrence"]["id"]
                .as_str()
                .unwrap()
                .parse::<uuid::Uuid>()
                .unwrap())
            .collect::<Vec<_>>(),
        ids
    );
    let mut tx = pool.begin().await.unwrap();
    porchlight_db::items::move_on_board(&mut tx, h, ids[2], 1, Some(ids[0]), Some(ids[1]), now)
        .await
        .expect("visible next non-expired card must be in ordering domain");
    tx.commit().await.unwrap();
    let board = body(
        r.clone()
            .oneshot(get("/v1/board?scope=household"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(board["items"][1]["occurrence"]["id"], ids[2].to_string());
    let item:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind) values($1,$2,'Long span','todo') returning id").bind(h).bind(member).fetch_one(&pool).await.unwrap();
    let span:uuid::Uuid=sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq,starts_at,ends_at) values($1,$2,0,$3,$4) returning id").bind(h).bind(item).bind(now-chrono::Duration::days(181)).bind(now-chrono::Duration::days(179)).fetch_one(&pool).await.unwrap();
    let ends_at_edge:uuid::Uuid=sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq,starts_at,ends_at) values($1,$2,2,$3,$4) returning id").bind(h).bind(item).bind(now-chrono::Duration::days(181)).bind(now-chrono::Duration::days(180)).fetch_one(&pool).await.unwrap();
    let floating:uuid::Uuid=sqlx::query_scalar("insert into porchlight.item_occurrences(household_id,item_id,seq,local_due) values($1,$2,1,$3) returning id").bind(h).bind(item).bind((now-chrono::Duration::days(180)-chrono::Duration::hours(1)).naive_utc()).fetch_one(&pool).await.unwrap();
    sqlx::query("update porchlight.items set recurrence_mode='completion',completion_interval=interval '1 month 2 days 3 hours' where id=$1").bind(item).execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,15)").bind(h).bind(item).execute(&pool).await.unwrap();
    // Nonempty planning fixtures must also conform: no internal suggestion columns.
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,0,'09:00','17:00')").bind(h).bind(member).execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.schedule_suggestions(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id) values($1,$2,$3,$4,'2026-10-06T13:00Z','2026-10-06T13:30Z',$5)").bind(h).bind(member).bind(item).bind(floating).bind(uuid::Uuid::now_v7()).execute(&pool).await.unwrap();
    let snapshot = body(r.clone().oneshot(get("/v1/snapshot")).await.unwrap()).await;
    assert_eq!(snapshot["suggestions"].as_array().unwrap().len(), 1);
    assert!(!snapshot["availability"].as_array().unwrap().is_empty());
    assert!(
        snapshot["occurrences"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == span.to_string()),
        "overlapping span must be included"
    );
    assert!(
        snapshot["occurrences"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == floating.to_string()),
        "floating anchor is in household zone, not UTC"
    );
    assert!(
        !snapshot["occurrences"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["id"] == ends_at_edge.to_string()),
        "a span ending at the half-open window edge does not overlap"
    );
    let doc = serde_json::to_value(porchlight_api::openapi::document()).unwrap();
    for (path, value) in [
        ("/v1/board", board),
        ("/v1/snapshot", snapshot),
        (
            "/v1/agenda",
            body(
                r.clone()
                    .oneshot(get(
                        "/v1/agenda?from=2026-10-06T00:00:00Z&to=2026-10-08T00:00:00Z",
                    ))
                    .await
                    .unwrap(),
            )
            .await,
        ),
        (
            "/v1/search",
            body(r.clone().oneshot(get("/v1/search?q=First")).await.unwrap()).await,
        ),
        (
            "/v1/members",
            body(r.clone().oneshot(get("/v1/members")).await.unwrap()).await,
        ),
        (
            "/v1/me/settings",
            body(r.oneshot(get("/v1/me/settings")).await.unwrap()).await,
        ),
    ] {
        conforms(
            &doc,
            &doc["paths"][path]["get"]["responses"]["200"]["content"]["application/json"]["schema"],
            &value,
        );
    }
    s.stop().await;
}
