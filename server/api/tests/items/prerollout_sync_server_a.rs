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

fn router(s: &TestServer, fp: [u8; 32]) -> Router {
    routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(fp),
    }))
}
async fn body(r: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(r.into_body(), 32 * 1024 * 1024).await.unwrap()).unwrap()
}
#[sqlx::test(migrations = "../db/migrations")]
async fn bundle_replaces_snapshot_rows_and_advertises_capability(pool: PgPool) {
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
    let r = router(&s, d.fingerprint);
    let id: uuid::Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind) values($1,$2,'Sync fixture','todo') returning id")
        .bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    let response = r
        .clone()
        .oneshot(d.request("GET", &format!("/v1/items/{id}/sync"), json!({}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "item sync route must exist");
    let bundle = body(response).await;
    let snapshot = body(
        r.clone()
            .oneshot(d.request("GET", "/v1/snapshot", json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(bundle["items"], snapshot["items"]);
    assert_eq!(bundle["occurrences"], snapshot["occurrences"]);
    let me = body(
        r.oneshot(d.request("GET", "/v1/me", json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(me["capabilities"], json!(["item-sync-v1"]));
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn notification_ids_are_validated(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let response = router(&s, d.fingerprint)
        .oneshot(d.request("GET", "/v1/notifications?ids=bad", json!({}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    s.stop().await;
}

fn caller(
    h: porchlight_db::testing::Household,
    member: uuid::Uuid,
) -> porchlight_api::extract::Caller {
    porchlight_api::extract::Caller::Member {
        household_id: h.id,
        member_id: member,
        user_id: uuid::Uuid::nil(),
        role: if member == h.super_admin {
            "super_admin"
        } else {
            "user"
        }
        .into(),
        device_id: uuid::Uuid::now_v7(),
        session_id: uuid::Uuid::nil(),
        request_id: uuid::Uuid::nil(),
        client_addr: None,
    }
}
async fn direct_bundle(
    s: &TestServer,
    c: porchlight_api::extract::Caller,
    id: uuid::Uuid,
    query: Option<String>,
) -> Value {
    body(
        routes::item_sync::get(
            axum::extract::State(s.state.clone()),
            c,
            axum::extract::Path(id),
            axum::extract::RawQuery(query),
        )
        .await
        .unwrap(),
    )
    .await
}
#[sqlx::test(migrations = "../db/migrations")]
async fn every_item_matches_all_projections_for_two_readers(pool: PgPool) {
    use axum::extract::State;
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let h = porchlight_db::testing::seed_household(&pool).await.unwrap();
    // Cross visibility and relationships, with real DST/floating/all-day/detached row shapes.
    for n in 0..12 {
        let id: uuid::Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,visibility,time_mode,tzid,starts_at,ends_at,local_start,local_end,rrule,recurrence_mode) values($1,$2,'Parity appointment','appointment',$3,$4,$5,$6,$7,$8,$9,'FREQ=DAILY;COUNT=4','calendar') returning id")
            .bind(h.id).bind(if n<6 { h.super_admin } else { h.user }).bind(["private","selected","household"][n%3])
            .bind(if n%2==0 { "fixed" } else { "floating" }).bind(if n%2==0 { Some("America/New_York") } else { None })
            .bind(if n%2==0 { Some("2026-11-01T05:30:00Z".parse::<chrono::DateTime<chrono::Utc>>().unwrap()) } else { None })
            .bind(if n%2==0 { Some("2026-11-01T07:30:00Z".parse::<chrono::DateTime<chrono::Utc>>().unwrap()) } else { None })
            .bind(if n%2!=0 { Some("2026-11-01T01:30:00".parse::<chrono::NaiveDateTime>().unwrap()) } else { None })
            .bind(if n%2!=0 { Some("2026-11-01T02:30:00".parse::<chrono::NaiveDateTime>().unwrap()) } else { None })
            .fetch_one(&pool).await.unwrap();
        if n % 3 == 1 {
            sqlx::query("insert into porchlight.item_viewers(household_id,item_id,member_id) values($1,$2,$3)").bind(h.id).bind(id).bind(h.user).execute(&pool).await.unwrap();
        }
        if n % 4 == 0 {
            sqlx::query("insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)").bind(h.id).bind(id).bind(h.user).execute(&pool).await.unwrap();
        }
        sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,starts_at,ends_at,local_start,local_end,override_local_start,override_local_end) select $1,$2,n,case when $3 then '2026-11-01T05:30Z'::timestamptz+n*interval '1 day' end,case when $3 then '2026-11-01T07:30Z'::timestamptz+n*interval '1 day' end,case when not $3 then '2026-11-01T01:30'::timestamp+n*interval '1 day' end,case when not $3 then '2026-11-01T02:30'::timestamp+n*interval '1 day' end,case when n=1 then '2027-03-14T02:30'::timestamp end,case when n=1 then '2027-03-14T03:30'::timestamp end from generate_series(0,3) n")
            .bind(h.id).bind(id).bind(n%2==0).execute(&pool).await.unwrap();
    }
    for sql in [
        "insert into porchlight.items(household_id,owner_id,title,kind,due_date,all_day) values($1,$2,'All day','todo','2026-11-01',true),($1,$2,'Undated','todo',null,false),($1,$2,'Distant board card','todo','2035-01-01',false)",
        "insert into porchlight.item_occurrences(household_id,item_id,seq,due_date) select $1,id,0,due_date from porchlight.items where household_id=$1 and kind='todo'",
        "insert into porchlight.member_agenda(household_id,member_id,item_id,occurrence_id,starts_at,ends_at,due_date,busy) select $1,m.id,i.id,o.id,coalesce(o.starts_at,o.local_start at time zone 'America/New_York'),coalesce(o.ends_at,o.local_end at time zone 'America/New_York'),o.due_date,case when o.starts_at is not null then tstzrange(o.starts_at,o.ends_at,'[)') end from porchlight.items i join porchlight.item_occurrences o on o.item_id=i.id cross join porchlight.household_members m where i.household_id=$1 and m.household_id=$1 and m.role<>'dependent'",
        "update porchlight.items set est_minutes=60 where household_id=$1 and title='All day'",
        "insert into porchlight.member_daily_load(member_id,date,planned_minutes,available_minutes,starts_at,ends_at,derived_at) select id,'2026-11-01',120,30,'2026-11-01T04:00Z','2026-11-02T05:00Z',now() from porchlight.household_members where household_id=$1 and role<>'dependent'",
        "insert into porchlight.comments(household_id,item_id,author_id,body) select $1,id,$2,'Parity comment' from porchlight.items where household_id=$1",
        "insert into porchlight.blobs(household_id,sha256,size,content_type) values($1,repeat('b',64),8,'text/plain')",
        "insert into porchlight.attachments(household_id,item_id,blob_sha256,original_name,uploader_id) select $1,id,repeat('b',64),'fixture.txt',$2 from porchlight.items where household_id=$1",
        "insert into porchlight.item_reminders(household_id,item_id,offset_minutes) select $1,id,10 from porchlight.items where household_id=$1",
        "insert into porchlight.scheduled_reminders(household_id,item_id,member_id,occurrence_id,reminder_id,fire_at) select $1,r.item_id,$2,o.id,r.id,'2026-11-01T05:20Z' from porchlight.item_reminders r join porchlight.item_occurrences o on o.item_id=r.item_id and o.seq=0 where r.household_id=$1",
        "insert into porchlight.schedule_suggestions(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id) select $1,$2,i.id,o.id,'2026-11-02T12:00Z','2026-11-02T13:00Z',uuidv7() from porchlight.items i join porchlight.item_occurrences o on o.item_id=i.id and o.seq=0 where i.household_id=$1",
        "insert into porchlight.lists(household_id,item_id,owner_id,name) select $1,id,$2,'Attached list' from porchlight.items where household_id=$1",
        "insert into porchlight.list_entries(household_id,list_id,label) select $1,id,'List entry' from porchlight.lists where household_id=$1",
    ] {
        // Keep positional fixture binds present even in statements not needing the owner.
        let uses_owner = sql.contains("$2");
        let q = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(h.id);
        let q = if uses_owner { q.bind(h.super_admin) } else { q };
        q.execute(&pool).await.unwrap();
    }
    let all_ids: Vec<uuid::Uuid> =
        sqlx::query_scalar("select id from porchlight.items where household_id=$1 order by id")
            .bind(h.id)
            .fetch_all(&pool)
            .await
            .unwrap();
    for member in [h.super_admin, h.user] {
        let c = caller(h, member);
        let snapshot = body(
            routes::reads::snapshot(State(s.state.clone()), c.clone())
                .await
                .unwrap(),
        )
        .await;
        let board = routes::reads::board(
            State(s.state.clone()),
            c.clone(),
            porchlight_api::extract::ApiQuery(routes::reads::BoardParams {
                scope: Some("household".into()),
                cursor: None,
                limit: Some(200),
            }),
        )
        .await
        .unwrap()
        .0
        .items;
        let from = "2026-04-08T12:00:00Z".parse().unwrap();
        let to = "2028-04-05T12:00:00Z".parse().unwrap();
        let agenda = routes::reads::agenda(
            State(s.state.clone()),
            c.clone(),
            porchlight_api::extract::ApiQuery(routes::reads::AgendaParams {
                from,
                to,
                member_id: Some(member),
                cursor: None,
                limit: Some(200),
            }),
        )
        .await
        .unwrap()
        .0
        .items;
        assert!(
            agenda
                .iter()
                .flat_map(|r| r["conflicts"].as_array().unwrap())
                .any(|c| c["kind"] == "hard")
        );
        assert!(
            agenda
                .iter()
                .flat_map(|r| r["conflicts"].as_array().unwrap())
                .any(|c| c["kind"] == "load")
        );
        for id in &all_ids {
            let visible = snapshot["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["id"] == id.to_string());
            if !visible {
                let response = routes::item_sync::get(
                    State(s.state.clone()),
                    c.clone(),
                    axum::extract::Path(*id),
                    axum::extract::RawQuery(None),
                )
                .await
                .unwrap_err();
                assert_eq!(response.problem(), porchlight_api::error::Problem::NotFound);
                continue;
            }
            let bundle = direct_bundle(
                &s,
                c.clone(),
                *id,
                Some(format!("include=collaboration&agenda={member}:{from}:{to}")),
            )
            .await;
            let doc: Value = serde_json::from_str(&porchlight_api::openapi::json()).unwrap();
            crate::common::schema::conforms(
                &doc,
                &doc["components"]["schemas"]["ItemSyncRead"],
                &bundle,
            );
            for section in [
                "items",
                "occurrences",
                "scheduled_reminders",
                "suggestions",
                "comments",
                "attachments",
                "lists",
            ] {
                let expected: Vec<_> = snapshot[section]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|r| {
                        if section == "items" {
                            r["id"] == id.to_string()
                        } else {
                            r["item_id"] == id.to_string()
                        }
                    })
                    .cloned()
                    .collect();
                assert_eq!(bundle[section], json!(expected), "{member} {id} {section}");
            }
            assert_eq!(
                bundle["board"],
                json!(
                    board
                        .iter()
                        .filter(|r| r["id"] == id.to_string())
                        .cloned()
                        .collect::<Vec<_>>()
                )
            );
            assert_eq!(
                bundle["agenda"][0]["items"],
                json!(
                    agenda
                        .iter()
                        .filter(|r| r["id"] == id.to_string())
                        .cloned()
                        .collect::<Vec<_>>()
                )
            );
            assert_eq!(bundle["coverage"]["collaboration"], true);
        }
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn ids_exact_absence_acl_and_comment_continuation(pool: PgPool) {
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
    let h = s.init.household_id;
    let member = s.init.admin_member_id;
    let other:uuid::Uuid=sqlx::query_scalar("insert into porchlight.household_members(household_id,display_name,role,color,tzid) values($1,'Other reader','dependent','#ABCDEF','UTC') returning id").bind(h).fetch_one(&pool).await.unwrap();
    let id:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind) values($1,$2,'Thread','todo') returning id").bind(h).bind(member).fetch_one(&pool).await.unwrap();
    let comments:Vec<uuid::Uuid>=sqlx::query_scalar("insert into porchlight.comments(household_id,item_id,author_id,body) select $1,$2,$3,'Comment '||n from generate_series(1,1003) n returning id").bind(h).bind(id).bind(member).fetch_all(&pool).await.unwrap();
    let missing = uuid::Uuid::now_v7();
    let get = |path: String| {
        let r = r.clone();
        let req = d.request("GET", &path, json!({}), None);
        async move { r.oneshot(req).await.unwrap() }
    };
    let page = body(
        get(format!(
            "/v1/items/{id}/comments?ids={},{missing},{}",
            comments[0], comments[1002]
        ))
        .await,
    )
    .await;
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    assert_eq!(page["items"][0]["id"], comments[0].to_string());
    assert_eq!(page["items"][1]["id"], comments[1002].to_string());
    assert!(page["next_cursor"].is_null());
    let bundle = body(get(format!("/v1/items/{id}/sync?include=collaboration")).await).await;
    assert_eq!(bundle["comments"].as_array().unwrap().len(), 200);
    assert_eq!(bundle["coverage"]["comments_complete"], false);
    let cursor = bundle["comments_next_cursor"].as_str().unwrap();
    let next = body(get(format!("/v1/items/{id}/comments?cursor={cursor}&limit=200")).await).await;
    assert_eq!(next["items"].as_array().unwrap().len(), 200);
    assert!(next["next_cursor"].is_string());
    let mut notifications = Vec::new();
    for recipient in [member, member, other] {
        let nid:uuid::Uuid=sqlx::query_scalar("insert into porchlight.notifications(household_id,recipient_member_id,kind,category,item_id,entity_type,entity_id,window_start,release_at,released_at,read_at) values($1,$2,'assigned','assignments',$3,'item',$3,now(),now(),now(),now()) returning id").bind(h).bind(recipient).bind(id).fetch_one(&pool).await.unwrap();
        let event:uuid::Uuid=sqlx::query_scalar("insert into porchlight.notification_events(household_id,recipient_member_id,kind,item_id,entity_type,entity_id,processed_at) values($1,$2,'assigned',$3,'item',$3,now()) returning id").bind(h).bind(recipient).bind(id).fetch_one(&pool).await.unwrap();
        sqlx::query(
            "insert into porchlight.notification_sources(notification_id,event_id) values($1,$2)",
        )
        .bind(nid)
        .bind(event)
        .execute(&pool)
        .await
        .unwrap();
        notifications.push(nid);
    }
    let result = body(
        get(format!(
            "/v1/notifications?ids={},{},{missing}",
            notifications[0], notifications[2]
        ))
        .await,
    )
    .await;
    assert_eq!(result["items"].as_array().unwrap().len(), 1);
    assert_eq!(result["items"][0]["id"], notifications[0].to_string());
    assert!(!result["items"][0]["read_at"].is_null());
    for prefix in [
        format!("/v1/items/{id}/comments"),
        "/v1/notifications".into(),
    ] {
        for suffix in [
            "ids=".into(),
            "ids=invalid".into(),
            format!("ids={missing}&limit=1"),
            format!("ids={}", vec![missing.to_string(); 101].join(",")),
        ] {
            assert_eq!(get(format!("{prefix}?{suffix}")).await.status(), 400);
        }
    }
    sqlx::query("update porchlight.comments set deleted_at=now() where id=$1")
        .bind(comments[0])
        .execute(&pool)
        .await
        .unwrap();
    let page = body(get(format!("/v1/items/{id}/comments?ids={}", comments[0])).await).await;
    assert_eq!(page["items"], json!([]));
    // Access loss removes the whole item scope; not a successful empty partial bundle.
    sqlx::query("update porchlight.items set owner_id=$2,visibility='private' where id=$1")
        .bind(id)
        .bind(other)
        .execute(&pool)
        .await
        .unwrap();
    for tail in ["sync", "comments"] {
        assert_eq!(get(format!("/v1/items/{id}/{tail}")).await.status(), 404);
    }
    let result = body(get(format!("/v1/notifications?ids={}", notifications[0])).await).await;
    assert_eq!(result["items"], json!([]));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn cap_gzip_and_tombstones(pool: PgPool) {
    use std::io::Read;
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
    let id:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,notes,kind) values($1,$2,'Gzip',repeat('Authorized text ',1000),'todo') returning id").bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    let path = format!("/v1/items/{id}/sync");
    let mut held = Vec::new();
    for _ in 0..4 {
        let response = r
            .clone()
            .oneshot(d.request("GET", &path, json!({}), None))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        held.push(response);
    }
    let response = r
        .clone()
        .oneshot(d.request("GET", &path, json!({}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), 429);
    assert!(response.headers().contains_key("retry-after"));
    held.clear();
    let mut request = d.request("GET", &path, json!({}), None);
    request
        .headers_mut()
        .insert("accept-encoding", "gzip".parse().unwrap());
    let response = r.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-encoding"], "gzip");
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let mut json = String::new();
    flate2::read::GzDecoder::new(bytes.as_ref())
        .read_to_string(&mut json)
        .unwrap();
    let bundle: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(bundle["items"][0]["id"], id.to_string());
    for query in [
        "include=unknown",
        "agenda=bad",
        "agenda=00000000-0000-0000-0000-000000000000:2020-01-01T00:00:00Z:2028-01-01T00:00:00Z",
    ] {
        assert_eq!(
            r.clone()
                .oneshot(d.request("GET", &format!("{path}?{query}"), json!({}), None))
                .await
                .unwrap()
                .status(),
            400
        );
    }
    sqlx::query("update porchlight.items set deleted_at=now() where id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        r.clone()
            .oneshot(d.request("GET", &path, json!({}), None))
            .await
            .unwrap()
            .status(),
        404
    );
    let _: Value = sqlx::query_scalar("select porchlight.pl_purge_items($1,$2)")
        .bind(s.init.household_id)
        .bind(vec![id])
        .fetch_one(&pool)
        .await
        .unwrap();
    let stored: bool =
        sqlx::query_scalar("select exists(select 1 from porchlight.items where id=$1)")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!stored);
    assert_eq!(
        r.clone()
            .oneshot(d.request("GET", &path, json!({}), None))
            .await
            .unwrap()
            .status(),
        404
    );
    let gone = uuid::Uuid::now_v7();
    assert_eq!(
        r.oneshot(d.request("GET", &format!("/v1/items/{gone}/sync"), json!({}), None))
            .await
            .unwrap()
            .status(),
        404
    );
    s.stop().await;
}

#[derive(Default, Clone)]
struct SqlCost(std::sync::Arc<std::sync::Mutex<(usize, u64)>>);
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for SqlCost {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" {
            return;
        }
        struct Rows(u64);
        impl tracing::field::Visit for Rows {
            fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
                if field.name() == "rows_returned" {
                    self.0 = value;
                }
            }
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
        }
        let mut rows = Rows(0);
        event.record(&mut rows);
        let mut cost = self.0.lock().unwrap();
        cost.0 += 1;
        cost.1 += rows.0;
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn cost_is_independent_of_other_items_and_bounds_dense_window(pool: PgPool) {
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::layer::SubscriberExt;
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let h = porchlight_db::testing::seed_household(&pool).await.unwrap();
    let c = caller(h, h.super_admin);
    let id:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind) values($1,$2,'Dense window','todo') returning id").bind(h.id).bind(h.super_admin).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq,due_date) select $1,$2,n,'2026-04-09'::date+n from generate_series(0,729) n").bind(h.id).bind(id).execute(&pool).await.unwrap();
    let _ = tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(tracing_subscriber::filter::LevelFilter::TRACE),
    );
    let mut measurements = Vec::new();
    for others in [10, 990] {
        sqlx::query("insert into porchlight.items(household_id,owner_id,title,kind) select $1,$2,'Other item','todo' from generate_series(1,$3)").bind(h.id).bind(h.super_admin).bind(others).execute(&pool).await.unwrap();
        // Other items have children too; a household read cannot hide in an empty extension.
        sqlx::query("insert into porchlight.item_occurrences(household_id,item_id,seq) select $1,id,0 from porchlight.items where household_id=$1 and title='Other item' on conflict(item_id,seq) do nothing").bind(h.id).execute(&pool).await.unwrap();
        let cost = SqlCost::default();
        let subscriber = tracing_subscriber::registry()
            .with(cost.clone())
            .with(tracing_subscriber::filter::LevelFilter::TRACE);
        let started = std::time::Instant::now();
        let bundle = direct_bundle(&s, c.clone(), id, Some("include=collaboration".into()))
            .with_subscriber(subscriber)
            .await;
        let latency = started.elapsed();
        let count = bundle["occurrences"].as_array().unwrap().len();
        assert!((720..=730).contains(&count));
        let size = serde_json::to_vec(&bundle).unwrap().len();
        let measured = *cost.0.lock().unwrap();
        assert!(measured.0 > 5, "SQL tracing must be enabled");
        assert!(
            measured.1 < 800,
            "only the dense item's rows are decoded: {measured:?}"
        );
        eprintln!(
            "item sync other_items={} statements={} decoded_rows={} occurrences={} bytes={} latency_ms={}",
            if others == 10 { 10 } else { 1000 },
            measured.0,
            measured.1,
            count,
            size,
            latency.as_millis()
        );
        measurements.push((measured, size));
    }
    assert_eq!(measurements[0], measurements[1]);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn inherited_lists_and_multiple_agenda_scopes(pool: PgPool) {
    use axum::extract::{Query, State};
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let h = porchlight_db::testing::seed_household(&pool).await.unwrap();
    let parent:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,due_date,rrule,recurrence_mode) values($1,$2,'Parent series','todo','2026-10-01','FREQ=DAILY;COUNT=3','calendar') returning id").bind(h.id).bind(h.super_admin).fetch_one(&pool).await.unwrap();
    let child:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,split_from_item_id,split_from_seq) values($1,$2,'Split series','todo',$3,1) returning id").bind(h.id).bind(h.super_admin).bind(parent).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.lists(household_id,item_id,owner_id,name,mode) values($1,$2,$3,'Inherited','per_occurrence'),($1,$4,$3,'Attached','persistent'),($1,null,$3,'Unrelated','persistent')").bind(h.id).bind(parent).bind(h.super_admin).bind(child).execute(&pool).await.unwrap();
    let c = caller(h, h.user);
    let query = format!(
        "include=collaboration&agenda={}:2026-10-01T00:00:00Z:2026-11-01T00:00:00Z&agenda={}:2026-11-01T00:00:00Z:2026-12-01T00:00:00Z",
        h.user, h.super_admin
    );
    let bundle = direct_bundle(&s, c.clone(), child, Some(query.clone())).await;
    assert_eq!(bundle["coverage"]["agenda"].as_array().unwrap().len(), 2);
    let lists = body(
        routes::lists::list(
            State(s.state.clone()),
            c.clone(),
            Query(routes::lists::ListsQuery {
                item_id: Some(child),
                ..Default::default()
            }),
        )
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(bundle["lists"], lists["items"]);
    assert_eq!(bundle["lists"].as_array().unwrap().len(), 2);
    let excess = std::iter::repeat_n(
        format!(
            "agenda={}:2026-10-01T00:00:00Z:2026-11-01T00:00:00Z",
            h.user
        ),
        9,
    )
    .collect::<Vec<_>>()
    .join("&");
    let error = routes::item_sync::get(
        State(s.state.clone()),
        c,
        axum::extract::Path(child),
        axum::extract::RawQuery(Some(excess)),
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.problem(),
        porchlight_api::error::Problem::InvalidRequest
    );
    s.stop().await;
}

#[test]
fn capability_is_optional_and_bundle_reuses_snapshot_schemas() {
    let doc: Value = serde_json::from_str(&porchlight_api::openapi::json()).unwrap();
    let schemas = &doc["components"]["schemas"];
    assert!(
        !schemas["MeResponse"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("capabilities"))
    );
    for key in [
        "items",
        "occurrences",
        "scheduled_reminders",
        "suggestions",
        "comments",
        "attachments",
        "lists",
    ] {
        assert_eq!(
            schemas["ItemSyncRead"]["properties"][key]["items"],
            schemas["SnapshotRead"]["properties"][key]["items"],
            "{key}"
        );
    }
}
