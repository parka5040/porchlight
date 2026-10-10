mod common;
use common::*;
use futures_util::StreamExt;
use porchlight_api::testing::trust;
use porchlight_api::testing::{
    SseFrame, TestClient, TestDevice, TestOptions, TestServer, TestStream,
};
use porchlight_core::auth::session::Platform;
use porchlight_db::outbox::assign_stream_seqs;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::time::Duration;
use tokio::time::timeout;
use trust::*;
use uuid::Uuid;

// Transport tests control their own outbox input. Real derivation is covered by
// derivation_worker.rs; do not let incidental worker commits race replay cursors.
async fn start(pool: &PgPool) -> TestServer {
    start_with(pool, TestOptions::default()).await
}
async fn start_with(pool: &PgPool, mut opts: TestOptions) -> TestServer {
    opts.background_derivation = false;
    TestServer::start_with(pool, opts).await
}
async fn device(s: &TestServer) -> TestDevice {
    s.enroll("admin", &s.admin_password, Platform::Desktop)
        .await
}
async fn open(s: &TestServer, d: &TestDevice, after: Option<i64>) -> TestStream {
    let mut req = d.request("GET", "/v1/stream", json!({}), None);
    if let Some(seq) = after {
        req.headers_mut()
            .insert("last-event-id", seq.to_string().parse().unwrap());
    }
    let st = TestClient::new(&s.ca_pem, Some(d))
        .stream(s.lan, req)
        .await
        .unwrap();
    assert_eq!(st.status, 200);
    assert_eq!(st.headers["content-type"], "text/event-stream");
    st
}
async fn frame(st: &mut TestStream) -> SseFrame {
    timeout(Duration::from_millis(2500), st.next())
        .await
        .unwrap()
        .expect("stream ended")
}
async fn event(st: &mut TestStream) -> SseFrame {
    loop {
        let f = frame(st).await;
        if f.event.is_some() {
            return f;
        }
    }
}
fn data(f: &SseFrame) -> Value {
    serde_json::from_str(&f.data).unwrap()
}
async fn ready(st: &mut TestStream) -> i64 {
    let f = frame(st).await;
    assert_eq!(f.event.as_deref(), Some("ready"));
    assert!(f.id.is_none());
    data(&f)["cursor"].as_i64().unwrap()
}
async fn item(pool: &PgPool, s: &TestServer, visibility: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO porchlight.items(household_id,owner_id,title,kind,visibility) VALUES ($1,$2,'Secret title','todo',$3) RETURNING id")
        .bind(s.init.household_id).bind(s.init.admin_member_id).bind(visibility).fetch_one(pool).await.unwrap()
}
async fn synthetic(
    pool: &PgPool,
    s: &TestServer,
    kind: &str,
    entity: &str,
    payload: Value,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO porchlight.outbox(household_id,kind,entity_type,entity_id,payload) VALUES ($1,$2,$3,$4,$5)")
        .bind(s.init.household_id).bind(kind).bind(entity).bind(id).bind(payload)
        .execute(pool).await.unwrap();
    id
}

