use axum::{Router, body::to_bytes};
use chrono::{DateTime, Utc};
use porchlight_api::{
    routes,
    server::{ConnInfo, Transport},
    testing::{HouseholdFixture, TestOptions},
};
use porchlight_db::{
    derive,
    tx::{Actor, begin_as},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;
fn now() -> DateTime<Utc> {
    "2026-10-05T13:00:00Z".parse().unwrap()
}
async fn body(r: axum::response::Response) -> Value {
    assert_eq!(r.status(), 200);
    serde_json::from_slice(&to_bytes(r.into_body(), 8 * 1024 * 1024).await.unwrap()).unwrap()
}
fn router(f: &HouseholdFixture, agent: bool) -> Router {
    if agent {
        routes::agent_router(f.server.state.clone()).layer(axum::Extension(ConnInfo {
            transport: Transport::Agent,
            client_addr: None,
            client_cert_fingerprint: None,
        }))
    } else {
        routes::mtls_router(f.server.state.clone()).layer(axum::Extension(ConnInfo {
            transport: Transport::Lan,
            client_addr: None,
            client_cert_fingerprint: Some(f.admin.device.fingerprint),
        }))
    }
}
#[sqlx::test(migrations = "../db/migrations")]
async fn snapshot_planning_is_private_acl_filtered_and_search_has_no_duplicate_sources(
    pool: PgPool,
) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let household = f.server.init.household_id;
    let member = f.admin.member_id;
    // All extensions must survive integration on the shared RR snapshot path.
    let source_response = router(&f, false)
        .oneshot(f.admin.device.request(
            "POST",
            "/v1/feeds",
            json!({"name":"Planning integration feed","url":"https://example.invalid/calendar"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(source_response.status(), 201);
    let source: Value = serde_json::from_slice(
        &to_bytes(source_response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    let list_response = router(&f, false)
        .oneshot(f.admin.device.request(
            "POST",
            "/v1/lists",
            json!({"name":"Planning integration list","visibility":"private"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(list_response.status(), 201);
    let list: Value = serde_json::from_slice(
        &to_bytes(list_response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    for m in [member, f.users[0].member_id] {
        sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,0,'09:00','10:00')").bind(household).bind(m).execute(&pool).await.unwrap();
    }
    let mut visible_ids = Vec::new();
    let mut all_ids = Vec::new();
    let mut all_suggestions = Vec::new();
    let mut collaboration_ids = Vec::new();
    let blob_hash = "c".repeat(64);
    sqlx::query("INSERT INTO porchlight.blobs(household_id,sha256,size,content_type) VALUES($1,$2,1,'text/plain')")
        .bind(household).bind(&blob_hash).execute(&pool).await.unwrap();
    for (vis, relation) in [
        ("household", "other"),
        ("private", "owner"),
        ("private", "other"),
        ("selected", "viewer"),
        ("selected", "other"),
        ("private", "dependent-subject"),
        ("private", "subject"),
        ("private", "assignee"),
    ] {
        let item:Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,subject_member_id,title,kind,visibility,time_mode,due_date,est_minutes) values($1,$2,$3,'integrationplanning','todo',$4,'floating','2026-10-12',90) returning id").bind(household).bind(if relation=="owner" {member}else{f.users[0].member_id}).bind(if relation=="subject"{Some(member)}else if relation=="dependent-subject"{Some(f.dependent)}else{None}).bind(vis).fetch_one(&pool).await.unwrap();
        if relation == "viewer" {
            sqlx::query("insert into porchlight.item_viewers(household_id,item_id,member_id) values($1,$2,$3)").bind(household).bind(item).bind(member).execute(&pool).await.unwrap();
        }
        if relation == "assignee" {
            sqlx::query("insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)").bind(household).bind(item).bind(member).execute(&pool).await.unwrap();
        }
        let mut tx = begin_as(&f.server.state.pool, &Actor::system())
            .await
            .unwrap();
        derive::derive_item(&mut tx, item, now()).await.unwrap();
        tx.commit().await.unwrap();
        let occ: Uuid =
            sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1")
                .bind(item)
                .fetch_one(&pool)
                .await
                .unwrap();
        let suggestion:Uuid=sqlx::query_scalar("insert into porchlight.schedule_suggestions(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id) values($1,$2,$3,$4,'2026-10-12 13:00Z','2026-10-12 14:30Z',$5) returning id").bind(household).bind(member).bind(item).bind(occ).bind(Uuid::now_v7()).fetch_one(&pool).await.unwrap();
        let can_read: bool = sqlx::query_scalar("select porchlight.pl_item_can_read($1,$2)")
            .bind(item)
            .bind(member)
            .fetch_one(&pool)
            .await
            .unwrap();
        // Dependents cannot read even items about them or household-visible items.
        let dependent_sql: bool = sqlx::query_scalar("select porchlight.pl_item_can_read($1,$2)")
            .bind(item)
            .bind(f.dependent)
            .fetch_one(&pool)
            .await
            .unwrap();
        let dependent_rust =
            porchlight_core::access::can_view(&porchlight_core::access::AccessContext {
                caller: porchlight_core::access::CallerKind::AppMember(
                    porchlight_core::access::Role::Dependent,
                ),
                visibility: match vis {
                    "household" => porchlight_core::access::Visibility::Household,
                    "selected" => porchlight_core::access::Visibility::Selected,
                    _ => porchlight_core::access::Visibility::Private,
                },
                is_owner: false,
                is_assignee: false,
                is_viewer: false,
                is_subject: relation == "dependent-subject",
                unassigned_dependent_item: relation == "dependent-subject",
            });
        assert_eq!(dependent_sql, dependent_rust);
        assert!(!dependent_sql);
        let get = router(&f, false)
            .oneshot(
                f.admin
                    .device
                    .request("GET", &format!("/v1/items/{item}"), Value::Null, None),
            )
            .await
            .unwrap();
        assert_eq!(
            get.status().as_u16(),
            if can_read { 200 } else { 404 },
            "SQL/Rust ACL mismatch {vis}/{relation}"
        );
        if can_read {
            visible_ids.push(item);
        }
        let comment: Uuid = sqlx::query_scalar("INSERT INTO porchlight.comments(household_id,item_id,author_id,body) VALUES($1,$2,$3,'snapshot comment') RETURNING id")
            .bind(household).bind(item).bind(member).fetch_one(&pool).await.unwrap();
        let attachment: Uuid = sqlx::query_scalar("INSERT INTO porchlight.attachments(household_id,item_id,blob_sha256,original_name,uploader_id) VALUES($1,$2,$3,'snapshot.txt',$4) RETURNING id")
            .bind(household).bind(item).bind(&blob_hash).bind(member).fetch_one(&pool).await.unwrap();
        collaboration_ids.push((item, comment, attachment));
        all_ids.push(item);
        all_suggestions.push(suggestion);
    }
    // A private row of another member must not leak even through app super-admin snapshot.
    sqlx::query("insert into porchlight.schedule_suggestions(household_id,member_id,item_id,occurrence_id,proposed_start,proposed_end,run_id) select household_id,$1,item_id,occurrence_id,proposed_start,proposed_end,run_id from porchlight.schedule_suggestions where id=$2").bind(f.users[0].member_id).bind(all_suggestions[0]).execute(&pool).await.unwrap();
    for agent in [false, true] {
        let mut request = f
            .admin
            .device
            .request("GET", "/v1/snapshot", Value::Null, None);
        if agent {
            request.headers_mut().insert(
                "authorization",
                format!("Bearer {}", f.server.bootstrap_token())
                    .parse()
                    .unwrap(),
            );
        }
        let snapshot = body(router(&f, agent).oneshot(request).await.unwrap()).await;
        let sources = snapshot["ics_sources"]
            .as_array()
            .expect("snapshot calendar extension alongside lists and planning");
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0]["id"], source["id"]);
        assert_eq!(sources[0]["host"], "example.invalid");
        assert!(sources[0].get("url").is_none());
        let lists = snapshot["lists"]
            .as_array()
            .expect("snapshot lists extension alongside planning");
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0]["id"], list["id"]);
        for (key, column) in [("comments", 1), ("attachments", 2)] {
            let rows = snapshot[key]
                .as_array()
                .expect("collaboration snapshot contribution");
            let expected: Vec<Uuid> = collaboration_ids
                .iter()
                .filter(|(item, _, _)| agent || visible_ids.contains(item))
                .map(|(_, comment, attachment)| if column == 1 { *comment } else { *attachment })
                .collect();
            assert_eq!(rows.len(), expected.len(), "{key}");
            assert!(
                rows.iter()
                    .all(|r| expected.contains(&r["id"].as_str().unwrap().parse().unwrap()))
            );
        }
        let availability = snapshot["availability"]
            .as_array()
            .expect("snapshot availability extension");
        assert_eq!(availability.len(), if agent { 4 } else { 1 });
        let suggestions = snapshot["suggestions"]
            .as_array()
            .expect("snapshot suggestions extension");
        assert_eq!(
            suggestions.len(),
            if agent {
                all_ids.len() + 1
            } else {
                visible_ids.len()
            }
        );
        for s in suggestions {
            if !agent {
                assert_eq!(s["member_id"], json!(member));
                assert!(visible_ids.contains(&s["item_id"].as_str().unwrap().parse().unwrap()));
            }
        }
        let mut request =
            f.admin
                .device
                .request("GET", "/v1/suggestions?limit=200", Value::Null, None);
        if agent {
            request.headers_mut().insert(
                "authorization",
                format!("Bearer {}", f.server.bootstrap_token())
                    .parse()
                    .unwrap(),
            );
        }
        let rest = body(router(&f, agent).oneshot(request).await.unwrap()).await;
        for suggestion in suggestions
            .iter()
            .filter(|s| s["member_id"] == json!(member))
        {
            assert_eq!(
                Some(suggestion),
                rest["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|s| s["id"] == suggestion["id"]),
                "REST and snapshot share the typed suggestion response"
            );
        }
        assert!(snapshot["cursor"].is_i64());
        let mut request = f.admin.device.request(
            "GET",
            "/v1/search?q=integrationplanning&limit=200",
            Value::Null,
            None,
        );
        if agent {
            request.headers_mut().insert(
                "authorization",
                format!("Bearer {}", f.server.bootstrap_token())
                    .parse()
                    .unwrap(),
            );
        }
        let search = body(router(&f, agent).oneshot(request).await.unwrap()).await;
        assert_eq!(
            search["items"].as_array().unwrap().len(),
            if agent {
                all_ids.len()
            } else {
                visible_ids.len()
            }
        );
        assert!(
            search["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["entity_type"] == "item"),
            "planning rows have no independent text document"
        );
    }
    // Both existing item reads and the S4b agenda read retain load warnings.
    let agenda = body(
        router(&f, false)
            .oneshot(f.admin.device.request(
                "GET",
                "/v1/agenda?from=2026-10-12T00:00:00Z&to=2026-10-13T00:00:00Z",
                Value::Null,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(
        agenda["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["conflicts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["kind"] == "load")),
        "agenda load warning lost in rebase"
    );
    let list = body(
        router(&f, false)
            .oneshot(
                f.admin
                    .device
                    .request("GET", "/v1/items?limit=200", Value::Null, None),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["conflicts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["kind"] == "load")),
        "item load warning lost in rebase"
    );
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn suggest_body_member_references_use_r38(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let foreign_h: Uuid = sqlx::query_scalar(
        "insert into porchlight.households(name) values('Foreign fixture') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let foreign: Uuid = sqlx::query_scalar("insert into porchlight.household_members(household_id,role,display_name,color,tzid) values($1,'dependent','Foreign','#123456','UTC') returning id").bind(foreign_h).fetch_one(&pool).await.unwrap();
    sqlx::query("update porchlight.household_members set deleted_at=now() where id=$1")
        .bind(f.users[1].member_id)
        .execute(&pool)
        .await
        .unwrap();
    for member in [Uuid::now_v7(), foreign, f.users[1].member_id] {
        for agent in [false, true] {
            let mut request = f.admin.device.request(
                "POST",
                "/v1/schedule/suggest",
                json!({"member_id":member}),
                Some(&Uuid::now_v7().to_string()),
            );
            if agent {
                request.headers_mut().insert(
                    "authorization",
                    format!("Bearer {}", f.server.bootstrap_token())
                        .parse()
                        .unwrap(),
                );
            }
            let response = router(&f, agent).oneshot(request).await.unwrap();
            assert_eq!(
                response.status(),
                422,
                "body member reference must not be a path 404"
            );
            let problem: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert!(
                problem["type"]
                    .as_str()
                    .unwrap()
                    .ends_with("validation-failed")
            );
            assert_eq!(problem["errors"][0]["field"], "member_id");
            assert_eq!(problem["errors"][0]["code"], "invalid-reference");
        }
    }
    // Valid references retain policy failures (other nondependent cannot be run).
    let response = router(&f, false)
        .oneshot(f.admin.device.request(
            "POST",
            "/v1/schedule/suggest",
            json!({"member_id":f.users[0].member_id}),
            Some(&Uuid::now_v7().to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn worker_load_and_item_batches_share_injected_today(pool: PgPool) {
    // A date far from the wall clock makes this deterministic, including zone-local today.
    let fixed: DateTime<Utc> = "2020-01-06T01:00:00Z".parse().unwrap();
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(move || fixed),
            ..Default::default()
        },
    )
    .await;
    let h = f.server.init.household_id;
    // First member is recomputed only through availability's member-only batch.
    let load_member = f.users[0].member_id;
    sqlx::query("update porchlight.household_members set tzid='America/New_York' where id=$1")
        .bind(load_member)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) values($1,$2,6,'09:00','10:00')").bind(h).bind(load_member).execute(&pool).await.unwrap();
    sqlx::query("insert into porchlight.member_daily_load(member_id,date,planned_minutes,available_minutes,starts_at,ends_at,derived_at) values($1,'2020-01-05',90,60,'2020-01-05T05:00Z','2020-01-06T05:00Z',now())").bind(load_member).execute(&pool).await.unwrap();
    // Old overload enters a leave transition on fixed member-local today.
    let item: Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,time_mode,due_date,est_minutes) values($1,$2,'Clock regression','todo','floating','2020-01-07',30) returning id").bind(h).bind(f.admin.member_id).fetch_one(&pool).await.unwrap();
    sqlx::query("update porchlight.item_derivation set retry_at=$2 where item_id=$1")
        .bind(item)
        .bind(fixed)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update porchlight.member_load_jobs set retry_at=$1")
        .bind(fixed)
        .execute(&pool)
        .await
        .unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let worker_pool = f.server.state.pool.clone();
    let worker = tokio::spawn(async move {
        porchlight_api::derive_worker::run_with_clock(worker_pool, Arc::new(move || fixed), async {
            let _ = stopped.await;
        })
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let done: bool = sqlx::query_scalar("select applied_gen=requested_gen from porchlight.item_derivation where item_id=$1").bind(item).fetch_one(&pool).await.unwrap();
            let load_done: bool = sqlx::query_scalar("select applied_gen=requested_gen from porchlight.member_load_jobs where member_id=$1").bind(load_member).fetch_one(&pool).await.unwrap();
            if done && load_done {break;}
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }).await.unwrap();
    stop.send(()).unwrap();
    worker.await.unwrap().unwrap();
    let item_time: DateTime<Utc> = sqlx::query_scalar(
        "select derived_at from porchlight.member_agenda where item_id=$1 limit 1",
    )
    .bind(item)
    .fetch_one(&pool)
    .await
    .unwrap();
    let load_time: DateTime<Utc> =
        sqlx::query_scalar("select retry_at from porchlight.member_load_jobs where member_id=$1")
            .bind(load_member)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(item_time, fixed);
    assert_eq!(
        load_time, fixed,
        "load batch must use the injected item clock"
    );
    let dates: Vec<String> = sqlx::query_scalar("select payload->>'date' from porchlight.notification_events where recipient_member_id=$1 and kind='load_conflict'").bind(load_member).fetch_all(&pool).await.unwrap();
    assert_eq!(
        dates,
        vec!["2020-01-05"],
        "notification window must start on member-local today"
    );
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn dependent_planning_stays_visible_to_admin_but_never_notifies_dependent(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let h = f.server.init.household_id;
    sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) select $1,$2,d,'09:00','10:00' from generate_series(0,6) d").bind(h).bind(f.dependent).execute(&pool).await.unwrap();
    for _ in 0..2 {
        let item: Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,subject_member_id,title,kind,time_mode,due_date,est_minutes) values($1,$2,$3,'Dependent work','todo','floating','2026-10-12',45) returning id").bind(h).bind(f.admin.member_id).bind(f.dependent).fetch_one(&pool).await.unwrap();
        let mut tx = begin_as(&f.server.state.pool, &Actor::system())
            .await
            .unwrap();
        derive::derive_item(&mut tx, item, now()).await.unwrap();
        tx.commit().await.unwrap();
    }
    let run = body(
        router(&f, false)
            .oneshot(f.admin.device.request(
                "POST",
                "/v1/schedule/suggest",
                json!({"member_id":f.dependent}),
                Some(&Uuid::now_v7().to_string()),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(run["suggestions"].as_array().unwrap().len(), 2);
    let load_conflicts: i64 = sqlx::query_scalar("select count(*) from porchlight.member_daily_load where member_id=$1 and planned_minutes>available_minutes").bind(f.dependent).fetch_one(&pool).await.unwrap();
    assert!(
        load_conflicts > 0,
        "dependent overload is still computed for admin reads"
    );
    let events: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_events where recipient_member_id=$1 and kind in ('load_conflict','suggestion')").bind(f.dependent).fetch_one(&pool).await.unwrap();
    assert_eq!(
        events, 0,
        "dependents cannot consume member-private planning events"
    );
    f.server.stop().await;
}
