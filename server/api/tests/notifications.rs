use axum::http::StatusCode;
use axum::{Router, body::to_bytes};
use chrono::{DateTime, Utc};
use porchlight_api::{
    routes,
    server::{ConnInfo, Transport},
    testing::{TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use porchlight_db::testing::{Role, insert_appointment, pool_as, seed_household};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
fn at(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}
async fn emit(pool: &PgPool, h: Uuid, m: Uuid, item: Option<Uuid>, kind: &str, t: DateTime<Utc>) {
    sqlx::query("insert into porchlight.notification_events(household_id,recipient_member_id,kind,item_id,entity_type,entity_id,created_at) values($1,$2,$3,$4,'item',$5,$6)")
        .bind(h).bind(m).bind(kind).bind(item).bind(item.unwrap_or(m)).bind(t).execute(pool).await.unwrap();
}
/// Consume/release as the production app login so a privileged notifier statement fails CI (N-m1);
/// fixtures and setup stay on the superuser pool.
async fn drain(pool: &PgPool, now: DateTime<Utc>) -> anyhow::Result<usize> {
    let app = pool_as(pool, Role::App, Some("notifier-app")).await?;
    let out = porchlight_api::notifier::drain(&app, now).await;
    app.close().await;
    out
}
#[sqlx::test(migrations = "../db/migrations")]
async fn window_edges_restart_preferences_and_digest(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let t = at("2026-10-05T12:00:00Z");
    for seconds in [0, 29, 30] {
        emit(
            &pool,
            h.id,
            h.super_admin,
            Some(item),
            "assigned",
            t + chrono::Duration::seconds(seconds),
        )
        .await;
    }
    assert_eq!(
        drain(&pool, t + chrono::Duration::seconds(31))
            .await
            .unwrap(),
        3
    );
    let counts: Vec<i32> =
        sqlx::query_scalar("select count from porchlight.notifications order by window_start")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(counts, vec![2, 1]);
    assert_eq!(
        drain(&pool, t + chrono::Duration::seconds(31))
            .await
            .unwrap(),
        0
    );
    sqlx::query("update porchlight.notification_prefs set quiet_start='22:00',quiet_end='07:00' where member_id=$1").bind(h.super_admin).execute(&pool).await.unwrap();
    // Seeded member zone is New York: overnight batch crosses the fallback transition.
    let night = at("2026-11-01T03:00:00Z");
    for seconds in [0, 3600] {
        emit(
            &pool,
            h.id,
            h.super_admin,
            Some(item),
            "completed",
            night + chrono::Duration::seconds(seconds),
        )
        .await;
    }
    drain(&pool, at("2026-11-01T05:00:00Z")).await.unwrap();
    let (count, release, released): (i32, DateTime<Utc>, Option<DateTime<Utc>>) = sqlx::query_as(
        "select count,release_at,released_at from porchlight.notifications where category='digest'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 2);
    assert_eq!(release, at("2026-11-01T12:00:00Z"));
    assert!(released.is_none());
    drain(&pool, release).await.unwrap();
    let emitted:i64=sqlx::query_scalar("select count(*) from porchlight.outbox where kind='notification.created' and payload->>'category'='digest'").fetch_one(&pool).await.unwrap();
    assert_eq!(emitted, 1);
    sqlx::query("update porchlight.notification_prefs set assignments=false where member_id=$1")
        .bind(h.super_admin)
        .execute(&pool)
        .await
        .unwrap();
    emit(&pool, h.id, h.super_admin, Some(item), "assigned", release).await;
    emit(&pool, h.id, h.super_admin, Some(item), "reminder", release).await;
    drain(&pool, release).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,i64>("select count(*) from porchlight.notifications where category='reminders' and released_at is not null").fetch_one(&pool).await.unwrap(),1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "select count(*) from porchlight.notifications where category='assignments'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        2
    );
}
#[test]
fn quiet_hours_and_dependent_slots_dst() {
    use porchlight_api::notifier::{dependent_slots, quiet_release};
    let z = chrono_tz::America::New_York;
    let start = "22:00:00".parse().unwrap();
    let end = "07:00:00".parse().unwrap();
    assert_eq!(
        quiet_release(at("2026-03-08T04:00:00Z"), z, Some(start), Some(end)),
        Some(at("2026-03-08T11:00:00Z"))
    );
    assert_eq!(
        quiet_release(at("2026-03-08T11:00:00Z"), z, Some(start), Some(end)),
        None
    );
    let slots = dependent_slots(at("2026-03-09T14:00:00Z"), z);
    assert_eq!(slots[0].1, at("2026-03-08T22:00:00Z"));
    assert_eq!(slots[1].1, at("2026-03-07T14:00:00Z"));
    assert_eq!(slots[2].1, at("2026-03-08T14:00:00Z"));
}
fn router(s: &TestServer, fp: [u8; 32]) -> Router {
    routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(fp),
    }))
}
async fn body(r: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(r.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}
#[sqlx::test(migrations = "../db/migrations")]
async fn inbox_prefs_visibility_read_dismissal_and_single_connection(pool: PgPool) {
    let t = at("2026-10-05T12:00:00Z");
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            db_pool_size: 1,
            background_derivation: false,
            item_clock: std::sync::Arc::new(move || t),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let r = router(&s, d.fingerprint);
    let prefs = r
        .clone()
        .oneshot(d.request("GET", "/v1/me/notification-prefs", json!({}), None))
        .await
        .unwrap();
    assert_eq!(prefs.status(), 200);
    let p = body(prefs).await;
    let missing = r
        .clone()
        .oneshot(d.request(
            "PUT",
            "/v1/me/notification-prefs",
            p["prefs"].clone(),
            Some("prefs-missing"),
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), 428);
    let item:Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,visibility) values($1,$2,'No leaked titles','todo','private') returning id").bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    emit(
        &pool,
        s.init.household_id,
        s.init.admin_member_id,
        Some(item),
        "assigned",
        t,
    )
    .await;
    drain(&pool, t).await.unwrap();
    let v = body(
        r.clone()
            .oneshot(d.request("GET", "/v1/notifications?unread=true", json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let id = v["items"][0]["id"].as_str().unwrap();
    let path = format!("/v1/notifications/{id}/read");
    let read = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        r.clone()
            .oneshot(d.request("POST", &path, json!({}), Some("read-one"))),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(read.status(), 200);
    let row = body(read).await;
    assert!(row["read_at"].is_string());
    assert_eq!(sqlx::query_scalar::<_,i64>("select count(*) from porchlight.outbox where kind='notification.dismissed' and payload->>'member_id'=$1").bind(s.init.admin_member_id.to_string()).fetch_one(&pool).await.unwrap(),1);
    let replay = body(
        r.clone()
            .oneshot(d.request("POST", &path, json!({}), Some("read-one")))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(row, replay);
    sqlx::query("update porchlight.items set deleted_at=now() where id=$1")
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    let hidden = body(
        r.clone()
            .oneshot(d.request("GET", "/v1/notifications", json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    assert!(hidden["items"].as_array().unwrap().is_empty());
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn scheduler_reminders_and_dependent_slots_are_idempotent(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let t = at("2026-03-09T14:00:00Z");
    sqlx::query(
        "update porchlight.items set subject_member_id=$2,starts_at=$3,ends_at=$4 where id=$1",
    )
    .bind(item)
    .bind(h.dependent)
    .bind(t)
    .bind(t + chrono::Duration::hours(1))
    .execute(&pool)
    .await
    .unwrap();
    let mut tx = porchlight_db::tx::begin_as(&pool, &porchlight_db::tx::Actor::system())
        .await
        .unwrap();
    sqlx::query("select id from porchlight.items where id=$1 for update")
        .bind(item)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    porchlight_db::derive::derive_item(&mut tx, item, at("2026-03-01T12:00:00Z"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for now in [
        at("2026-03-07T13:59:59Z"),
        at("2026-03-07T14:00:00Z"),
        at("2026-03-08T14:00:00Z"),
        at("2026-03-08T22:00:00Z"),
        at("2026-03-08T22:00:00Z"),
    ] {
        let mut tx = pool.begin().await.unwrap();
        porchlight_api::notifier::dependents(&mut tx, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let slots:Vec<(String,i64)>=sqlx::query_as("select payload->>'slot',count(*) from porchlight.notification_events where kind='dependent_unassigned' group by 1 order by 1").fetch_all(&pool).await.unwrap();
    assert_eq!(
        slots,
        vec![("24h".into(), 2), ("48h".into(), 2), ("weekly".into(), 2)]
    );
    let owned = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set starts_at=$2,ends_at=$3 where id=$1")
        .bind(owned)
        .bind(t)
        .bind(t + chrono::Duration::hours(1))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into porchlight.item_reminders(household_id,item_id,offset_minutes) values($1,$2,60)").bind(h.id).bind(owned).execute(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("select id from porchlight.items where id=$1 for update")
        .bind(owned)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    porchlight_db::derive::derive_item(&mut tx, owned, t - chrono::Duration::days(2))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for now in [
        t - chrono::Duration::minutes(61),
        t - chrono::Duration::minutes(60),
        t - chrono::Duration::minutes(59),
    ] {
        let mut tx = pool.begin().await.unwrap();
        porchlight_api::notifier::reminders(&mut tx, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let events: Vec<Value> = sqlx::query_scalar(
        "select payload from porchlight.notification_events where kind='reminder'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(events, vec![json!({"local_alarm":true})]);
}
#[sqlx::test(migrations = "../db/migrations")]
async fn coalesced_sources_recheck_acl_and_delayed_morning_is_one_batch(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let a = insert_appointment(&pool, &h).await.unwrap();
    let b = insert_appointment(&pool, &h).await.unwrap();
    let t = at("2026-10-05T12:00:00Z");
    emit(&pool, h.id, h.super_admin, Some(a), "comment", t).await;
    emit(&pool, h.id, h.super_admin, Some(b), "mention", t).await;
    drain(&pool, t).await.unwrap();
    sqlx::query("update porchlight.items set owner_id=$2,visibility='private' where id=$1")
        .bind(b)
        .bind(h.user)
        .execute(&pool)
        .await
        .unwrap();
    let id: Uuid =
        sqlx::query_scalar("select id from porchlight.notifications where recipient_member_id=$1")
            .bind(h.super_admin)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut c = pool.acquire().await.unwrap();
    let sources = porchlight_api::notifier::visible_sources(&mut c, id, h.super_admin, h.id)
        .await
        .unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].item_id, Some(a));
    drop(c);
    sqlx::query("update porchlight.notification_prefs set quiet_start='22:00',quiet_end='07:00' where member_id=$1").bind(h.super_admin).execute(&pool).await.unwrap();
    let night = at("2026-10-06T03:00:00Z");
    for i in 0..501 {
        emit(
            &pool,
            h.id,
            h.super_admin,
            Some(a),
            "comment",
            night + chrono::Duration::seconds(i),
        )
        .await;
    }
    let morning = at("2026-10-06T12:00:00Z");
    drain(&pool, morning).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,i64>("select count(*) from porchlight.outbox where kind='notification.created' and payload->>'category'='digest'").fetch_one(&pool).await.unwrap(),0);
    drain(&pool, morning).await.unwrap();
    let counts: Vec<i32> =
        sqlx::query_scalar("select count from porchlight.notifications where category='digest'")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(counts, vec![501]);
    assert_eq!(sqlx::query_scalar::<_,i64>("select count(*) from porchlight.outbox where kind='notification.created' and payload->>'category'='digest'").fetch_one(&pool).await.unwrap(),1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn generated_notification_endpoint_rows_over_tls_and_agent_socket(pool: PgPool) {
    use porchlight_api::{
        permissions::{self, Relationship},
        testing::{HouseholdFixture, TestClient, agent_request, trust},
    };
    let t = at("2026-10-05T12:00:00Z");
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(move || t),
            ..Default::default()
        },
    )
    .await;
    let rows = permissions::matrix_rows(permissions::NOTIFICATION_ENDPOINTS);
    assert_eq!(rows.len(), 60);
    let (_, write_pat) = trust::seed_pat(&pool, &f.server, &["write"]).await;
    let (_, read_pat) = trust::seed_pat(&pool, &f.server, &["read"]).await;
    for (index, row) in rows.into_iter().enumerate() {
        let who = if matches!(
            row.relationship,
            Relationship::SuperAdmin | Relationship::Agent
        ) {
            &f.admin
        } else {
            &f.users[0]
        };
        let recipient = if row.is_self {
            who.member_id
        } else {
            f.users[1].member_id
        };
        let now = t + chrono::Duration::minutes(index as i64);
        let item:Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,visibility) values($1,$2,'PRIVATE-NOTIFICATION-TITLE','todo',$3) returning id").bind(f.server.init.household_id).bind(recipient).bind(row.visibility.as_str()).fetch_one(&pool).await.unwrap();
        emit(
            &pool,
            f.server.init.household_id,
            recipient,
            Some(item),
            "assigned",
            now,
        )
        .await;
        drain(&pool, now).await.unwrap();
        let id:Uuid=sqlx::query_scalar("select n.id from porchlight.notifications n join porchlight.notification_sources s on s.notification_id=n.id join porchlight.notification_events e on e.id=s.event_id where e.item_id=$1").bind(item).fetch_one(&pool).await.unwrap();
        let mut path = row.endpoint.path.replace("{id}", &id.to_string());
        if row.endpoint.handler == "admin_list" {
            path.push_str(&format!("?member={recipient}"));
        }
        let prefs:Value=sqlx::query_scalar("select to_jsonb(p)-'member_id'-'household_id'-'version'-'created_at'-'updated_at' from porchlight.notification_prefs p where member_id=$1").bind(who.member_id).fetch_one(&pool).await.unwrap();
        let body = if row.endpoint.handler == "prefs_put" {
            prefs
        } else {
            json!({})
        };
        let mut req = who.device.request(
            row.endpoint.method,
            &path,
            body,
            Some(&format!("matrix-{index}")),
        );
        req.headers_mut().insert("if-match", "1".parse().unwrap());
        let response = if matches!(row.relationship, Relationship::Agent) {
            agent_request(
                &f.server.agent_socket,
                Some(match row.pat_scope {
                    Some(permissions::PatScope::Read) => &read_pat,
                    Some(permissions::PatScope::Write) => &write_pat,
                    _ => f.server.bootstrap_token(),
                }),
                req,
            )
            .await
        } else {
            TestClient::new(&f.server.ca_pem, Some(&who.device))
                .request(f.server.lan, req)
                .await
                .unwrap()
        };
        assert_eq!(response.status, row.status, "{row:?}");
        assert!(!String::from_utf8_lossy(&response.body).contains("PRIVATE-NOTIFICATION-TITLE"));
        if row.endpoint.handler == "list" {
            let has = response.json()["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["id"] == id.to_string());
            assert_eq!(has, row.is_self, "recipient isolation {row:?}");
        }
    }
    let (_, read_pat) = trust::seed_pat(&pool, &f.server, &["read"]).await;
    let req = f.admin.device.request(
        "GET",
        &format!("/v1/admin/notifications?member={}", f.admin.member_id),
        json!({}),
        None,
    );
    assert_eq!(
        agent_request(&f.server.agent_socket, Some(&read_pat), req)
            .await
            .status,
        403
    );
    // Stream delivery/replay proofs are intentionally deferred until S6b integration.
    let tickles: Vec<Value> = sqlx::query_scalar(
        "select payload from porchlight.outbox where kind like 'notification.%'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(!tickles.is_empty());
    for payload in tickles {
        let keys = payload.as_object().unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains_key("member_id") && keys.contains_key("category"));
    }
    f.server.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn prefs_replace_stale_noop_defaults_and_read_all(pool: PgPool) {
    let t = at("2026-10-05T12:00:00Z");
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            db_pool_size: 1,
            background_derivation: false,
            item_clock: std::sync::Arc::new(move || t),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let r = router(&s, d.fingerprint);
    sqlx::query(
        "alter table porchlight.notification_prefs add column future_setting boolean default false",
    )
    .execute(&pool)
    .await
    .unwrap();
    let initial = body(
        r.clone()
            .oneshot(d.request("GET", "/v1/me/notification-prefs", json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    assert!(initial["prefs"].get("future_setting").is_none());
    let mut fractional = initial["prefs"].clone();
    fractional["quiet_start"] = json!("22:00:00.500");
    fractional["quiet_end"] = json!("07:00:00");
    let mut req = d.request(
        "PUT",
        "/v1/me/notification-prefs",
        fractional,
        Some("fractional"),
    );
    req.headers_mut().insert("if-match", "1".parse().unwrap());
    assert_eq!(r.clone().oneshot(req).await.unwrap().status(), 422);
    let mut p = initial["prefs"].clone();
    p["quiet_start"] = json!("22:00:00");
    p["quiet_end"] = json!("07:00:00");
    p["default_appointment_offsets"] = json!([15]);
    let item:Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,visibility) values($1,$2,'Prefs','todo','private') returning id").bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&pool).await.unwrap();
    let before: i64 =
        sqlx::query_scalar("select requested_gen from porchlight.item_derivation where item_id=$1")
            .bind(item)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut req = d.request(
        "PUT",
        "/v1/me/notification-prefs",
        p.clone(),
        Some("replace"),
    );
    req.headers_mut().insert("if-match", "1".parse().unwrap());
    let response = tokio::time::timeout(std::time::Duration::from_secs(3), r.clone().oneshot(req))
        .await
        .expect("IdemTx only")
        .unwrap();
    assert_eq!(response.status(), 200);
    let saved = body(response).await;
    assert_eq!(saved["version"], 2);
    let after: i64 =
        sqlx::query_scalar("select requested_gen from porchlight.item_derivation where item_id=$1")
            .bind(item)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(after, before + 1);
    let mut req = d.request("PUT", "/v1/me/notification-prefs", p.clone(), Some("stale"));
    req.headers_mut().insert("if-match", "1".parse().unwrap());
    let response = r.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(body(response).await["current"], saved);
    let mut req = d.request("PUT", "/v1/me/notification-prefs", p, Some("noop"));
    req.headers_mut().insert("if-match", "2".parse().unwrap());
    assert_eq!(body(r.clone().oneshot(req).await.unwrap()).await, saved);
    for seconds in [0, 30] {
        emit(
            &pool,
            s.init.household_id,
            s.init.admin_member_id,
            Some(item),
            "assigned",
            t + chrono::Duration::seconds(seconds),
        )
        .await;
    }
    drain(&pool, t + chrono::Duration::seconds(30))
        .await
        .unwrap();
    let all = body(
        r.clone()
            .oneshot(d.request("POST", "/v1/notifications/read-all", json!({}), Some("all")))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(all["count"], 2);
    let no_more = body(
        r.clone()
            .oneshot(d.request(
                "POST",
                "/v1/notifications/read-all",
                json!({}),
                Some("all-again"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(no_more["count"], 0);
    let snapshot = body(
        r.clone()
            .oneshot(d.request("GET", "/v1/snapshot", json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(snapshot["notification_prefs"], saved);
    assert_eq!(snapshot["notifications"].as_array().unwrap().len(), 2);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn reminders_bypass_quiet_and_all_activity_preferences_are_honored(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let t = at("2026-11-01T03:00:00Z");
    sqlx::query("update porchlight.notification_prefs set quiet_start='22:00',quiet_end='07:00',activity=false,assignments=false,dependents=false where member_id=$1").bind(h.super_admin).execute(&pool).await.unwrap();
    for kind in [
        "assigned",
        "comment",
        "mention",
        "completed",
        "status_changed",
        "conflict",
        "load_conflict",
        "suggestion",
        "dependent_unassigned",
        "derivation_error",
        "reminder",
    ] {
        emit(&pool, h.id, h.super_admin, Some(item), kind, t).await;
    }
    assert_eq!(drain(&pool, t).await.unwrap(), 11);
    let categories: Vec<String> =
        sqlx::query_scalar("select category from porchlight.notifications")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(categories, vec!["reminders"]);
}
#[sqlx::test(migrations = "../db/migrations")]
async fn consumer_concurrency_and_failed_link_roll_back_every_effect(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let t = at("2026-10-05T12:00:00Z");
    emit(&pool, h.id, h.super_admin, Some(item), "assigned", t).await;
    sqlx::raw_sql("CREATE FUNCTION porchlight.pl_test_reject_source() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'test rejection'; END $$; CREATE TRIGGER test_reject_source BEFORE INSERT ON porchlight.notification_sources FOR EACH ROW EXECUTE FUNCTION porchlight.pl_test_reject_source();").execute(&pool).await.unwrap();
    assert_eq!(drain(&pool, t).await.unwrap(), 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from porchlight.notifications")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "select count(*) from porchlight.notification_events where processed_at is not null"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "select count(*) from porchlight.outbox where kind like 'notification.%'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    sqlx::raw_sql("DROP TRIGGER test_reject_source ON porchlight.notification_sources; DROP FUNCTION porchlight.pl_test_reject_source();").execute(&pool).await.unwrap();
    let t = t + chrono::Duration::seconds(2);
    for _ in 0..25 {
        emit(&pool, h.id, h.super_admin, Some(item), "assigned", t).await;
    }
    let (a, b) = tokio::join!(drain(&pool, t), drain(&pool, t));
    assert_eq!(a.unwrap() + b.unwrap(), 26);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from porchlight.notification_sources")
            .fetch_one(&pool)
            .await
            .unwrap(),
        26
    );
    assert_eq!(
        sqlx::query_scalar::<_, i32>("select count from porchlight.notifications")
            .fetch_one(&pool)
            .await
            .unwrap(),
        26
    );
}

// I-1: only two sessions hold locks. Observe the notifier's wait from the writer session.
#[sqlx::test(migrations = "../db/migrations")]
async fn member_lock_precedes_notifier_prefs_lock(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let t = at("2026-10-05T12:00:00Z");
    emit(&pool, h.id, h.super_admin, None, "assigned", t).await;
    let mut writer = pool.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
        .fetch_one(&mut *writer)
        .await
        .unwrap();
    sqlx::query("select id from porchlight.household_members where id=$1 for update")
        .bind(h.super_admin)
        .fetch_one(&mut *writer)
        .await
        .unwrap();
    let p = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    let consumer = tokio::spawn(async move { drain(&p, t).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if consumer.is_finished() {
                break;
            }
            sqlx::query("select pg_stat_clear_snapshot()")
                .execute(&mut *writer)
                .await
                .unwrap();
            let waiting: bool = sqlx::query_scalar(
                "select exists(select 1 from pg_stat_activity where $1=any(pg_blocking_pids(pid)))",
            )
            .bind(pid)
            .fetch_one(&mut *writer)
            .await
            .unwrap();
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let prefs = sqlx::query(
        "select member_id from porchlight.notification_prefs where member_id=$1 for update nowait",
    )
    .bind(h.super_admin)
    .fetch_one(&mut *writer)
    .await;
    writer.rollback().await.unwrap();
    let first = consumer.await.unwrap().unwrap();
    let next = drain(&pool, t).await.unwrap();
    assert_eq!(
        first + next,
        1,
        "skipped recipient is delivered after member unlock"
    );
    assert!(
        prefs.is_ok(),
        "notifier held prefs while waiting on the member"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn poisoned_recipient_does_not_block_healthy_delivery_or_release(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let morning = seed_household(&pool).await.unwrap();
    let night = at("2026-11-01T03:00:00Z");
    let t = at("2026-11-01T12:00:00Z");
    sqlx::query("update porchlight.notification_prefs set quiet_start='22:00',quiet_end='07:00' where member_id=$1").bind(morning.super_admin).execute(&pool).await.unwrap();
    emit(
        &pool,
        morning.id,
        morning.super_admin,
        None,
        "completed",
        night,
    )
    .await;
    drain(&pool, night).await.unwrap();
    emit(
        &pool,
        h.id,
        h.super_admin,
        None,
        "assigned",
        t - chrono::Duration::seconds(1),
    )
    .await;
    emit(&pool, h.id, h.user, None, "assigned", t).await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE FUNCTION porchlight.pl_test_poison() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.recipient_member_id='{}'::uuid THEN RAISE EXCEPTION 'poison'; END IF; RETURN NEW; END $$; CREATE TRIGGER test_poison BEFORE INSERT ON porchlight.notifications FOR EACH ROW EXECUTE FUNCTION porchlight.pl_test_poison();",h.super_admin))).execute(&pool).await.unwrap();
    assert_eq!(drain(&pool, t).await.unwrap(), 1);
    assert_eq!(sqlx::query_scalar::<_,i64>("select count(*) from porchlight.notifications where recipient_member_id=$1 and released_at is not null").bind(h.user).fetch_one(&pool).await.unwrap(),1);
    assert_eq!(sqlx::query_scalar::<_,i64>("select count(*) from porchlight.notifications where recipient_member_id=$1 and released_at is not null").bind(morning.super_admin).fetch_one(&pool).await.unwrap(),1);
    // Failure is atomic; a second invocation at the same injected instant must not retry the poison.
    assert_eq!(drain(&pool, t).await.unwrap(), 0);
    sqlx::raw_sql("DROP TRIGGER test_poison ON porchlight.notifications; DROP FUNCTION porchlight.pl_test_poison();").execute(&pool).await.unwrap();
    assert_eq!(drain(&pool, t).await.unwrap(), 0);
    assert_eq!(
        drain(&pool, t + chrono::Duration::seconds(2))
            .await
            .unwrap(),
        1
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn prefs_schema_extension_does_not_poison_consumer(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let t = at("2026-10-05T12:00:00Z");
    sqlx::query("alter table porchlight.notification_prefs add column future_setting boolean not null default false").execute(&pool).await.unwrap();
    emit(&pool, h.id, h.super_admin, None, "assigned", t).await;
    assert_eq!(drain(&pool, t).await.unwrap(), 1);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn dependent_countdowns_rearm_for_rescheduled_instant(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    let start = at("2026-03-09T14:00:00Z");
    for new_start in [start, start + chrono::Duration::days(7)] {
        sqlx::query(
            "update porchlight.items set subject_member_id=$2,starts_at=$3,ends_at=$4 where id=$1",
        )
        .bind(item)
        .bind(h.dependent)
        .bind(new_start)
        .bind(new_start + chrono::Duration::hours(1))
        .execute(&pool)
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("select id from porchlight.items where id=$1 for update")
            .bind(item)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        porchlight_db::derive::derive_item(&mut tx, item, new_start - chrono::Duration::days(9))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        for _ in 0..2 {
            let mut tx = pool.begin().await.unwrap();
            porchlight_api::notifier::dependents(&mut tx, new_start - chrono::Duration::hours(12))
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    }
    let slots:Vec<(String,i64)>=sqlx::query_as("select payload->>'slot',count(*) from porchlight.notification_events where kind='dependent_unassigned' group by 1 order by 1").fetch_all(&pool).await.unwrap();
    assert_eq!(
        slots,
        vec![("24h".into(), 4), ("48h".into(), 4), ("weekly".into(), 4)]
    );
}

#[test]
fn registered_kind_templates_are_safe_and_cleared_load_is_neutral() {
    use porchlight_api::notifier::{Source, summary};
    let source = |kind: &str, conflicting: Option<bool>| Source {
        id: Uuid::nil(),
        kind: kind.into(),
        item_id: None,
        occurrence_id: None,
        entity_type: "test".into(),
        entity_id: Uuid::nil(),
        actor_member_id: None,
        actor_label: None,
        actor_name: None,
        created_at: at("2026-10-05T12:00:00Z"),
        conflicting,
    };
    for (kind, title, category) in [
        ("assigned", "New assignment", "assignments"),
        ("status_changed", "Status changed", "activity"),
        ("completed", "Item completed", "activity"),
        ("comment", "New comment", "comments"),
        ("mention", "You were mentioned", "comments"),
        ("conflict", "Calendar conflict", "conflicts"),
        ("load_conflict", "Availability exceeded", "conflicts"),
        ("reminder", "Reminder", "reminders"),
        (
            "dependent_unassigned",
            "Unassigned appointment",
            "dependents",
        ),
        (
            "derivation_error",
            "Processing needs attention",
            "operational",
        ),
        ("feed_error", "Calendar feed needs attention", "operational"),
        ("calendar_changed", "Calendar updated", "activity"),
        ("suggestion", "Planning suggestion", "activity"),
        (
            "unknown_https://secret.invalid/token",
            "Porchlight activity",
            "activity",
        ),
    ] {
        let value = serde_json::to_value(summary(&[source(kind, Some(true))], false)).unwrap();
        assert_eq!(value["title"], title, "{kind}");
        assert_eq!(value["category"], category, "{kind}");
        assert!(!value["body"].as_str().unwrap().is_empty());
        assert!(!value.to_string().contains("secret.invalid"));
    }
    let cleared =
        serde_json::to_value(summary(&[source("load_conflict", Some(false))], false)).unwrap();
    assert_eq!(cleared["title"], "Availability restored");
    assert_eq!(cleared["body"], "Load back within availability.");
    assert!(
        !cleared["text"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("conflict")
    );
    let digest = serde_json::to_value(summary(&[source("comment", None)], true)).unwrap();
    assert_eq!(digest["text"], "Morning digest: 1 update");
    let reminders = serde_json::to_value(summary(
        &[source("reminder", None), source("reminder", None)],
        false,
    ))
    .unwrap();
    assert_eq!(reminders["text"], "Reminder history: 2 reminders");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn quiet_end_fold_and_gap_release_at_resolved_instant(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    for (night, end, release) in [
        ("2026-03-08T04:00:00Z", "02:30", "2026-03-08T07:00:00Z"),
        ("2026-11-01T03:00:00Z", "01:30", "2026-11-01T05:30:00Z"),
    ] {
        sqlx::query("update porchlight.notification_prefs set quiet_start='22:00',quiet_end=$2::text::time where member_id=$1").bind(h.super_admin).bind(end).execute(&pool).await.unwrap();
        emit(&pool, h.id, h.super_admin, None, "completed", at(night)).await;
        drain(&pool, at(night)).await.unwrap();
        let actual: DateTime<Utc> = sqlx::query_scalar(
            "select release_at from porchlight.notifications where released_at is null",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(actual, at(release));
        drain(&pool, actual - chrono::Duration::seconds(1))
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "select count(*) from porchlight.notifications where released_at is null"
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            1
        );
        drain(&pool, actual).await.unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "select count(*) from porchlight.notifications where released_at is null"
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            0
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn inbox_cursor_is_bound_to_member_and_unread_filter(pool: PgPool) {
    let t = at("2026-10-05T12:00:00Z");
    let f = porchlight_api::testing::HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(move || t),
            ..Default::default()
        },
    )
    .await;
    let s = &f.server;
    let admin = &f.admin.device;
    for seconds in [0, 31, 62] {
        emit(
            &pool,
            s.init.household_id,
            s.init.admin_member_id,
            None,
            "assigned",
            t + chrono::Duration::seconds(seconds),
        )
        .await;
    }
    drain(&pool, t + chrono::Duration::seconds(63))
        .await
        .unwrap();
    let r = router(s, admin.fingerprint);
    let page = body(
        r.clone()
            .oneshot(admin.request(
                "GET",
                "/v1/notifications?limit=1&unread=true",
                json!({}),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    let cursor = page["next_cursor"].as_str().unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let same = format!("/v1/notifications?limit=1&unread=true&cursor={cursor}");
    assert_eq!(
        r.clone()
            .oneshot(admin.request("GET", &same, json!({}), None))
            .await
            .unwrap()
            .status(),
        200
    );
    let changed = format!("/v1/notifications?limit=1&unread=false&cursor={cursor}");
    assert_eq!(
        r.clone()
            .oneshot(admin.request("GET", &changed, json!({}), None))
            .await
            .unwrap()
            .status(),
        400
    );
    let d = &f.users[0].device;
    assert_eq!(
        router(s, d.fingerprint)
            .oneshot(d.request("GET", &same, json!({}), None))
            .await
            .unwrap()
            .status(),
        400
    );
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn permission_loss_while_recipient_is_busy_is_rechecked_at_consumption(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    sqlx::query("update porchlight.items set visibility='household' where id=$1")
        .bind(item)
        .execute(&pool)
        .await
        .unwrap();
    let t = at("2026-10-05T12:00:00Z");
    emit(&pool, h.id, h.user, Some(item), "assigned", t).await;
    let mut writer = pool.begin().await.unwrap();
    sqlx::query("select id from porchlight.items where id=$1 for no key update")
        .bind(item)
        .fetch_one(&mut *writer)
        .await
        .unwrap();
    sqlx::query("select id from porchlight.household_members where id=$1 for update")
        .bind(h.user)
        .fetch_one(&mut *writer)
        .await
        .unwrap();
    let consumer = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    assert_eq!(drain(&consumer, t).await.unwrap(), 0);
    sqlx::query("update porchlight.items set visibility='private' where id=$1")
        .bind(item)
        .execute(&mut *writer)
        .await
        .unwrap();
    writer.commit().await.unwrap();
    assert_eq!(drain(&consumer, t).await.unwrap(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from porchlight.notifications")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn dependent_all_day_and_detached_override_use_effective_household_anchor(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let item = insert_appointment(&pool, &h).await.unwrap();
    // Floating all-day dates are resolved at midnight in the household zone.
    sqlx::query("update porchlight.items set subject_member_id=$2,time_mode='floating',all_day=true,tzid=null,starts_at=null,ends_at=null,local_start='2026-03-09 00:00',local_end='2026-03-10 00:00' where id=$1").bind(item).bind(h.dependent).execute(&pool).await.unwrap();
    async fn derive(pool: &PgPool, item: Uuid) {
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("select id from porchlight.items where id=$1 for update")
            .bind(item)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        porchlight_db::derive::derive_item(&mut tx, item, at("2026-03-01T12:00:00Z"))
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    derive(&pool, item).await;
    for (instant, expected) in [("2026-03-07T03:59:59Z", 0), ("2026-03-07T04:00:00Z", 2)] {
        let mut tx = pool.begin().await.unwrap();
        porchlight_api::notifier::dependents(&mut tx, at(instant))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(sqlx::query_scalar::<_,i64>("select count(*) from porchlight.notification_events where kind='dependent_unassigned' and payload->>'slot'='48h'").fetch_one(&pool).await.unwrap(),expected);
    }
    sqlx::query("update porchlight.item_occurrences set override_local_start='2026-03-16 00:00',override_local_end='2026-03-17 00:00' where item_id=$1").bind(item).execute(&pool).await.unwrap();
    derive(&pool, item).await;
    let mut tx = pool.begin().await.unwrap();
    porchlight_api::notifier::dependents(&mut tx, at("2026-03-14T04:00:00Z"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,i64>("select count(*) from porchlight.notification_events where kind='dependent_unassigned' and payload->>'slot'='48h'").fetch_one(&pool).await.unwrap(),4);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn consumer_payload_projection_clears_load_without_copying_secrets(pool: PgPool) {
    let t = at("2026-10-05T12:00:00Z");
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(move || t),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    sqlx::query("insert into porchlight.notification_events(household_id,recipient_member_id,kind,entity_type,entity_id,payload,created_at) values($1,$2,'load_conflict','member_daily_load',$2,$3,$4)").bind(s.init.household_id).bind(s.init.admin_member_id).bind(json!({"conflicting":false,"url":"https://secret.invalid/token","body":"PRIVATE-PAYLOAD"})).bind(t).execute(&pool).await.unwrap();
    drain(&pool, t).await.unwrap();
    let inbox = body(
        router(&s, d.fingerprint)
            .oneshot(d.request("GET", "/v1/notifications", json!({}), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        inbox["items"][0]["summary"]["title"],
        "Availability restored"
    );
    assert_eq!(
        inbox["items"][0]["summary"]["body"],
        "Load back within availability."
    );
    assert_eq!(inbox["items"][0]["summary"]["category"], "conflicts");
    assert!(!inbox.to_string().contains("PRIVATE-PAYLOAD"));
    assert!(!inbox.to_string().contains("secret.invalid"));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn poison_backoff_is_durable_bounded_and_cleared_after_success(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut now = at("2026-10-05T12:00:00Z");
    emit(&pool, h.id, h.super_admin, None, "assigned", now).await;
    sqlx::raw_sql("CREATE FUNCTION porchlight.pl_test_retry_poison() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'poison'; END $$; CREATE TRIGGER test_retry_poison BEFORE INSERT ON porchlight.notifications FOR EACH ROW EXECUTE FUNCTION porchlight.pl_test_retry_poison();").execute(&pool).await.unwrap();
    for attempt in 1..=4 {
        assert_eq!(drain(&pool, now).await.unwrap(), 0);
        let (attempts,retry):(i32,DateTime<Utc>)=sqlx::query_as("select attempts,retry_at from porchlight.notification_event_delivery where event_id=(select id from porchlight.notification_events where recipient_member_id=$1 and processed_at is null)").bind(h.super_admin).fetch_one(&pool).await.unwrap();
        assert_eq!(attempts, attempt);
        let seconds = (1_i64 << attempt).min(60);
        assert_eq!(retry - now, chrono::Duration::seconds(seconds));
        // Fresh invocation/pool models restart: durable state still prevents an early retry.
        let fresh = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_with((*pool.connect_options()).clone())
            .await
            .unwrap();
        assert_eq!(
            drain(&fresh, retry - chrono::Duration::seconds(1))
                .await
                .unwrap(),
            0
        );
        fresh.close().await;
        assert_eq!(
            sqlx::query_scalar::<_, i32>(
                "select attempts from porchlight.notification_event_delivery where event_id=(select id from porchlight.notification_events where recipient_member_id=$1 and processed_at is null)"
            )
            .bind(h.super_admin)
            .fetch_one(&pool)
            .await
            .unwrap(),
            attempts
        );
        now = retry;
    }
    sqlx::raw_sql("DROP TRIGGER test_retry_poison ON porchlight.notifications; DROP FUNCTION porchlight.pl_test_retry_poison();").execute(&pool).await.unwrap();
    assert_eq!(drain(&pool, now).await.unwrap(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from porchlight.notification_event_delivery")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn registered_unknown_kind_has_safe_default_and_clean_inbox(pool: PgPool) {
    let t = at("2026-10-05T12:00:00Z");
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(move || t),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let kind = "https://unknown.invalid/PRIVATE-KIND";
    sqlx::query("insert into porchlight.notification_kinds(kind,self_suppressed,description) values($1,false,'future kind') on conflict(kind) do nothing")
        .bind(kind).execute(&pool).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("select set_config('porchlight.notification_now',$1,true)")
        .bind(t.to_rfc3339())
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "select porchlight.pl_emit_notification_event($1,$2,$3,'household',$1,$4,null,null,null)",
    )
    .bind(s.init.household_id)
    .bind(s.init.admin_member_id)
    .bind(kind)
    .bind(json!({"body":"PRIVATE-PAYLOAD","url":"https://secret.invalid/token"}))
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(drain(&pool, t).await.unwrap(), 1);
    let response = router(&s, d.fingerprint)
        .oneshot(d.request("GET", "/v1/notifications", json!({}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let inbox = body(response).await;
    assert_eq!(inbox["items"].as_array().unwrap().len(), 1);
    let n = &inbox["items"][0];
    assert_eq!(n["category"], "activity");
    assert_eq!(n["summary"]["category"], "activity");
    assert_eq!(n["summary"]["title"], "Porchlight activity");
    assert_eq!(n["summary"]["body"], "New household activity is available.");
    assert_eq!(n["summary"]["text"], "Porchlight: Porchlight activity");
    assert!(!n["summary"].to_string().contains("PRIVATE"));
    assert!(!n["summary"].to_string().contains("invalid"));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn poisoned_event_is_skipped_after_five_and_backlog_digest_flow(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let night = at("2026-11-01T03:00:00Z");
    let morning = at("2026-11-01T12:00:00Z");
    sqlx::query("update porchlight.notification_prefs set quiet_start='22:00',quiet_end='07:00' where member_id=$1").bind(h.super_admin).execute(&pool).await.unwrap();
    emit(&pool, h.id, h.super_admin, None, "completed", night).await;
    assert_eq!(drain(&pool, night).await.unwrap(), 1);
    emit(&pool, h.id, h.super_admin, None, "assigned", morning).await;
    let poison: Uuid =
        sqlx::query_scalar("select id from porchlight.notification_events where kind='assigned'")
            .fetch_one(&pool)
            .await
            .unwrap();
    emit(&pool, h.id, h.super_admin, None, "feed_error", morning).await;
    // Poison the source link (not just the notification insert), proving the whole event unit rolls back.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE FUNCTION porchlight.pl_test_event_poison() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.event_id='{}'::uuid THEN RAISE EXCEPTION 'PRIVATE poison error'; END IF; RETURN NEW; END $$; CREATE TRIGGER test_event_poison BEFORE INSERT ON porchlight.notification_sources FOR EACH ROW EXECUTE FUNCTION porchlight.pl_test_event_poison();",poison))).execute(&pool).await.unwrap();
    let mut now = morning + chrono::Duration::seconds(1);
    for attempt in 1..=5 {
        let fresh = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect_with((*pool.connect_options()).clone())
            .await
            .unwrap();
        let count = drain(&fresh, now).await.unwrap();
        fresh.close().await;
        if attempt < 5 {
            assert_eq!(count, 0);
            assert_eq!(
                sqlx::query_scalar::<_, i64>("select count(*) from porchlight.notifications")
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                1,
                "failed source insertion must roll back new notification row"
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "select count(*) from porchlight.outbox where kind like 'notification.%'"
                )
                .fetch_one(&pool)
                .await
                .unwrap(),
                0,
                "failed event must leave no publication or premature digest release"
            );
            assert_eq!(drain(&pool, now).await.unwrap(), 0);
            now += chrono::Duration::seconds(1_i64 << attempt);
        } else {
            assert_eq!(
                count, 2,
                "fifth failure must skip poison and deliver later event"
            );
        }
    }
    let state:(i32,String,Option<DateTime<Utc>>)=sqlx::query_as("select attempts,last_error,poisoned_at from porchlight.notification_event_delivery where event_id=$1").bind(poison).fetch_one(&pool).await.unwrap();
    assert_eq!(state, (5, "database".into(), Some(now)));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "select count(*) from porchlight.notification_events where processed_at is null"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "select count(*) from porchlight.notification_sources where event_id=$1"
        )
        .bind(poison)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "select count(*) from porchlight.notifications where released_at is not null"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        2
    );
    assert_eq!(
        drain(&pool, now + chrono::Duration::days(1)).await.unwrap(),
        0
    );
    let agent = pool_as(&pool, Role::Agent, Some("agent:notification-admin"))
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "select last_error from porchlight.notification_event_delivery where event_id=$1"
        )
        .bind(poison)
        .fetch_one(&agent)
        .await
        .unwrap(),
        "database"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn successful_release_clears_stale_recipient_retry(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let night = at("2026-11-01T03:00:00Z");
    let morning = at("2026-11-01T12:00:00Z");
    sqlx::query("update porchlight.notification_prefs set quiet_start='22:00',quiet_end='07:00' where member_id=$1").bind(h.super_admin).execute(&pool).await.unwrap();
    emit(&pool, h.id, h.super_admin, None, "completed", night).await;
    drain(&pool, night).await.unwrap();
    sqlx::raw_sql("CREATE FUNCTION porchlight.pl_test_release_poison() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.released_at IS NOT NULL AND OLD.released_at IS NULL THEN RAISE EXCEPTION 'release poison'; END IF; RETURN NEW; END $$; CREATE TRIGGER test_release_poison BEFORE UPDATE ON porchlight.notifications FOR EACH ROW EXECUTE FUNCTION porchlight.pl_test_release_poison();").execute(&pool).await.unwrap();
    drain(&pool, morning).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from porchlight.notification_delivery_retry")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    sqlx::raw_sql("DROP TRIGGER test_release_poison ON porchlight.notifications; DROP FUNCTION porchlight.pl_test_release_poison();").execute(&pool).await.unwrap();
    drain(&pool, morning + chrono::Duration::seconds(2))
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from porchlight.notification_delivery_retry")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "select count(*) from porchlight.notifications where released_at is not null"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        1
    );
}

// SQLSTATE injection covers real savepoint rollback/backoff without inducing a
// deadlock or spending seconds on each lock/statement timeout.
#[sqlx::test(migrations = "../db/migrations")]
async fn transient_event_errors_have_six_hour_grace(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    for (index, code) in ["40P01", "40001", "55P03", "57014"].into_iter().enumerate() {
        let mut now = at("2026-10-05T12:00:00Z") + chrono::Duration::days(index as i64);
        emit(&pool, h.id, h.super_admin, None, "assigned", now).await;
        let event: Uuid = sqlx::query_scalar(
            "select id from porchlight.notification_events where processed_at is null",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE FUNCTION porchlight.pl_test_transient() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected transient' USING ERRCODE='{code}'; END $$; CREATE TRIGGER test_transient BEFORE INSERT ON porchlight.notification_sources FOR EACH ROW EXECUTE FUNCTION porchlight.pl_test_transient();")))
            .execute(&pool).await.unwrap();
        // More failures than the five-strike budget, with fresh app pools (restart).
        for attempt in 1..=7 {
            assert_eq!(drain(&pool, now).await.unwrap(), 0);
            let pending: bool = sqlx::query_scalar(
                "select processed_at is null from porchlight.notification_events where id=$1",
            )
            .bind(event)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(pending, "transient {code} must not poison within grace");
            let strikes: i64 = sqlx::query_scalar(
                "select coalesce(sum(attempts),0)::bigint from porchlight.notification_event_delivery where event_id=$1",
            )
            .bind(event)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(strikes, 0, "transient {code} charged to event poison");
            let (attempts, retry): (i32, DateTime<Utc>) = sqlx::query_as("select attempts,retry_at from porchlight.notification_delivery_retry where member_id=$1")
                .bind(h.super_admin).fetch_one(&pool).await.unwrap();
            assert_eq!(attempts, attempt.min(6));
            let mut retry = retry;
            if matches!(code, "55P03" | "57014") {
                // N1: timeout-class failures do not park the recipient; the failed
                // event's own retry_at carries the (growing) backoff.
                assert_eq!(retry, now);
                retry = sqlx::query_scalar(
                    "select retry_at from porchlight.notification_event_delivery where event_id=$1",
                )
                .bind(event)
                .fetch_one(&pool)
                .await
                .unwrap();
            }
            assert_eq!(
                retry - now,
                chrono::Duration::seconds((1_i64 << attempt).min(60))
            );
            assert_eq!(
                drain(&pool, retry - chrono::Duration::seconds(1))
                    .await
                    .unwrap(),
                0
            );
            now = retry;
        }
        sqlx::raw_sql("DROP TRIGGER test_transient ON porchlight.notification_sources; DROP FUNCTION porchlight.pl_test_transient();")
            .execute(&pool).await.unwrap();
        assert_eq!(drain(&pool, now).await.unwrap(), 1);
        let sources: i64 = sqlx::query_scalar(
            "select count(*) from porchlight.notification_sources where event_id=$1",
        )
        .bind(event)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(sources, 1);
        let retries: i64 = sqlx::query_scalar(
            "select count(*) from porchlight.notification_delivery_retry where member_id=$1",
        )
        .bind(h.super_admin)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(retries, 0);
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn transient_failure_preserves_prior_data_strikes_and_successes(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let mut now = at("2026-10-05T12:00:00Z");
    emit(&pool, h.id, h.super_admin, None, "assigned", now).await;
    let event: Uuid = sqlx::query_scalar("select id from porchlight.notification_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE FUNCTION porchlight.pl_test_mixed_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'bad data'; END $$; CREATE TRIGGER test_mixed_failure BEFORE INSERT ON porchlight.notification_sources FOR EACH ROW EXECUTE FUNCTION porchlight.pl_test_mixed_failure();")
        .execute(&pool).await.unwrap();
    assert_eq!(drain(&pool, now).await.unwrap(), 0);
    now += chrono::Duration::seconds(2);
    // An earlier event succeeds in the same batch, then the previously bad event
    // hits contention. Earlier delivery and the old data strike must survive.
    emit(
        &pool,
        h.id,
        h.super_admin,
        None,
        "feed_error",
        now - chrono::Duration::seconds(3),
    )
    .await;
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE OR REPLACE FUNCTION porchlight.pl_test_mixed_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.event_id='{event}'::uuid THEN RAISE EXCEPTION 'contention' USING ERRCODE='55P03'; END IF; RETURN NEW; END $$;")))
        .execute(&pool).await.unwrap();
    for attempt in 0..6 {
        assert_eq!(drain(&pool, now).await.unwrap(), usize::from(attempt == 0));
        let (attempts,poisoned): (i32,Option<DateTime<Utc>>) = sqlx::query_as("select attempts,poisoned_at from porchlight.notification_event_delivery where event_id=$1")
            .bind(event).fetch_one(&pool).await.unwrap();
        assert_eq!(attempts, 1);
        assert!(poisoned.is_none());
        let delivered: i64 =
            sqlx::query_scalar("select count(*) from porchlight.notification_sources")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            delivered, 1,
            "earlier success must survive transient failure"
        );
        now = sqlx::query_scalar(
            "select retry_at from porchlight.notification_event_delivery where event_id=$1",
        )
        .bind(event)
        .fetch_one(&pool)
        .await
        .unwrap();
    }
    sqlx::raw_sql("DROP TRIGGER test_mixed_failure ON porchlight.notification_sources; DROP FUNCTION porchlight.pl_test_mixed_failure();")
        .execute(&pool).await.unwrap();
    assert_eq!(drain(&pool, now).await.unwrap(), 1);
    let sources: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_sources")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(sources, 2);
    let strikes: i64 =
        sqlx::query_scalar("select count(*) from porchlight.notification_event_delivery")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(strikes, 0);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn persistent_transients_poison_after_grace(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    for (index, code) in ["40P01", "40001", "55P03", "57014"].into_iter().enumerate() {
        let start = at("2026-10-05T12:00:00Z") + chrono::Duration::days(index as i64);
        emit(&pool, h.id, h.super_admin, None, "assigned", start).await;
        let event: Uuid = sqlx::query_scalar(
            "select id from porchlight.notification_events where processed_at is null",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE FUNCTION porchlight.pl_test_transient() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected transient' USING ERRCODE='{code}'; END $$; CREATE TRIGGER test_transient BEFORE INSERT ON porchlight.notification_sources FOR EACH ROW EXECUTE FUNCTION porchlight.pl_test_transient();"))).execute(&pool).await.unwrap();
        for now in [
            start,
            start + chrono::Duration::hours(6) - chrono::Duration::seconds(1),
        ] {
            assert_eq!(drain(&pool, now).await.unwrap(), 0);
            let strikes: i64 = sqlx::query_scalar("select coalesce(sum(attempts),0)::bigint from porchlight.notification_event_delivery where event_id=$1").bind(event).fetch_one(&pool).await.unwrap();
            assert_eq!(strikes, 0);
        }
        let mut now = start + chrono::Duration::hours(6) + chrono::Duration::seconds(60);
        for strike in 1..=5 {
            assert_eq!(drain(&pool, now).await.unwrap(), usize::from(strike == 5));
            let strikes: i32 = sqlx::query_scalar("select coalesce((select attempts from porchlight.notification_event_delivery where event_id=$1),0)").bind(event).fetch_one(&pool).await.unwrap();
            assert_eq!(
                strikes, strike,
                "persistent {code} must spend poison strikes after grace"
            );
            let (attempts, poisoned, retry): (i32, Option<DateTime<Utc>>, DateTime<Utc>) = sqlx::query_as("select attempts,poisoned_at,retry_at from porchlight.notification_event_delivery where event_id=$1").bind(event).fetch_one(&pool).await.unwrap();
            assert_eq!(
                attempts, strike,
                "persistent {code} must spend poison strikes after grace"
            );
            assert_eq!(poisoned.is_some(), strike == 5);
            now = retry;
        }
        let processed: bool = sqlx::query_scalar(
            "select processed_at is not null from porchlight.notification_events where id=$1",
        )
        .bind(event)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(processed);
        sqlx::raw_sql("DROP TRIGGER test_transient ON porchlight.notification_sources; DROP FUNCTION porchlight.pl_test_transient();").execute(&pool).await.unwrap();
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn transient_event_isolated_from_other_events_and_digest_release(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let night = at("2026-11-01T03:00:00Z");
    let morning = at("2026-11-01T12:00:00Z");
    sqlx::query("update porchlight.notification_prefs set quiet_start='22:00',quiet_end='07:00' where member_id=$1").bind(h.super_admin).execute(&pool).await.unwrap();
    emit(&pool, h.id, h.super_admin, None, "completed", night).await;
    assert_eq!(drain(&pool, night).await.unwrap(), 1);
    for offset in [-40, 0, 40] {
        emit(
            &pool,
            h.id,
            h.super_admin,
            None,
            "assigned",
            morning + chrono::Duration::seconds(offset),
        )
        .await;
    }
    let event: Uuid =
        sqlx::query_scalar("select id from porchlight.notification_events where created_at=$1")
            .bind(morning)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE FUNCTION porchlight.pl_test_isolated() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.event_id='{event}'::uuid THEN RAISE EXCEPTION 'injected transient' USING ERRCODE='40P01'; END IF; RETURN NEW; END $$; CREATE TRIGGER test_isolated BEFORE INSERT ON porchlight.notification_sources FOR EACH ROW EXECUTE FUNCTION porchlight.pl_test_isolated();"))).execute(&pool).await.unwrap();
    assert_eq!(
        drain(&pool, morning + chrono::Duration::seconds(40))
            .await
            .unwrap(),
        2,
        "successes on both sides of failure must commit"
    );
    let (sources, released): (i64, i64) = sqlx::query_as("select (select count(*) from porchlight.notification_sources), (select count(*) from porchlight.notifications where category='digest' and released_at is not null)").fetch_one(&pool).await.unwrap();
    assert_eq!(sources, 3);
    assert_eq!(
        released, 1,
        "backed-off event must not hold the digest hostage"
    );
    assert_eq!(
        drain(&pool, morning + chrono::Duration::seconds(42))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from porchlight.notification_sources")
            .fetch_one(&pool)
            .await
            .unwrap(),
        3,
        "successful deliveries must not duplicate on retry"
    );
}

// N1: correlated timeout-class failures (55P03 ~5 s, 57014 ~30 s each in
// production) must cost at most one timeout per recipient pass, otherwise one
// recipient transaction holds its locks and stalls everyone behind it.
#[sqlx::test(migrations = "../db/migrations")]
async fn timeout_class_transient_costs_one_attempt_per_recipient_pass(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let t0 = at("2026-11-01T12:00:00Z");
    let times: Vec<_> = (0..4)
        .map(|i| t0 + chrono::Duration::seconds(40 * i))
        .collect();
    for t in &times {
        emit(&pool, h.id, h.super_admin, None, "assigned", *t).await;
    }
    emit(
        &pool,
        h.id,
        h.user,
        None,
        "assigned",
        t0 + chrono::Duration::seconds(10),
    )
    .await;
    let mut bad = Vec::new();
    for t in &times[1..3] {
        let id: Uuid = sqlx::query_scalar(
            "select id from porchlight.notification_events where created_at=$1 and recipient_member_id=$2",
        )
        .bind(t)
        .bind(h.super_admin)
        .fetch_one(&pool)
        .await
        .unwrap();
        bad.push(id);
    }
    // nextval is non-transactional, so the counter survives the savepoint rollback.
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE SEQUENCE porchlight.pl_test_hits; CREATE FUNCTION porchlight.pl_test_timeouts() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER AS $$ BEGIN IF NEW.event_id IN ('{}'::uuid,'{}'::uuid) THEN PERFORM nextval('porchlight.pl_test_hits'); RAISE EXCEPTION 'injected timeout' USING ERRCODE='57014'; END IF; RETURN NEW; END $$; CREATE TRIGGER test_timeouts BEFORE INSERT ON porchlight.notification_sources FOR EACH ROW EXECUTE FUNCTION porchlight.pl_test_timeouts();", bad[0], bad[1]))).execute(&pool).await.unwrap();
    let hits = |pool: PgPool| async move {
        sqlx::query_scalar::<_, Option<i64>>(
            "select (select last_value from porchlight.pl_test_hits) where (select is_called from porchlight.pl_test_hits)",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap_or(0)
    };
    let delivered = |pool: PgPool, member: Uuid| async move {
        sqlx::query_scalar::<_, i64>(
            "select count(*) from porchlight.notification_events where recipient_member_id=$1 and processed_at is not null",
        )
        .bind(member)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let mut now = t0 + chrono::Duration::seconds(200);
    // Pass 1: e0 succeeds, e1 times out and stops the recipient; the other
    // recipient is still served in the same drain.
    assert_eq!(drain(&pool, now).await.unwrap(), 2);
    assert_eq!(
        hits(pool.clone()).await,
        1,
        "one timeout per recipient pass"
    );
    assert_eq!(delivered(pool.clone(), h.super_admin).await, 1);
    assert_eq!(delivered(pool.clone(), h.user).await, 1);
    // Later passes (1 s apart): the failed events sit out their own backoff and the
    // later healthy event goes through; no pass may hit more than one timeout.
    let mut last_hits = 1;
    let mut done_at = None;
    for pass in 1..=12 {
        now += chrono::Duration::seconds(1);
        drain(&pool, now).await.unwrap();
        let total = hits(pool.clone()).await;
        assert!(
            total - last_hits <= 1,
            "pass {pass} hit {} timeouts",
            total - last_hits
        );
        last_hits = total;
        if delivered(pool.clone(), h.super_admin).await == 2 {
            done_at = Some(pass);
            break;
        }
    }
    let done_at =
        done_at.expect("later healthy event must be delivered while failed events back off");
    assert!(done_at <= 6, "later event delivered after {done_at} passes");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from porchlight.notification_sources")
            .fetch_one(&pool)
            .await
            .unwrap(),
        3,
        "earlier success kept, no duplicates"
    );
    let pending: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.notification_events where processed_at is null",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pending, 2, "only the two failing events stay pending");
}

// N1 gap: a timeout that is the fifth (poisoning) strike must also end the pass.
#[sqlx::test(migrations = "../db/migrations")]
async fn timeout_poison_strike_also_ends_recipient_pass(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let t0 = at("2026-11-01T12:00:00Z");
    for i in 0..4 {
        emit(
            &pool,
            h.id,
            h.super_admin,
            None,
            "assigned",
            t0 + chrono::Duration::seconds(40 * i),
        )
        .await;
    }
    let ids: Vec<Uuid> =
        sqlx::query_scalar("select id from porchlight.notification_events order by created_at")
            .fetch_all(&pool)
            .await
            .unwrap();
    for id in &ids[..3] {
        sqlx::query("insert into porchlight.notification_event_delivery(event_id,attempts,retry_at,last_error) values($1,4,$2,'statement_cancel')")
            .bind(id).bind(t0).execute(&pool).await.unwrap();
    }
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("CREATE SEQUENCE porchlight.pl_test_hits; CREATE FUNCTION porchlight.pl_test_timeouts() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER AS $$ BEGIN IF NEW.event_id IN ('{}'::uuid,'{}'::uuid,'{}'::uuid) THEN PERFORM nextval('porchlight.pl_test_hits'); RAISE EXCEPTION 'injected timeout' USING ERRCODE='57014'; END IF; RETURN NEW; END $$; CREATE TRIGGER test_timeouts BEFORE INSERT ON porchlight.notification_sources FOR EACH ROW EXECUTE FUNCTION porchlight.pl_test_timeouts();", ids[0], ids[1], ids[2]))).execute(&pool).await.unwrap();
    let mut now = t0 + chrono::Duration::hours(7);
    let mut last = 0;
    for pass in 1..=4 {
        drain(&pool, now).await.unwrap();
        let total: i64 = sqlx::query_scalar(
            "select coalesce((select last_value from porchlight.pl_test_hits where is_called),0)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(total - last, i64::from(pass <= 3), "pass {pass}");
        last = total;
        let poisoned: i64 = sqlx::query_scalar(
            "select count(*) from porchlight.notification_event_delivery where poisoned_at is not null",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(poisoned, pass.min(3), "pass {pass}");
        now += chrono::Duration::seconds(1);
    }
    let sources: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_sources")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(sources, 1, "later healthy event delivered");
}