#[sqlx::test(migrations = "../db/migrations")]
async fn full_scope_catalogue_and_member_dismissal(pool: PgPool) {
    let s = start(&pool).await;
    let alice = user(&pool, &s).await;
    let visible = item(&pool, &s, "household").await;
    let hidden = item(&pool, &s, "private").await;
    project(&pool).await;
    let before = porchlight_db::stream::maximum(&pool).await.unwrap();
    let mut expected = Vec::new();
    for (kind, entity) in [
        ("list.changed", "list"),
        ("list_entry.changed", "list_entry"),
        ("comment.changed", "comment"),
        ("attachment.changed", "attachment"),
    ] {
        expected.push(synthetic(&pool, &s, kind, entity, json!({"item_id":visible})).await);
        synthetic(&pool, &s, kind, entity, json!({"item_id":hidden})).await;
        // Unknown standalone lists fail closed against the real S7a ACL.
        synthetic(&pool, &s, kind, entity, json!({"list_id":Uuid::new_v4()})).await;
    }
    for (kind, entity) in [
        ("notification.created", "notification"),
        ("notification.updated", "notification"),
        ("notification.dismissed", "notification"),
        ("settings.changed", "member_settings"),
        ("settings.changed", "notification_prefs"),
        ("settings.changed", "availability_profile"),
        ("ics_source.changed", "ics_source"),
        ("suggestion.changed", "schedule_suggestion"),
        ("suggestion.changed", "suggestion"),
    ] {
        expected.push(synthetic(&pool, &s, kind, entity, json!({"member_id":alice})).await);
        synthetic(
            &pool,
            &s,
            kind,
            entity,
            json!({"member_id":s.init.admin_member_id}),
        )
        .await;
    }
    for payload in [
        json!({}),
        json!({"item_id":"bad"}),
        json!({"list_id":"bad"}),
        json!({"member_id":"bad"}),
    ] {
        synthetic(&pool, &s, "comment.changed", "comment", payload).await;
    }
    synthetic(
        &pool,
        &s,
        "notification.created",
        "notification",
        json!({"item_id":visible}),
    )
    .await;
    synthetic(
        &pool,
        &s,
        "unknown.future.kind",
        "comment",
        json!({"item_id":visible}),
    )
    .await;
    project(&pool).await;
    let a = porchlight_db::stream::read(&s.state.pool, s.init.household_id, alice, before)
        .await
        .unwrap();
    assert_eq!(
        a.events.iter().map(|t| t.entity_id).collect::<Vec<_>>(),
        expected
    );
    assert!(a.scanned > a.events.len() as i64);
    let other = porchlight_db::stream::read(&s.state.pool, Uuid::new_v4(), alice, before)
        .await
        .unwrap();
    assert!(other.events.is_empty());
    // Same member, two devices: dismissal is an ID-only private tickle on both streams.
    let d1 = s.session_device(alice, Platform::Desktop).await;
    let d2 = s.session_device(alice, Platform::Desktop).await;
    let mut st1 = open(&s, &d1, None).await;
    let mut st2 = open(&s, &d2, None).await;
    ready(&mut st1).await;
    ready(&mut st2).await;
    let id = synthetic(
        &pool,
        &s,
        "notification.dismissed",
        "notification",
        json!({"member_id":alice,"title":"must never leak","actor_label":"agent:test"}),
    )
    .await;
    project(&pool).await;
    for st in [&mut st1, &mut st2] {
        let f = event(st).await;
        assert_eq!(f.event.as_deref(), Some("notification.dismissed"));
        assert_eq!(data(&f)["entity_id"], id.to_string());
        assert!(data(&f).get("title").is_none());
        assert!(data(&f).get("actor_label").is_none());
    }
    drop((st1, st2));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn held_stream_slides_due_session_and_gracefully_drains(pool: PgPool) {
    let s = start(&pool).await;
    let d = device(&s).await;
    let mut st = open(&s, &d, None).await;
    ready(&mut st).await;
    sqlx::query("UPDATE porchlight.sessions SET last_used_at=now()-interval '2 hours',expires_at=now()+interval '5 minutes' WHERE device_id=$1")
        .bind(d.device_id).execute(&pool).await.unwrap();
    frame(&mut st).await; // heartbeat triggers best-effort sliding without another HTTP request
    let remaining: f64 = sqlx::query_scalar("SELECT extract(epoch from expires_at-now())::float8 FROM porchlight.sessions WHERE device_id=$1 AND revoked_at IS NULL")
        .bind(d.device_id).fetch_one(&pool).await.unwrap();
    assert!(
        remaining > 3600.,
        "held stream did not slide session: {remaining}"
    );
    let listeners = [s.lan, s.enroll, s.funnel];
    let stopped = tokio::spawn(async move { s.stop().await });
    let f = event(&mut st).await;
    assert_eq!(f.event.as_deref(), Some("drain"));
    assert!(f.id.is_none());
    assert!(
        timeout(Duration::from_secs(2), st.next())
            .await
            .unwrap()
            .is_none(),
        "not clean EOF"
    );
    timeout(Duration::from_secs(6), stopped)
        .await
        .unwrap()
        .unwrap();
    for addr in listeners {
        assert!(
            tokio::net::TcpStream::connect(addr).await.is_err(),
            "listener still accepts after drain"
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn stream_sliding_skips_contended_device_then_retries_on_both_platforms(pool: PgPool) {
    let s = start_with(
        &pool,
        TestOptions {
            db_pool_size: 1,
            ..Default::default()
        },
    )
    .await;
    for platform in [Platform::Desktop, Platform::Android] {
        let d = s.session_device(s.init.admin_member_id, platform).await;
        let mut st = open(&s, &d, None).await;
        ready(&mut st).await;
        sqlx::query("UPDATE porchlight.sessions SET last_used_at=now()-interval '2 hours',expires_at=now()+interval '5 minutes' WHERE device_id=$1")
            .bind(d.device_id).execute(&pool).await.unwrap();
        let mut held = pool.begin().await.unwrap();
        sqlx::query("SELECT id FROM porchlight.devices WHERE id=$1 FOR UPDATE")
            .bind(d.device_id)
            .execute(&mut *held)
            .await
            .unwrap();
        let hb = frame(&mut st).await;
        assert!(
            hb.event.is_none(),
            "contended touch must preserve heartbeat"
        );
        let untouched: bool = sqlx::query_scalar("SELECT last_used_at<now()-interval '1 hour' FROM porchlight.sessions WHERE device_id=$1 AND revoked_at IS NULL")
            .bind(d.device_id).fetch_one(&pool).await.unwrap();
        assert!(
            untouched,
            "best-effort touch waited on the contended device"
        );
        drop(
            timeout(Duration::from_millis(500), s.state.pool.acquire())
                .await
                .unwrap()
                .unwrap(),
        );
        held.rollback().await.unwrap();
        // Heartbeats are produced eagerly every second, so frames generated while the lock was held can still be
        // buffered here (the steps above take seconds under a loaded test binary); a later heartbeat must retry.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut slid = false;
        while !slid && tokio::time::Instant::now() < deadline {
            frame(&mut st).await;
            slid = sqlx::query_scalar("SELECT last_used_at>now()-interval '1 minute' AND expires_at>now()+interval '1 hour' FROM porchlight.sessions WHERE device_id=$1 AND revoked_at IS NULL")
                .bind(d.device_id).fetch_one(&pool).await.unwrap();
        }
        assert!(
            slid,
            "next heartbeat must retry touch after contention clears"
        );
        drop(st);
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn retained_window_edges_and_pruned_cursor_resnapshot(pool: PgPool) {
    use chrono::{Duration as Span, TimeZone, Utc};
    let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
    let opts = TestOptions {
        item_clock: std::sync::Arc::new(move || now),
        ..Default::default()
    };
    let s = start_with(&pool, opts).await;
    let d = device(&s).await;
    let old: i64 = sqlx::query_scalar("INSERT INTO porchlight.outbox(household_id,kind,entity_type,entity_id,created_at,stream_seq) VALUES ($1,'member.changed','member',$2,$3,nextval('porchlight.outbox_stream_seq')) RETURNING stream_seq")
        .bind(s.init.household_id).bind(s.init.admin_member_id).bind(now-Span::days(7)-Span::microseconds(1)).fetch_one(&pool).await.unwrap();
    let edge: i64 = sqlx::query_scalar("INSERT INTO porchlight.outbox(household_id,kind,entity_type,entity_id,created_at,stream_seq) VALUES ($1,'member.changed','member',$2,$3,nextval('porchlight.outbox_stream_seq')) RETURNING stream_seq")
        .bind(s.init.household_id).bind(s.init.admin_member_id).bind(now-Span::days(7)).fetch_one(&pool).await.unwrap();
    let w = porchlight_db::stream::window_at(&pool, now).await.unwrap();
    assert!(
        w.expired(old),
        "cursor row itself is older than the retained window"
    );
    assert!(!w.expired(edge), "exact seven-day cutoff is retained");
    let quiet = porchlight_db::stream::window_at(&pool, now + Span::days(30))
        .await
        .unwrap();
    assert!(
        !quiet.expired(quiet.maximum),
        "snapshot cursor must not enter a resnapshot loop on a quiet household"
    );
    let mut st = open(&s, &d, Some(old)).await;
    let f = frame(&mut st).await;
    assert_eq!(f.event.as_deref(), Some("resnapshot"));
    assert_eq!(data(&f)["reason"], "cursor_expired");
    assert!(f.id.is_none());
    assert!(
        timeout(Duration::from_secs(1), st.next())
            .await
            .unwrap()
            .is_none()
    );
    let mut good = open(&s, &d, Some(edge)).await;
    assert_eq!(ready(&mut good).await, edge);
    drop(good);
    sqlx::query("DELETE FROM porchlight.outbox WHERE stream_seq<=$1")
        .bind(edge)
        .execute(&pool)
        .await
        .unwrap();
    // A cursor above an empty/older retained maximum is unknown, not silently accepted.
    let mut pruned = open(&s, &d, Some(edge)).await;
    assert_eq!(
        frame(&mut pruned).await.event.as_deref(),
        Some("resnapshot")
    );
    drop((st, pruned));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn replay_rechecks_queued_acl_and_lists_evict_former_readers(pool: PgPool) {
    let s = start(&pool).await;
    let alice = user(&pool, &s).await;
    let id = item(&pool, &s, "household").await;
    project(&pool).await;
    let before = porchlight_db::stream::maximum(&pool).await.unwrap();
    let comment = synthetic(
        &pool,
        &s,
        "comment.changed",
        "comment",
        json!({"item_id":id}),
    )
    .await;
    let list = Uuid::new_v4();
    let evict = synthetic(
        &pool,
        &s,
        "list.changed",
        "list",
        json!({"list_id":list,"eviction":true,"recipients":[alice]}),
    )
    .await;
    project(&pool).await;
    let batch = porchlight_db::stream::read(&s.state.pool, s.init.household_id, alice, before)
        .await
        .unwrap();
    let queued = batch
        .events
        .iter()
        .find(|t| t.entity_id == comment)
        .unwrap();
    sqlx::query("UPDATE porchlight.items SET visibility='private' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        porchlight_db::stream::delivery(&s.state.pool, s.init.household_id, alice, queued.seq)
            .await
            .unwrap()
            .is_none()
    );
    let eviction = batch
        .events
        .iter()
        .find(|t| t.entity_id == evict)
        .expect("former list reader must evict");
    assert!(eviction.evicted);
    // Replay uses current ACL, so old comments/changes disappear and access events survive.
    project(&pool).await;
    let replay = porchlight_db::stream::read(&s.state.pool, s.init.household_id, alice, before)
        .await
        .unwrap();
    assert!(!replay.events.iter().any(|t| t.entity_id == comment));
    assert!(
        replay
            .events
            .iter()
            .any(|t| t.entity_id == id && t.kind == "item.access_changed" && t.evicted)
    );
    // A newly entitled reader receives an ordinary access refresh, not a stale eviction.
    sqlx::query("UPDATE porchlight.items SET visibility='household' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let again = porchlight_db::stream::read(&s.state.pool, s.init.household_id, alice, before)
        .await
        .unwrap();
    assert!(
        again
            .events
            .iter()
            .filter(|t| t.entity_id == id && t.kind == "item.access_changed")
            .all(|t| !t.evicted)
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn generated_sse_matrix_is_exercised_against_authoritative_scopes(pool: PgPool) {
    use porchlight_api::permissions::{Relationship, STREAM_ENDPOINTS, matrix_rows};
    let s = start(&pool).await;
    let owner = user_named(&pool, &s, "owner", "Owner").await.1;
    let assignee = user_named(&pool, &s, "assignee", "Assignee").await.1;
    let viewer = user_named(&pool, &s, "viewer", "Viewer").await.1;
    let subject = user_named(&pool, &s, "subject", "Subject").await.1;
    let other = user_named(&pool, &s, "other", "Other").await.1;
    let members = [
        owner,
        assignee,
        viewer,
        subject,
        other,
        s.init.admin_member_id,
    ];
    let mut resources = Vec::new();
    for visibility in ["household", "private", "selected"] {
        let id: Uuid = sqlx::query_scalar("INSERT INTO porchlight.items(household_id,owner_id,subject_member_id,title,kind,visibility) VALUES ($1,$2,$3,'Never stream this','todo',$4) RETURNING id")
            .bind(s.init.household_id).bind(owner).bind(subject).bind(visibility).fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO porchlight.item_assignees(household_id,item_id,member_id) VALUES ($1,$2,$3)")
            .bind(s.init.household_id).bind(id).bind(assignee).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO porchlight.item_viewers(household_id,item_id,member_id) VALUES ($1,$2,$3)",
        )
        .bind(s.init.household_id)
        .bind(id)
        .bind(viewer)
        .execute(&pool)
        .await
        .unwrap();
        let occurrence: Uuid = sqlx::query_scalar("INSERT INTO porchlight.item_occurrences(household_id,item_id,seq) VALUES ($1,$2,0) RETURNING id")
            .bind(s.init.household_id).bind(id).fetch_one(&pool).await.unwrap();
        let list: Uuid = sqlx::query_scalar("INSERT INTO porchlight.lists(household_id,owner_id,name,visibility) VALUES ($1,$2,'Hidden list content',$3) RETURNING id")
            .bind(s.init.household_id).bind(owner).bind(visibility).fetch_one(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO porchlight.list_viewers(household_id,list_id,member_id) VALUES ($1,$2,$3)",
        )
        .bind(s.init.household_id)
        .bind(list)
        .bind(viewer)
        .execute(&pool)
        .await
        .unwrap();
        resources.push((id, occurrence, list));
    }
    // Exercise the installed S7a function, never substitute or hand-apply a migration.
    let hidden_owner = user_named(&pool, &s, "hidden", "Hidden").await.1;
    let hidden_item: Uuid = sqlx::query_scalar("INSERT INTO porchlight.items(household_id,owner_id,title,kind,visibility) VALUES ($1,$2,'Former-reader resource','todo','private') RETURNING id")
        .bind(s.init.household_id).bind(hidden_owner).fetch_one(&pool).await.unwrap();
    let hidden_list: Uuid = sqlx::query_scalar("INSERT INTO porchlight.lists(household_id,owner_id,name,visibility) VALUES ($1,$2,'Former-reader list','private') RETURNING id")
        .bind(s.init.household_id).bind(hidden_owner).fetch_one(&pool).await.unwrap();
    project(&pool).await;
    let mut checked = 0;
    for row in matrix_rows(STREAM_ENDPOINTS) {
        if matches!(row.relationship, Relationship::Agent) {
            assert_eq!(row.status, 404);
            continue;
        }
        let member = match row.relationship {
            Relationship::Owner => members[0],
            Relationship::Assignee => members[1],
            Relationship::Viewer => members[2],
            Relationship::Subject => members[3],
            Relationship::Other => members[4],
            Relationship::SuperAdmin => members[5],
            Relationship::Agent => unreachable!(),
        };
        if row.endpoint.case.is_empty() {
            continue;
        }
        let (mut id, occurrence, mut list) = resources[match row.visibility.as_str() {
            "household" => 0,
            "private" => 1,
            _ => 2,
        }];
        let kind = row.endpoint.case.split_whitespace().nth(1).unwrap();
        let standalone = row.endpoint.case.contains("standalone");
        let former = row.endpoint.case.contains("former reader");
        if former {
            id = hidden_item;
            list = hidden_list;
        }
        let (entity_type, entity_id) = match kind {
            "item.changed" | "item.access_changed" | "derived" => ("item", id),
            "occurrence.changed" => ("occurrence", occurrence),
            "list.changed" => ("list", list),
            "list_entry.changed" => ("list_entry", Uuid::new_v4()),
            "comment.changed" => ("comment", Uuid::new_v4()),
            "attachment.changed" => ("attachment", Uuid::new_v4()),
            "settings.changed" => ("member_settings", Uuid::new_v4()),
            "ics_source.changed" => ("ics_source", Uuid::new_v4()),
            "suggestion.changed" => ("suggestion", Uuid::new_v4()),
            "member.changed" => ("member", owner),
            _ => ("notification", Uuid::new_v4()),
        };
        let payload = if former {
            if kind == "list.changed" {
                json!({"list_id":list,"recipients":[member],"eviction":true})
            } else {
                json!({"item_id":id,"recipients":[member]})
            }
        } else if standalone {
            json!({"list_id":list})
        } else if kind.starts_with("notification.")
            || matches!(
                kind,
                "settings.changed" | "ics_source.changed" | "suggestion.changed"
            )
        {
            json!({"member_id": if row.is_self { member } else { hidden_owner }})
        } else {
            json!({"item_id":id})
        };
        let seq: i64 = sqlx::query_scalar("INSERT INTO porchlight.outbox(household_id,kind,entity_type,entity_id,payload,stream_seq) VALUES ($1,$2,$3,$4,$5,nextval('porchlight.outbox_stream_seq')) RETURNING stream_seq")
            .bind(s.init.household_id).bind(kind).bind(entity_type).bind(entity_id).bind(payload).fetch_one(&pool).await.unwrap();
        let actual =
            porchlight_db::stream::delivery(&s.state.pool, s.init.household_id, member, seq)
                .await
                .unwrap();
        assert_eq!(
            actual.is_some(),
            row.status == 200,
            "{} {:?} {:?}",
            row.endpoint.case,
            row.relationship,
            row.visibility
        );
        if let Some(t) = actual {
            assert_eq!(t.evicted, former, "{}", row.endpoint.case);
        }
        checked += 1;
    }
    assert_eq!(checked, 340);
    let r = agent(
        &s,
        Some(s.bootstrap_token()),
        "GET",
        "/v1/stream",
        json!({}),
        "matrix",
    )
    .await;
    assert_eq!(r.status, 404);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn former_readers_exclude_deleted_and_dependent_members(pool: PgPool) {
    let s = start(&pool).await;
    let alice = user(&pool, &s).await;
    let dependent: Uuid = sqlx::query_scalar("INSERT INTO porchlight.household_members(household_id,role,display_name,color,tzid) VALUES ($1,'dependent','Child','#AABBCC','UTC') RETURNING id")
        .bind(s.init.household_id).fetch_one(&pool).await.unwrap();
    let id = item(&pool, &s, "selected").await;
    for member in [alice, dependent] {
        sqlx::query("INSERT INTO porchlight.item_assignees(household_id,item_id,member_id) VALUES ($1,$2,$3)")
            .bind(s.init.household_id).bind(id).bind(member).execute(&pool).await.unwrap();
    }
    sqlx::query("UPDATE porchlight.household_members SET deleted_at=now() WHERE id=$1")
        .bind(alice)
        .execute(&pool)
        .await
        .unwrap();
    let readers: Vec<Uuid> =
        sqlx::query_scalar("SELECT porchlight.pl_item_readers($1,$2,$3,'selected')")
            .bind(s.init.household_id)
            .bind(id)
            .bind(s.init.admin_member_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(readers, vec![s.init.admin_member_id]);
    let batch = porchlight_db::stream::read(&s.state.pool, s.init.household_id, alice, 0)
        .await
        .unwrap();
    assert!(batch.events.is_empty());
    s.stop().await;
}

async fn project(pool: &PgPool) {
    while assign_stream_seqs(pool, 500).await.unwrap().is_some() {}
}
async fn seq(pool: &PgPool, id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT stream_seq FROM porchlight.outbox WHERE kind='item.changed' AND entity_id=$1 ORDER BY stream_seq LIMIT 1").bind(id).fetch_one(pool).await.unwrap()
}
async fn ended(st: &mut TestStream, secs: u64) {
    timeout(Duration::from_secs(secs), async {
        while let Some(f) = st.next().await {
            assert!(f.event.is_none(), "unexpected event: {f:?}");
        }
    })
    .await
    .unwrap();
}
async fn no_events(st: &mut TestStream) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1200);
    loop {
        match tokio::time::timeout_at(deadline, st.next()).await {
            Err(_) => break,
            Ok(Some(f)) => assert!(f.event.is_none(), "unexpected {f:?}"),
            Ok(None) => panic!("unexpected EOF"),
        }
    }
}

async fn wait_listeners(pool: &PgPool, name: Option<&str>, count: i64) {
    timeout(Duration::from_secs(3), async {
        loop {
            let n: i64 = sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND state='idle' AND query LIKE 'LISTEN%' AND ($1::text IS NULL OR application_name=$1)")
                .bind(name).fetch_one(pool).await.unwrap();
            if n >= count { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.expect("listeners did not start");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn listeners_leave_two_request_connections_available(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            db_pool_size: 2,
            ..Default::default()
        },
    )
    .await;
    wait_listeners(&pool, None, 3).await;
    timeout(Duration::from_secs(3), async {
        // Both request slots must remain available even while all three listeners hold theirs.
        let a = s.state.pool.acquire().await.unwrap();
        let b = s.state.pool.acquire().await.unwrap();
        drop((a, b));
        let d = device(&s).await;
        let login_device = s
            .issue_device(s.init.admin_member_id, Platform::Desktop)
            .await;
        let client = TestClient::new(&s.ca_pem, Some(&login_device));
        let (mut a, mut b, login) = tokio::join!(
            open(&s, &d, None),
            open(&s, &d, None),
            client.request(
                s.lan,
                login_device.request(
                    "POST",
                    "/v1/auth/login",
                    json!({"password":s.admin_password}),
                    None
                )
            )
        );
        ready(&mut a).await;
        ready(&mut b).await;
        assert_eq!(login.unwrap().status, 200);
        drop((a, b));
    })
    .await
    .expect("two-slot request pool stalled");
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn hub_reconnect_repairs_events_without_heartbeat(pool: PgPool) {
    let mut opts = TestOptions::default();
    opts.settings.heartbeat = Duration::from_secs(30);
    let s = start_with(&pool, opts).await;
    let d = device(&s).await;
    let mut st = open(&s, &d, None).await;
    ready(&mut st).await;
    wait_listeners(&pool, Some("porchlight-stream-listener"), 1).await;
    // Repeat quiet reconnects: no notification is needed to recover successfully.
    for _ in 0..3 {
        let pid: i32 = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND application_name='porchlight-stream-listener' AND state='idle' AND query LIKE 'LISTEN%'")
            .fetch_one(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        assert!(
            sqlx::query_scalar::<_, bool>("SELECT pg_terminate_backend($1)")
                .bind(pid)
                .fetch_one(&mut *tx)
                .await
                .unwrap()
        );
        // Notification is lost while disconnected; reconnect catch-up must recover the event.
        let id: Uuid = sqlx::query_scalar("INSERT INTO porchlight.items(household_id,owner_id,title,kind,visibility) VALUES ($1,$2,'Reconnect','todo','household') RETURNING id")
            .bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        project(&pool).await;
        timeout(Duration::from_secs(10), async {
            loop {
                let new_pid: Option<i32> = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND application_name='porchlight-stream-listener' AND state='idle' AND query LIKE 'LISTEN%' AND pid<>$1").bind(pid).fetch_optional(&pool).await.unwrap();
                if new_pid.is_some() { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("hub did not reconnect with a new LISTEN pid");
        assert_eq!(data(&event(&mut st).await)["entity_id"], id.to_string());
        wait_listeners(&pool, Some("porchlight-stream-listener"), 1).await;
    }
    drop(st);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn ready_then_live_event(pool: PgPool) {
    let mut opts = TestOptions::default();
    opts.settings.heartbeat = Duration::from_secs(30);
    let s = start_with(&pool, opts).await;
    let d = device(&s).await;
    let mut st = open(&s, &d, None).await;
    let n = ready(&mut st).await;
    let id = item(&pool, &s, "household").await;
    project(&pool).await;
    let f = event(&mut st).await;
    assert_eq!(f.event.as_deref(), Some("item.changed"));
    assert!(f.id.as_ref().unwrap().parse::<i64>().unwrap() > n);
    let j = data(&f);
    assert_eq!(j["entity_id"], id.to_string());
    assert_eq!(j["kind"], "item.changed");
    assert_eq!(j["entity_type"], "item");
    assert_eq!(j.as_object().unwrap().len(), 4);
    drop(st);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn replay_after_last_event_id(pool: PgPool) {
    let s = start(&pool).await;
    let d = device(&s).await;
    let mut ids = vec![];
    for _ in 0..3 {
        ids.push(item(&pool, &s, "household").await);
    }
    project(&pool).await;
    let n = seq(&pool, ids[0]).await;
    let mut st = open(&s, &d, Some(n)).await;
    assert_eq!(ready(&mut st).await, n);
    for id in &ids[1..] {
        let f = event(&mut st).await;
        assert_eq!(data(&f)["entity_id"], id.to_string());
        assert_eq!(f.id.unwrap(), seq(&pool, *id).await.to_string());
    }
    no_events(&mut st).await;
    drop(st);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn private_items_filtered(pool: PgPool) {
    let s = start(&pool).await;
    user(&pool, &s).await;
    let a = device(&s).await;
    let d = s
        .enroll("alice", &s.admin_password, Platform::Desktop)
        .await;
    let mut admin_st = open(&s, &a, None).await;
    ready(&mut admin_st).await;
    let mut user_st = open(&s, &d, None).await;
    ready(&mut user_st).await;
    let id = item(&pool, &s, "private").await;
    project(&pool).await;
    assert_eq!(
        data(&event(&mut admin_st).await)["entity_id"],
        id.to_string()
    );
    no_events(&mut user_st).await;
    drop((admin_st, user_st));
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn heartbeat_comment(pool: PgPool) {
    let s = start(&pool).await;
    let d = device(&s).await;
    // S5 emits member.changed for the seeded member. Publish it before the
    // ready cursor so this idle-heartbeat test cannot race the projector.
    project(&pool).await;
    let mut st = open(&s, &d, None).await;
    ready(&mut st).await;
    let f = frame(&mut st).await;
    assert_eq!(f.comment.as_deref(), Some("hb"));
    assert!(f.event.is_none());
    assert!(f.id.is_none());
    assert!(f.data.is_empty());
    drop(st);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn unknown_cursor_resnapshot(pool: PgPool) {
    let s = start(&pool).await;
    let d = device(&s).await;
    let mut st = open(&s, &d, Some(999999999)).await;
    let f = frame(&mut st).await;
    assert_eq!(f.event.as_deref(), Some("resnapshot"));
    assert_eq!(data(&f), json!({"reason":"cursor_unknown"}));
    assert!(f.id.is_none());
    ended(&mut st, 1).await;
    drop(st);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn revocation_closes_stream(pool: PgPool) {
    let mut opts = TestOptions::default();
    opts.settings.heartbeat = Duration::from_secs(30);
    let s = start_with(&pool, opts).await;
    let d = device(&s).await;
    let mut st = open(&s, &d, None).await;
    ready(&mut st).await;
    assert_eq!(
        admin(
            &s,
            "POST",
            &format!("/v1/admin/devices/{}/revoke", d.device_id),
            json!({})
        )
        .await
        .status,
        200
    );
    ended(&mut st, 2).await;
    drop(st);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn revoked_between_auth_and_subscribe(pool: PgPool) {
    let opts = TestOptions {
        stream_pre_subscribe_delay: Duration::from_millis(700),
        ..Default::default()
    };
    let s = start_with(&pool, opts).await;
    let d = device(&s).await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let opening = client.stream(s.lan, d.request("GET", "/v1/stream", json!({}), None));
    let (st, _) = tokio::join!(opening, async {
        timeout(
            Duration::from_secs(2),
            s.state.stream_pre_subscribe_started.notified(),
        )
        .await
        .unwrap();
        assert_eq!(
            admin(
                &s,
                "POST",
                &format!("/v1/admin/devices/{}/revoke", d.device_id),
                json!({})
            )
            .await
            .status,
            200
        );
    });
    let mut st = st.unwrap();
    assert_eq!(st.status, 200);
    ended(&mut st, 1).await;
    drop(st);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn opening_database_failure_is_problem_not_empty_sse(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            stream_pre_subscribe_delay: Duration::from_millis(700),
            ..Default::default()
        },
    )
    .await;
    let d = device(&s).await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let (response, ()) = tokio::join!(
        client.request(s.lan, d.request("GET", "/v1/stream", json!({}), None)),
        async {
            timeout(
                Duration::from_secs(2),
                s.state.stream_pre_subscribe_started.notified(),
            )
            .await
            .unwrap();
            s.state.pool.close().await;
        }
    );
    let response = response.unwrap();
    problem(&response, 500, "internal");
    assert_eq!(response.headers["content-type"], "application/problem+json");
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn mixed_case_bearer_uses_extracted_session_identity(pool: PgPool) {
    let s = start(&pool).await;
    let d = device(&s).await;
    let mut req = d.request("GET", "/v1/stream", json!({}), None);
    req.headers_mut().insert(
        "authorization",
        format!("bEaReR {}", d.session.as_ref().unwrap())
            .parse()
            .unwrap(),
    );
    let mut st = TestClient::new(&s.ca_pem, Some(&d))
        .stream(s.lan, req)
        .await
        .unwrap();
    assert_eq!(st.status, 200);
    ready(&mut st).await;
    drop(st);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn former_readers_get_eviction(pool: PgPool) {
    let s = start(&pool).await;
    user(&pool, &s).await;
    let d = s
        .enroll("alice", &s.admin_password, Platform::Desktop)
        .await;
    let id = item(&pool, &s, "household").await;
    let deleted = item(&pool, &s, "household").await;
    project(&pool).await;
    let mut st = open(&s, &d, None).await;
    ready(&mut st).await;
    sqlx::query("UPDATE porchlight.items SET visibility='private' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    project(&pool).await;
    let f = event(&mut st).await;
    assert_eq!(f.event.as_deref(), Some("item.access_changed"));
    assert_eq!(data(&f)["entity_id"], id.to_string());
    sqlx::query("UPDATE porchlight.items SET title='Still private' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    project(&pool).await;
    no_events(&mut st).await;
    sqlx::query("UPDATE porchlight.items SET deleted_at=now() WHERE id=$1")
        .bind(deleted)
        .execute(&pool)
        .await
        .unwrap();
    project(&pool).await;
    let f = event(&mut st).await;
    assert_eq!(f.event.as_deref(), Some("item.access_changed"));
    assert_eq!(data(&f)["entity_id"], deleted.to_string());
    no_events(&mut st).await;
    drop(st);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn unknown_kinds_not_delivered(pool: PgPool) {
    let s = start(&pool).await;
    let d = device(&s).await;
    let id = item(&pool, &s, "household").await;
    project(&pool).await;
    let mut st = open(&s, &d, None).await;
    ready(&mut st).await;
    sqlx::query("INSERT INTO porchlight.outbox(household_id,kind,entity_type,entity_id) VALUES ($1,'unknown.future.kind','item',$2)").bind(s.init.household_id).bind(id).execute(&pool).await.unwrap();
    project(&pool).await;
    no_events(&mut st).await;
    drop(st);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn logout_closes_stream_at_next_heartbeat(pool: PgPool) {
    let s = start(&pool).await;
    let d = device(&s).await;
    let mut st = open(&s, &d, None).await;
    ready(&mut st).await;
    assert_eq!(
        TestClient::new(&s.ca_pem, Some(&d))
            .request(s.lan, d.request("POST", "/v1/auth/logout", json!({}), None))
            .await
            .unwrap()
            .status,
        204
    );
    ended(&mut st, 2).await;
    drop(st);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn requires_session(pool: PgPool) {
    let s = start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    problem(
        &TestClient::new(&s.ca_pem, Some(&d))
            .request(s.lan, d.request("GET", "/v1/stream", json!({}), None))
            .await
            .unwrap(),
        401,
        "unauthenticated",
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn after_query_header_precedence_and_global_default(pool: PgPool) {
    let s = start(&pool).await;
    let d = device(&s).await;
    let a = item(&pool, &s, "household").await;
    let b = item(&pool, &s, "household").await;
    project(&pool).await;
    let n = seq(&pool, a).await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let mut st = client
        .stream(
            s.lan,
            d.request("GET", &format!("/v1/stream?after={n}"), json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(ready(&mut st).await, n);
    assert_eq!(data(&event(&mut st).await)["entity_id"], b.to_string());
    drop(st);
    let mut req = d.request("GET", "/v1/stream?after=999999999", json!({}), None);
    req.headers_mut()
        .insert("last-event-id", n.to_string().parse().unwrap());
    let mut st = client.stream(s.lan, req).await.unwrap();
    assert_eq!(ready(&mut st).await, n);
    assert_eq!(data(&event(&mut st).await)["entity_id"], b.to_string());
    drop(st);
    let before_other = porchlight_db::stream::maximum(&pool).await.unwrap();
    let other = porchlight_db::testing::seed_household(&pool).await.unwrap();
    porchlight_db::testing::insert_appointment(&pool, &other)
        .await
        .unwrap();
    project(&pool).await;
    let global = porchlight_db::stream::maximum(&pool).await.unwrap();
    assert!(global > before_other);
    // Exercise replay across the other household's rows, not just live/default cursors.
    let mut st = client
        .stream(
            s.lan,
            d.request(
                "GET",
                &format!("/v1/stream?after={before_other}"),
                json!({}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(ready(&mut st).await, before_other);
    no_events(&mut st).await;
    drop(st);
    let mut st = open(&s, &d, None).await;
    assert_eq!(ready(&mut st).await, global);
    no_events(&mut st).await;
    drop(st);
    // A known global cursor from another household is not a resnapshot request.
    let mut st = open(&s, &d, Some(global)).await;
    assert_eq!(ready(&mut st).await, global);
    drop(st);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn revocation_lag_revalidates_immediately(pool: PgPool) {
    let mut opts = TestOptions {
        watch_revocations: false,
        ..Default::default()
    };
    opts.settings.heartbeat = Duration::from_secs(30);
    let s = start_with(&pool, opts).await;
    let d = device(&s).await;
    let mut st = open(&s, &d, None).await;
    ready(&mut st).await;
    sqlx::query("UPDATE porchlight.devices SET revoked_at=now() WHERE id=$1")
        .bind(d.device_id)
        .execute(&pool)
        .await
        .unwrap();
    // No own-device hint and no watcher: only Lagged revalidation can close before the heartbeat.
    let unrelated = Uuid::now_v7();
    for _ in 0..10000 {
        s.state.revocations.send(unrelated).unwrap();
    }
    ended(&mut st, 2).await;
    drop(st);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn replay_advances_past_fully_filtered_batches(pool: PgPool) {
    let s = start(&pool).await;
    let d = device(&s).await;
    // S4b seeds a private default settings resource during member creation.
    project(&pool).await;
    let initial = porchlight_db::stream::maximum(&pool).await.unwrap();
    sqlx::query("INSERT INTO porchlight.outbox(household_id,kind,entity_type,entity_id) SELECT $1,'unknown.future.kind','item',$2 FROM generate_series(1,1001)").bind(s.init.household_id).bind(Uuid::now_v7()).execute(&pool).await.unwrap();
    let id = item(&pool, &s, "household").await;
    project(&pool).await;
    let mut st = open(&s, &d, Some(initial)).await;
    ready(&mut st).await;
    assert_eq!(data(&event(&mut st).await)["entity_id"], id.to_string());
    no_events(&mut st).await;
    drop(st);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn expired_session_closes_at_heartbeat(pool: PgPool) {
    let s = start(&pool).await;
    let d = device(&s).await;
    let mut st = open(&s, &d, None).await;
    ready(&mut st).await;
    sqlx::query(
        "UPDATE porchlight.sessions SET expires_at=now()-interval '1 second' WHERE device_id=$1",
    )
    .bind(d.device_id)
    .execute(&pool)
    .await
    .unwrap();
    ended(&mut st, 2).await;
    drop(st);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn heartbeat_repairs_missed_notification(pool: PgPool) {
    let s = start(&pool).await;
    let d = device(&s).await;
    let mut st = open(&s, &d, None).await;
    // The initial cursor includes the S4b default settings event.
    assert_eq!(
        ready(&mut st).await,
        porchlight_db::stream::maximum(&pool).await.unwrap()
    );
    // Test-only pre-projection: no NOTIFY at all. The heartbeat must find this row independently.
    sqlx::query("INSERT INTO porchlight.outbox(household_id,kind,entity_type,entity_id,stream_seq) VALUES ($1,'member.changed','member',$2,nextval('porchlight.outbox_stream_seq'))").bind(s.init.household_id).bind(s.init.admin_member_id).execute(&pool).await.unwrap();
    let f = event(&mut st).await;
    assert_eq!(f.event.as_deref(), Some("member.changed"));
    assert_eq!(data(&f)["entity_id"], s.init.admin_member_id.to_string());
    drop(st);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn multiple_streams_share_one_listener_and_hints_are_untrusted(pool: PgPool) {
    let mut opts = TestOptions::default();
    opts.settings.heartbeat = Duration::from_secs(30);
    let s = start_with(&pool, opts).await;
    let d = device(&s).await;
    let mut a = open(&s, &d, None).await;
    let mut b = open(&s, &d, None).await;
    ready(&mut a).await;
    ready(&mut b).await;
    wait_listeners(&pool, Some("porchlight-stream-listener"), 1).await;
    let listeners:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND query LIKE 'LISTEN%porchlight_stream%' AND state='idle'").fetch_one(&pool).await.unwrap();
    assert_eq!(listeners, 1);
    sqlx::query("SELECT pg_notify('porchlight_stream','999999999 untrusted hint')")
        .execute(&pool)
        .await
        .unwrap();
    let id = item(&pool, &s, "household").await;
    project(&pool).await;
    for st in [&mut a, &mut b] {
        assert_eq!(data(&event(st).await)["entity_id"], id.to_string());
    }
    drop((a, b));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn private_viewer_removal_is_not_delivered_but_selected_is(pool: PgPool) {
    let s = start(&pool).await;
    let member = user(&pool, &s).await;
    let d = s.session_device(member, Platform::Desktop).await;
    let private = item(&pool, &s, "private").await;
    let selected = item(&pool, &s, "selected").await;
    for id in [private, selected] {
        sqlx::query(
            "INSERT INTO porchlight.item_viewers(household_id,item_id,member_id) VALUES ($1,$2,$3)",
        )
        .bind(s.init.household_id)
        .bind(id)
        .bind(member)
        .execute(&pool)
        .await
        .unwrap();
    }
    project(&pool).await;
    let mut st = open(&s, &d, None).await;
    ready(&mut st).await;
    sqlx::query("DELETE FROM porchlight.item_viewers WHERE item_id=$1")
        .bind(private)
        .execute(&pool)
        .await
        .unwrap();
    project(&pool).await;
    no_events(&mut st).await;
    sqlx::query("DELETE FROM porchlight.item_viewers WHERE item_id=$1")
        .bind(selected)
        .execute(&pool)
        .await
        .unwrap();
    project(&pool).await;
    let f = event(&mut st).await;
    assert_eq!(f.event.as_deref(), Some("item.access_changed"));
    assert_eq!(data(&f)["entity_id"], selected.to_string());
    drop(st);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn derived_is_a_supported_live_tickle(pool: PgPool) {
    let s = TestServer::start_with(&pool, TestOptions::default()).await;
    let d = device(&s).await;

    let mut st = open(&s, &d, None).await;
    ready(&mut st).await;
    // Real background LISTEN/poll worker, not a synthetic outbox helper.
    let id = item(&pool, &s, "household").await;
    timeout(Duration::from_secs(3), async {
        loop {
            let pending: bool = sqlx::query_scalar(
                "select applied_gen<requested_gen from porchlight.item_derivation where item_id=$1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
            if !pending {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    project(&pool).await;
    let f = loop {
        let f = event(&mut st).await;
        if f.event.as_deref() == Some("derived") {
            break f;
        }
    };
    assert_eq!(data(&f)["entity_id"], id.to_string());
    assert_eq!(data(&f).as_object().unwrap().len(), 4); // no title, notes or job error
    no_events(&mut st).await;
    drop(st);
    s.stop().await;
}

#[test]
fn queued_flush_is_one_acl_statement_and_indexable() {
    let route = include_str!("../src/routes/stream.rs");
    assert!(
        !route.contains("r=stream::delivery("),
        "one-row recheck per tickle"
    );
    assert_eq!(route.matches("r=stream::delivery_batch(").count(), 1);
    assert!(!route.contains("self.pending.pop_front()"));
    assert!(route.contains("return Some(tickle_chunk(batch.events))"));
    assert!(
        route.contains("r=stream::delivery_batch("),
        "one batched recheck per body flush"
    );
}
#[test]
fn payload_uuid_arms_are_indexable() {
    let db = include_str!("../../db/src/stream.rs");
    let read_range = db.split("async fn read_range(").nth(1).unwrap();
    assert_eq!(
        read_range.matches("fetch_one(pool)").count(),
        1,
        "one statement per batch"
    );
    assert!(
        db.contains("stream_seq = ANY($6)"),
        "recheck only queued seqs in one statement"
    );
    assert!(
        !db.contains("i.id::text"),
        "UUID payload arm cannot use items_pkey"
    );
}
#[test]
fn sse_matrix_preserves_resource_dimensions() {
    use porchlight_api::permissions::{STREAM_ENDPOINTS, matrix_rows};
    let rows = matrix_rows(STREAM_ENDPOINTS);
    assert_eq!(rows.len(), 464);
    for case in [
        "SSE notification.created",
        "SSE notification.updated",
        "SSE notification.dismissed",
    ] {
        let notification_rows: Vec<_> = rows.iter().filter(|r| r.endpoint.case == case).collect();
        assert_eq!(notification_rows.len(), 54);
        assert!(
            notification_rows
                .iter()
                .any(|r| r.is_self && !r.context.is_owner)
        );
        assert!(
            notification_rows
                .iter()
                .any(|r| !r.is_self && r.context.is_owner)
        );
    }
    for case in ["SSE item.changed", "SSE list.changed (standalone)"] {
        assert_eq!(rows.iter().filter(|r| r.endpoint.case == case).count(), 21);
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn queued_batch_recheck_filters_all_stale_decisions(pool: PgPool) {
    let s = start(&pool).await;
    let reader = user(&pool, &s).await;
    let id = item(&pool, &s, "household").await;
    project(&pool).await;
    let before = porchlight_db::stream::maximum(&pool).await.unwrap();
    for _ in 0..500 {
        synthetic(
            &pool,
            &s,
            "comment.changed",
            "comment",
            json!({"item_id": id}),
        )
        .await;
    }
    project(&pool).await;
    let queued = porchlight_db::stream::read(&s.state.pool, s.init.household_id, reader, before)
        .await
        .unwrap();
    assert_eq!(queued.events.len(), 500);
    let seqs: Vec<_> = queued.events.iter().map(|t| t.seq).collect();
    let flush =
        porchlight_db::stream::deliveries(&s.state.pool, s.init.household_id, reader, &seqs)
            .await
            .unwrap();
    assert_eq!(flush.iter().map(|t| t.seq).collect::<Vec<_>>(), seqs);
    sqlx::query("UPDATE porchlight.items SET visibility='private' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        porchlight_db::stream::deliveries(&s.state.pool, s.init.household_id, reader, &seqs)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("UPDATE porchlight.items SET visibility='household' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM porchlight.outbox WHERE stream_seq=$1")
        .bind(seqs[0])
        .execute(&pool)
        .await
        .unwrap();
    let flush =
        porchlight_db::stream::deliveries(&s.state.pool, s.init.household_id, reader, &seqs)
            .await
            .unwrap();
    assert_eq!(flush.iter().map(|t| t.seq).collect::<Vec<_>>(), seqs[1..]);
    sqlx::query("UPDATE porchlight.household_members SET deleted_at=now() WHERE id=$1")
        .bind(reader)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        porchlight_db::stream::deliveries(&s.state.pool, s.init.household_id, reader, &seqs)
            .await
            .unwrap()
            .is_empty()
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn stream_query_plan_uses_typed_primary_key_lookup(pool: PgPool) {
    let s = start(&pool).await;
    let source = include_str!("../../db/src/stream.rs");
    assert!(
        source.contains("let row = sqlx::query!("),
        "ACL delivery query must be compile-time checked"
    );
    let query = source
        .split("let row = sqlx::query!(")
        .nth(1)
        .unwrap()
        .split("r#\"")
        .nth(1)
        .unwrap()
        .split("\"#")
        .next()
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    // Timing-independent: disabling seq scans makes an unindexable arm visible as
    // a forced Seq Scan, regardless of fixture size, machine load or planner costs.
    sqlx::query("SET LOCAL enable_seqscan=off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "EXPLAIN (FORMAT JSON) {query}"
    )))
    .bind(s.init.household_id)
    .bind(s.init.admin_member_id)
    .bind(0_i64)
    .bind(500_i64)
    .bind(i64::MAX)
    .bind(None::<Vec<i64>>)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    fn item_lookup(v: &Value) -> bool {
        if v["Relation Name"] == "items" {
            assert!(
                matches!(
                    v["Index Name"].as_str(),
                    Some("items_pkey" | "items_household_id_id_key")
                ),
                "{v}"
            );
            assert!(v["Index Cond"].as_str().unwrap().contains("(id ="));
            return true;
        }
        v["Plans"]
            .as_array()
            .is_some_and(|p| p.iter().any(item_lookup))
    }
    assert!(item_lookup(&plan[0]["Plan"]), "missing item lookup: {plan}");
    tx.rollback().await.unwrap();
    s.stop().await;
}

#[test]
fn serialized_tickle_conforms_to_openapi_schema() {
    let schema = &serde_json::to_value(porchlight_api::openapi::document()).unwrap()["components"]
        ["schemas"]["StreamTickle"];
    let required = schema["required"].as_array().unwrap();
    for evicted in [false, true] {
        let wire = serde_json::to_value(porchlight_db::stream::Tickle {
            seq: 42,
            kind: "item.changed".into(),
            entity_type: "item".into(),
            entity_id: Uuid::new_v4(),
            evicted,
        })
        .unwrap();
        for name in required {
            assert!(wire.get(name.as_str().unwrap()).is_some());
        }
        for (name, value) in wire.as_object().unwrap() {
            let property = &schema["properties"][name];
            assert!(!property.is_null(), "undocumented wire property {name}");
            let kind = if value.is_boolean() {
                "boolean"
            } else if value.is_i64() {
                "integer"
            } else {
                "string"
            };
            assert!(
                property["type"].to_string().contains(kind),
                "{name}: {property}"
            );
        }
        assert_eq!(wire.get("evicted"), evicted.then_some(&json!(true)));
        assert!(!required.contains(&json!("evicted")));
        assert_eq!(schema["properties"]["entity_id"]["format"], "uuid");
    }
}

// Use one deadline for the whole search: eager heartbeats and unrelated item /
// settings frames are expected, never assume the next frame is our notification.
async fn notification_frame(st: &mut TestStream, kind: &str, id: Uuid) -> SseFrame {
    timeout(Duration::from_secs(5), async {
        loop {
            let f = st.next().await.expect("stream ended before notification");
            if f.event.as_deref() == Some(kind) && data(&f)["entity_id"] == id.to_string() {
                assert_eq!(data(&f)["entity_type"], "notification");
                assert!(!f.data.contains("Secret title"));
                assert!(data(&f).get("summary").is_none());
                assert!(data(&f).get("member_id").is_none());
                return f;
            }
        }
    })
    .await
    .expect("notification deadline")
}
async fn through_barrier(st: &mut TestStream, barrier: Uuid) -> Vec<SseFrame> {
    timeout(Duration::from_secs(5), async {
        let mut frames = Vec::new();
        loop {
            let f = st.next().await.expect("stream ended before barrier");
            if f.event.is_none() || f.event.as_deref() == Some("ready") {
                continue;
            }
            if f.event.as_deref() == Some("member.changed")
                && data(&f)["entity_id"] == barrier.to_string()
            {
                return frames;
            }
            frames.push(f);
        }
    })
    .await
    .expect("barrier deadline")
}
async fn barrier(pool: &PgPool, s: &TestServer) -> Uuid {
    let id = synthetic(pool, s, "member.changed", "member", json!({})).await;
    project(pool).await;
    id
}
async fn notification_input(
    pool: &PgPool,
    s: &TestServer,
    recipient: Uuid,
    source: Option<Uuid>,
    kind: &str,
    now: chrono::DateTime<chrono::Utc>,
) {
    sqlx::query("INSERT INTO porchlight.notification_events(household_id,recipient_member_id,kind,item_id,entity_type,entity_id,created_at) VALUES ($1,$2,$3,$4,'item',$5,$6)")
        .bind(s.init.household_id).bind(recipient).bind(kind).bind(source).bind(source.unwrap_or(recipient)).bind(now).execute(pool).await.unwrap();
    let app = porchlight_db::testing::pool_as(
        pool,
        porchlight_db::testing::Role::App,
        Some("stream-notifier-app"),
    )
    .await
    .unwrap();
    porchlight_api::notifier::drain(&app, now).await.unwrap();
    app.close().await;
    project(pool).await;
}
async fn notification_id(pool: &PgPool, member: Uuid, category: &str) -> Uuid {
    sqlx::query_scalar("SELECT id FROM porchlight.notifications WHERE recipient_member_id=$1 AND category=$2 ORDER BY created_at DESC,id DESC LIMIT 1")
        .bind(member).bind(category).fetch_one(pool).await.unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn notification_sse_matrix_rows_use_real_tls_streams(pool: PgPool) {
    use porchlight_api::permissions::{Policy, Relationship, STREAM_ENDPOINTS, matrix_rows};
    let s = start(&pool).await;
    let mut devices = Vec::new();
    for name in ["owner", "assignee", "viewer", "subject", "other"] {
        let member = user_named(&pool, &s, name, name).await.1;
        let d = s.session_device(member, Platform::Desktop).await;
        devices.push((member, d));
    }
    devices.push((s.init.admin_member_id, device(&s).await));
    let outsider = user_named(&pool, &s, "outsider", "Outsider").await.1;
    let mut pats = Vec::new();
    for scope in ["read", "write", "admin"] {
        pats.push((scope, seed_pat(&pool, &s, &[scope]).await.1));
    }
    project(&pool).await;
    let rows: Vec<_> = matrix_rows(STREAM_ENDPOINTS)
        .into_iter()
        .filter(|r| r.endpoint.case.starts_with("SSE notification."))
        .collect();
    assert_eq!(rows.len(), 162);
    let mut checked = 0;
    for row in rows {
        assert!(matches!(
            row.endpoint.policy,
            Policy::StreamResource {
                member_scoped: true,
                ..
            }
        ));
        if matches!(row.relationship, Relationship::Agent) {
            let scope = match row.pat_scope.unwrap() {
                porchlight_api::permissions::PatScope::Read => "read",
                porchlight_api::permissions::PatScope::Write => "write",
                porchlight_api::permissions::PatScope::Admin => "admin",
            };
            let token = &pats.iter().find(|p| p.0 == scope).unwrap().1;
            let reply = agent(
                &s,
                Some(token),
                "GET",
                "/v1/stream",
                json!({}),
                "notification-matrix",
            )
            .await;
            assert_eq!(reply.status, row.status);
        } else {
            let index = match row.relationship {
                Relationship::Owner => 0,
                Relationship::Assignee => 1,
                Relationship::Viewer => 2,
                Relationship::Subject => 3,
                Relationship::Other => 4,
                Relationship::SuperAdmin => 5,
                Relationship::Agent => unreachable!(),
            };
            let (member, d) = &devices[index];
            let before = porchlight_db::stream::maximum(&pool).await.unwrap();
            let mut st = open(&s, d, Some(before)).await;
            let kind = row.endpoint.case.split_whitespace().nth(1).unwrap();
            let id = synthetic(
                &pool,
                &s,
                kind,
                "notification",
                json!({"member_id":if row.is_self {*member} else {outsider}}),
            )
            .await;
            let fence = barrier(&pool, &s).await;
            let frames = through_barrier(&mut st, fence).await;
            let delivered: Vec<_> = frames
                .iter()
                .filter(|f| {
                    f.event.as_deref() == Some(kind) && data(f)["entity_id"] == id.to_string()
                })
                .collect();
            assert_eq!(delivered.len(), usize::from(row.status == 200), "{row:?}");
            drop(st);
        }
        checked += 1;
    }
    assert_eq!(checked, 162);
    // Dependents have no user/session and cannot consume any member's stream.
    let dependent: Uuid = sqlx::query_scalar("INSERT INTO porchlight.household_members(household_id,role,display_name,color,tzid) VALUES ($1,'dependent','Child','#AABBCC','UTC') RETURNING id")
        .bind(s.init.household_id).fetch_one(&pool).await.unwrap();
    let mut child = s.issue_device(dependent, Platform::Desktop).await;
    assert_ne!(s.login(&mut child, &s.admin_password).await.status, 200);
    let denied = get(&s, &child, "/v1/stream").await;
    assert_eq!(denied.status, 401);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn notifier_stream_cross_device_read_and_read_all_are_recipient_private(pool: PgPool) {
    let now: chrono::DateTime<chrono::Utc> = "2026-10-06T12:00:00Z".parse().unwrap();
    let s = start_with(
        &pool,
        TestOptions {
            item_clock: std::sync::Arc::new(move || now),
            ..Default::default()
        },
    )
    .await;
    let recipient = user_named(&pool, &s, "recipient", "Recipient").await.1;
    let other = user_named(&pool, &s, "other", "Other").await.1;
    let d1 = s.session_device(recipient, Platform::Desktop).await;
    let d2 = s.session_device(recipient, Platform::Android).await;
    let d3 = s.session_device(other, Platform::Desktop).await;
    let source = item(&pool, &s, "household").await;
    project(&pool).await;
    let mut first = open(&s, &d1, None).await;
    let mut second = open(&s, &d2, None).await;
    let mut unrelated = open(&s, &d3, None).await;
    notification_input(&pool, &s, recipient, Some(source), "assigned", now).await;
    let id = notification_id(&pool, recipient, "assignments").await;
    notification_frame(&mut first, "notification.created", id).await;
    notification_frame(&mut second, "notification.created", id).await;
    // Real coalescing UPDATE trigger must reach both devices, not just INSERT.
    notification_input(
        &pool,
        &s,
        recipient,
        Some(source),
        "assigned",
        now + chrono::Duration::seconds(1),
    )
    .await;
    notification_frame(&mut first, "notification.updated", id).await;
    notification_frame(&mut second, "notification.updated", id).await;
    let read = post(&s, &d1, &format!("/v1/notifications/{id}/read"), json!({})).await;
    assert_eq!(read.status, 200);
    assert_eq!(read.json()["read_at"], "2026-10-06T12:00:00Z");
    project(&pool).await;
    notification_frame(&mut first, "notification.dismissed", id).await;
    notification_frame(&mut second, "notification.dismissed", id).await;
    assert!(
        get(&s, &d2, "/v1/notifications?unread=true").await.json()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    notification_input(
        &pool,
        &s,
        recipient,
        Some(source),
        "completed",
        now + chrono::Duration::seconds(2),
    )
    .await;
    let id2 = notification_id(&pool, recipient, "activity").await;
    notification_frame(&mut first, "notification.created", id2).await;
    notification_frame(&mut second, "notification.created", id2).await;
    let all = post(&s, &d1, "/v1/notifications/read-all", json!({})).await;
    assert_eq!(all.status, 200);
    assert_eq!(all.json()["count"], 1);
    project(&pool).await;
    notification_frame(&mut first, "notification.dismissed", id2).await;
    notification_frame(&mut second, "notification.dismissed", id2).await;
    assert!(
        get(&s, &d2, "/v1/notifications?unread=true").await.json()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let fence = barrier(&pool, &s).await;
    assert!(
        through_barrier(&mut unrelated, fence)
            .await
            .iter()
            .all(|f| !f.event.as_deref().unwrap().starts_with("notification."))
    );
    assert!(
        get(&s, &d3, "/v1/notifications").await.json()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    drop((first, second, unrelated));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn notification_replay_acl_eviction_and_resnapshot(pool: PgPool) {
    let now: chrono::DateTime<chrono::Utc> = "2026-10-06T12:00:00Z".parse().unwrap();
    let s = start_with(
        &pool,
        TestOptions {
            item_clock: std::sync::Arc::new(move || now),
            ..Default::default()
        },
    )
    .await;
    let recipient = user_named(&pool, &s, "recipient", "Recipient").await.1;
    let d = s.session_device(recipient, Platform::Desktop).await;
    let source = item(&pool, &s, "household").await;
    project(&pool).await;
    let before = porchlight_db::stream::maximum(&pool).await.unwrap();
    let mut live = open(&s, &d, Some(before)).await;
    notification_input(&pool, &s, recipient, Some(source), "assigned", now).await;
    let id = notification_id(&pool, recipient, "assignments").await;
    let created = notification_frame(&mut live, "notification.created", id).await;
    let cursor: i64 = created.id.unwrap().parse().unwrap();
    drop(live);
    sqlx::query("UPDATE porchlight.items SET visibility='private' WHERE id=$1")
        .bind(source)
        .execute(&pool)
        .await
        .unwrap();
    project(&pool).await;
    let mut replay = open(&s, &d, Some(before)).await;
    assert_eq!(
        notification_frame(&mut replay, "notification.created", id)
            .await
            .id
            .unwrap()
            .parse::<i64>()
            .unwrap(),
        cursor
    );
    let fence = barrier(&pool, &s).await;
    let frames = through_barrier(&mut replay, fence).await;
    assert!(
        frames
            .iter()
            .any(|f| f.event.as_deref() == Some("item.access_changed")
                && data(f)["entity_id"] == source.to_string()
                && data(f)["evicted"] == true)
    );
    assert!(
        get(&s, &d, "/v1/notifications").await.json()["items"]
            .as_array()
            .unwrap()
            .is_empty(),
        "member tickle is delivered, but inbox source ACL must hide it"
    );
    drop(replay);
    // Expired publication cursor, then a genuinely retained private notification.
    let old: i64 = sqlx::query_scalar("INSERT INTO porchlight.outbox(household_id,kind,entity_type,entity_id,created_at,stream_seq) VALUES ($1,'member.changed','member',$2,$3,nextval('porchlight.outbox_stream_seq')) RETURNING stream_seq")
        .bind(s.init.household_id).bind(recipient).bind(now-chrono::Duration::days(8)).fetch_one(&pool).await.unwrap();
    notification_input(
        &pool,
        &s,
        recipient,
        None,
        "completed",
        now + chrono::Duration::seconds(31),
    )
    .await;
    let safe = notification_id(&pool, recipient, "activity").await;
    let mut expired = open(&s, &d, Some(old)).await;
    let resnapshot = timeout(Duration::from_secs(5), async {
        loop {
            let f = expired.next().await.unwrap();
            if f.event.as_deref() == Some("resnapshot") {
                break f;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(data(&resnapshot)["reason"], "cursor_expired");
    let snapshot = get(&s, &d, "/v1/snapshot").await;
    assert_eq!(snapshot.status, 200);
    let snapshot = snapshot.json();
    assert_eq!(snapshot["notifications"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["notifications"][0]["id"], safe.to_string());
    let mut resumed = open(&s, &d, Some(snapshot["cursor"].as_i64().unwrap())).await;
    post(&s, &d, &format!("/v1/notifications/{safe}/read"), json!({})).await;
    project(&pool).await;
    notification_frame(&mut resumed, "notification.dismissed", safe).await;
    drop((expired, resumed));
    s.stop().await;
}

#[test]
fn notification_matrix_names_recipient_dimension() {
    let doc = porchlight_api::permissions::permissions_markdown();
    let rows: Vec<_> = doc
        .lines()
        .filter(|line| line.starts_with('|') && line.contains("SSE notification."))
        .collect();
    assert_eq!(rows.len(), 162);
    assert_eq!(
        rows.iter()
            .filter(|line| line.split('|').nth(6).unwrap().trim() == "self")
            .count(),
        81
    );
    assert_eq!(
        rows.iter()
            .filter(|line| line.split('|').nth(6).unwrap().trim() == "other")
            .count(),
        81
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn retention_resnapshots_held_and_unpolled_streams(pool: PgPool) {
    let s = start(&pool).await;
    let d = device(&s).await;
    assign_stream_seqs(&s.state.pool, 5000).await.unwrap();
    let mut held = open(&s, &d, None).await;
    assert_eq!(event(&mut held).await.event.as_deref(), Some("ready"));
    let mut slow = open(&s, &d, None).await; // Deliberately don't poll its opening body yet.
    let mut tx = porchlight_db::tx::begin_as(&s.state.pool, &porchlight_db::tx::Actor::system())
        .await
        .unwrap();
    let future = chrono::Utc::now() + chrono::Duration::days(10);
    let counts: Value = sqlx::query_scalar("select porchlight.pl_retention($1)")
        .bind(future)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(counts["outbox"].as_i64().unwrap() > 0);
    tx.commit().await.unwrap();
    for stream in [&mut held, &mut slow] {
        timeout(Duration::from_secs(5), async {
            loop {
                if event(stream).await.event.as_deref() == Some("resnapshot") {
                    break;
                }
            }
        })
        .await
        .unwrap();
    }
    let mut old = open(&s, &d, Some(0)).await;
    assert_eq!(event(&mut old).await.event.as_deref(), Some("resnapshot"));
    s.stop().await;
}
