//! Adversarial M7 regressions: deterministic clocks, UID isolation and exact lock blockers.
use chrono::{DateTime, Utc};
use porchlight_api::{
    calendar,
    extract::Caller,
    items::ics_import,
    testing::{TestOptions, TestServer, wait_blocked_behind},
};
use porchlight_db::{
    items::{ScopeRequest, WriteScope},
    tx::{Actor, begin_as},
};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

fn now() -> DateTime<Utc> {
    "2026-10-06T12:00:00Z".parse().unwrap()
}
fn event(uid: &str, details: &str) -> String {
    format!(
        "BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nSUMMARY:Class\r\n{details}END:VEVENT\r\n"
    )
}
fn cal(events: &str) -> String {
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{events}END:VCALENDAR\r\n")
}
async fn setup(pool: &PgPool) -> (TestServer, Caller, Uuid) {
    let s = TestServer::start_with(
        pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(now),
            ..Default::default()
        },
    )
    .await;
    let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    let id: Uuid = sqlx::query_scalar("insert into porchlight.ics_sources(household_id,owner_member_id,target_member_id,kind,import_key,name,url) values($1,$2,$2,'feed','m7fix','m7fix','https://example.invalid/calendar') returning id")
        .bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let c = Caller::Agent {
        token_id: Uuid::nil(),
        member_id: s.init.admin_member_id,
        household_id: s.init.household_id,
        label: "m7fix".into(),
        scopes: vec!["admin".into()],
        request_id: Uuid::now_v7(),
    };
    (s, c, id)
}
async fn apply(
    s: &TestServer,
    c: &Caller,
    id: Uuid,
    content: &str,
) -> Result<calendar::ImportSummary, porchlight_api::error::ApiError> {
    let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    let source = calendar::load_source(&mut tx, s.init.household_id, id, true)
        .await
        .unwrap();
    let result =
        ics_import::apply_calendar(&mut tx, c, &source, content, source.kind == "feed", now())
            .await;
    if result.is_ok() {
        tx.commit().await.unwrap();
    } else {
        tx.rollback().await.unwrap();
    }
    result
}

#[sqlx::test(migrations = "../db/migrations")]
async fn f2_retained_override_details_are_never_silently_lost(pool: PgPool) {
    let (s, c, id) = setup(&pool).await;
    let master = event(
        "class",
        "RRULE:FREQ=DAILY;COUNT=3\r\nLOCATION:Room A\r\nDESCRIPTION:Lesson\r\n",
    );
    let replacement = "BEGIN:VEVENT\r\nUID:class\r\nRECURRENCE-ID:20261013T090000Z\r\nDTSTART:20261013T110000Z\r\nDTEND:20261013T120000Z\r\nSUMMARY:Exam\r\nLOCATION:Room B\r\nDESCRIPTION:Bring pencils\r\nEND:VEVENT\r\n";
    let content = cal(&(master + replacement));
    for _ in 0..2 {
        let summary = apply(&s, &c, id, &content).await.unwrap();
        assert!(
            summary.warnings.iter().any(
                |w| w.uid.as_deref() == Some("class") && w.code == "UnsupportedOverrideDetails"
            ),
            "changed title/notes/location cannot be reported as lossless: {:?}",
            summary.warnings
        );
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn f4_bad_uid_isolated_from_valid_update_and_removal(pool: PgPool) {
    let (s, c, id) = setup(&pool).await;
    apply(
        &s,
        &c,
        id,
        &cal(&(event("good", "") + &event("removed", ""))),
    )
    .await
    .unwrap();
    for bad in ["x".repeat(4097), "nul\0uid".into()] {
        let content = cal(
            &(event("good", "").replace("SUMMARY:Class", "SUMMARY:Updated") + &event(&bad, "")),
        );
        let summary = apply(&s, &c, id, &content)
            .await
            .expect("one invalid UID cannot roll back the calendar");
        assert!(summary.warnings.iter().any(|w| w.code == "InvalidEvent"));
        let title: String = sqlx::query_scalar("select i.title from porchlight.items i join porchlight.ics_uid_map m on m.item_id=i.id where m.source_id=$1 and m.uid='good'").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(title, "Updated");
        let live: i64 = sqlx::query_scalar(
            "select count(*) from porchlight.items where ics_source_id=$1 and deleted_at is null",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(live, 1);
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn f5_rebuttal_new_exceptions_prelock_closed_imported_history(pool: PgPool) {
    let (s, c, _) = setup(&pool).await;
    let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    let id: Uuid = sqlx::query_scalar("insert into porchlight.ics_sources(household_id,owner_member_id,target_member_id,kind,import_key,name) values($1,$2,$2,'import','one-off','one-off') returning id").bind(s.init.household_id).bind(s.init.admin_member_id).fetch_one(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let content = cal(&event("class", "RRULE:FREQ=DAILY;COUNT=3\r\n"));
    apply(&s, &c, id, &content).await.unwrap();
    let rows: Vec<(Uuid,i32)> = sqlx::query_as("select o.id,o.version from porchlight.item_occurrences o join porchlight.items i on i.id=o.item_id where i.id in (select item_id from porchlight.ics_uid_map where source_id=$1) order by o.seq").bind(id).fetch_all(&pool).await.unwrap();
    for (index, status) in [
        porchlight_core::items::Status::Done,
        porchlight_core::items::Status::Skipped,
    ]
    .into_iter()
    .enumerate()
    {
        let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
        let mut scope = WriteScope::begin_at(
            &mut tx,
            s.init.household_id,
            ScopeRequest {
                occurrences: vec![rows[index].0],
                ..Default::default()
            },
            now(),
        )
        .await
        .unwrap();
        scope
            .set_status(
                rows[index].0,
                rows[index].1,
                status,
                Some(s.init.admin_member_id),
                now(),
            )
            .await
            .unwrap();
        scope.finish(now()).await.unwrap();
        tx.commit().await.unwrap();
    }
    // No exception mappings exist yet for either closed anchor. New moved
    // overrides also succeed on done and manually skipped imported history.
    let overrides = "BEGIN:VEVENT\r\nUID:class\r\nRECURRENCE-ID:20261012T090000Z\r\nDTSTART:20261012T110000Z\r\nDTEND:20261012T120000Z\r\nSUMMARY:Class\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:class\r\nRECURRENCE-ID:20261013T090000Z\r\nDTSTART:20261013T110000Z\r\nDTEND:20261013T120000Z\r\nSUMMARY:Class\r\nEND:VEVENT\r\n";
    let moved = cal(&(event("class", "RRULE:FREQ=DAILY;COUNT=3\r\n") + overrides));
    apply(&s, &c, id, &moved)
        .await
        .expect("new overrides on closed anchors are already prelocked");
    let detached: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.item_occurrences where id=any($1) and is_detached",
    )
    .bind(vec![rows[0].0, rows[1].0])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(detached, 2);
    let changed = content.replace(
        "RRULE:FREQ=DAILY;COUNT=3",
        "RRULE:FREQ=DAILY;COUNT=3\r\nEXDATE:20261012T090000Z,20261013T090000Z",
    );
    apply(&s, &c, id, &changed)
        .await
        .expect("new exceptions cannot fail on undeclared closed rows");
    let statuses: Vec<String> = sqlx::query_scalar(
        "select status from porchlight.item_occurrences where id=any($1) order by seq",
    )
    .bind(vec![rows[0].0, rows[1].0])
    .fetch_all(&pool)
    .await
    .unwrap();
    // R3 explicitly makes EXDATE skipped, including an existing completed anchor.
    assert_eq!(statuses, vec!["skipped", "skipped"]);
    let remaining: i64 =
        sqlx::query_scalar("select count(*) from porchlight.item_occurrences where id=any($1)")
            .bind(vec![rows[0].0, rows[1].0])
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        remaining, 2,
        "existing closed anchors, not replacement rows"
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn f6_retained_series_row_budget_is_atomic(pool: PgPool) {
    let (s, c, id) = setup(&pool).await;
    apply(&s, &c, id, &cal(&event("previous", "")))
        .await
        .unwrap();
    // Small remote payload projects >40k retained occurrence rows.
    let events = (0..100)
        .map(|i| event(&format!("daily-{i}"), "RRULE:FREQ=DAILY\r\n"))
        .collect::<String>();
    let content = cal(&events);
    assert!(content.len() < 20_000);
    let result = apply(&s, &c, id, &content).await;
    let rows: i64 = sqlx::query_scalar("select count(*) from porchlight.item_occurrences o join porchlight.items i on i.id=o.item_id where i.ics_source_id=$1").bind(id).fetch_one(&pool).await.unwrap();
    assert!(
        result
            .as_ref()
            .err()
            .is_some_and(|e| e.slug() == "calendar-too-large"),
        "calendar work must be rejected before exceeding 4000 work units; measured persisted rows={rows}"
    );
    assert_eq!(
        rows, 1,
        "budget failure preserves previous state atomically"
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn f3_feed_trigger_never_locks_deleted_items_after_members(pool: PgPool) {
    let (s, c, id) = setup(&pool).await;
    apply(
        &s,
        &c,
        id,
        &cal(&(event("older", "") + &event("newer", ""))),
    )
    .await
    .unwrap();
    let ids: Vec<Uuid> =
        sqlx::query_scalar("select id from porchlight.items where ics_source_id=$1 order by id")
            .bind(id)
            .fetch_all(&pool)
            .await
            .unwrap();
    sqlx::query("update porchlight.items set deleted_at=$2 where id=$1")
        .bind(ids[0])
        .bind(now())
        .execute(&pool)
        .await
        .unwrap();
    for remove in [false, true] {
        let mut a = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
        let source = calendar::load_source(&mut a, s.init.household_id, id, true)
            .await
            .unwrap();
        if remove {
            ics_import::remove_feed(&mut a, &source, now())
                .await
                .unwrap();
        } else {
            ics_import::configure_feed(
                &mut a,
                &source,
                &calendar::SourceRequest {
                    name: "renamed".into(),
                    url: source.url.clone().unwrap(),
                    target_member_id: source.target_member_id,
                    visibility: Some(source.visibility.clone()),
                    allow_http: false,
                    refresh_seconds: 86400,
                    enabled: true,
                },
                now(),
            )
            .await
            .unwrap();
        }
        let holder: i32 = sqlx::query_scalar("select pg_backend_pid()")
            .fetch_one(&mut *a)
            .await
            .unwrap();
        let p = s.state.pool.clone();
        let household = s.init.household_id;
        let member = s.init.admin_member_id;
        // The actual zone scope includes the older deleted item, then waits for live item A.
        let b = tokio::spawn(async move {
            let mut tx = begin_as(&p, &Actor::system()).await.unwrap();
            let result = WriteScope::begin_at(
                &mut tx,
                household,
                ScopeRequest {
                    member_zones: vec![member],
                    enqueue_only: true,
                    ..Default::default()
                },
                now(),
            )
            .await;
            let ok = result.is_ok();
            drop(result);
            tx.rollback().await.unwrap();
            ok
        });
        wait_blocked_behind(&pool, holder, 1, Duration::from_secs(15))
            .await
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(15), async {
            if remove {
                sqlx::query("update porchlight.ics_sources set deleted_at=$2 where id=$1")
                    .bind(id)
                    .bind(now())
                    .execute(&mut *a)
                    .await
            } else {
                sqlx::query("update porchlight.ics_sources set name='renamed' where id=$1")
                    .bind(id)
                    .execute(&mut *a)
                    .await
            }
        })
        .await
        .expect("source operation has a bounded deadline");
        a.rollback().await.unwrap();
        assert!(
            b.await.unwrap(),
            "zone operation must complete without a deadlock"
        );
        assert!(
            result.is_ok(),
            "source trigger locked an undeclared deleted item: {result:?}"
        );
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn f4_oneoff_invalid_uid_warnings_are_safe_for_idempotency_jsonb(pool: PgPool) {
    use porchlight_api::testing::TestClient;
    use porchlight_core::auth::session::Platform;
    let (s, _, _) = setup(&pool).await;
    let device = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&device));
    for (index, bad) in ["x".repeat(4097), "nul\0uid".into()].iter().enumerate() {
        let bad_event = event(bad, "");
        let content = cal(&(event("good", "") + &bad_event + &bad_event));
        let body = serde_json::json!({"content":content,"member_id":s.init.admin_member_id,"scope":format!("bad-uid-{index}")});
        let key = format!("invalid-uid-{index}");
        for _ in 0..2 {
            let response = client
                .request(
                    s.lan,
                    device.request("POST", "/v1/imports/ics", body.clone(), Some(&key)),
                )
                .await
                .unwrap();
            assert_eq!(
                response.status, 200,
                "invalid UID warning must serialize and replay"
            );
            assert_eq!(response.json()["created"], 1);
            assert!(
                response.json()["warnings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|w| w["code"] == "InvalidEvent")
            );
        }
    }
    s.stop().await;
}

// Fixed-seed xorshift produces high-entropy ASCII, not TOAST-compressible repeats.
fn entropy_uid(bytes: usize) -> String {
    let mut seed = 0x91f2_034b_a5cd_6789_u64;
    (0..bytes)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (b'A' + (seed % 26) as u8) as char
        })
        .collect()
}
#[sqlx::test(migrations = "../db/migrations")]
async fn fix1_uid_bound_is_utf8_bytes(pool: PgPool) {
    let (s, c, id) = setup(&pool).await;
    let at = entropy_uid(1024);
    let summary = apply(&s, &c, id, &cal(&event(&at, ""))).await.unwrap();
    assert_eq!(summary.created, 1);
    for bad in [entropy_uid(1025), entropy_uid(1023) + "é"] {
        let summary = apply(&s, &c, id, &cal(&(event(&at, "") + &event(&bad, ""))))
            .await
            .unwrap();
        assert_eq!(summary.created, 0, "UID beyond byte bound must be isolated");
        assert!(summary.warnings.iter().any(|w| w.code == "InvalidEvent"));
    }
    let multibyte_at = entropy_uid(1022) + "é";
    assert_eq!(multibyte_at.len(), 1024);
    assert_eq!(
        apply(
            &s,
            &c,
            id,
            &cal(&(event(&at, "") + &event(&multibyte_at, "")))
        )
        .await
        .unwrap()
        .created,
        1
    );
    let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    let error = sqlx::query("insert into porchlight.ics_uid_map(household_id,source_id,uid,recurrence_key,item_id) select household_id,source_id,$2,'',item_id from porchlight.ics_uid_map where source_id=$1 and uid=$3")
        .bind(id).bind(entropy_uid(1025)).bind(&at).execute(&mut *tx).await.unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("23514")
    );
    assert_eq!(
        error.as_database_error().unwrap().constraint(),
        Some("ics_uid_map_uid_bytes")
    );
    tx.rollback().await.unwrap();
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn fix1_nul_detail_fields_are_per_event(pool: PgPool) {
    let (s, c, id) = setup(&pool).await;
    for field in ["SUMMARY", "DESCRIPTION", "LOCATION"] {
        let replacement = if field == "SUMMARY" {
            "SUMMARY:bad\0value".to_owned()
        } else {
            format!("SUMMARY:Class\r\n{field}:bad\0value")
        };
        let bad = event("bad", "").replace("SUMMARY:Class", &replacement);
        let summary = apply(&s, &c, id, &cal(&(event("good", "") + &bad)))
            .await
            .expect("NUL detail must not abort valid events");
        assert!(
            summary
                .warnings
                .iter()
                .any(|w| w.code == "InvalidEvent" && w.uid.as_deref() == Some("bad"))
        );
    }
    s.stop().await;
}
fn worked_calendar() -> String {
    let mut events = (0..12)
        .map(|i| {
            event(&format!("weekly-{i}"), "RRULE:FREQ=WEEKLY\r\n").replace("20261012", "20231002")
        })
        .collect::<String>();
    events += &(0..200)
        .map(|i| event(&format!("single-{i}"), ""))
        .collect::<String>();
    cal(&events)
}
#[sqlx::test(migrations = "../db/migrations")]
#[ignore = "large occurrence fixture; scripts/ci.sh --full"]
async fn fix1_worked_calendar_refresh_and_clock_drift(pool: PgPool) {
    let (s, c, id) = setup(&pool).await;
    let content = worked_calendar();
    measured_apply(&s, &c, id, &content, "worked first").await;
    let changed = content.replacen("SUMMARY:Class", "SUMMARY:Changed", 1);
    measured_apply(&s, &c, id, &changed, "worked one-change").await;
    let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    let source = calendar::load_source(&mut tx, s.init.household_id, id, true)
        .await
        .unwrap();
    let summary = ics_import::apply_calendar(
        &mut tx,
        &c,
        &source,
        &changed,
        true,
        "2027-10-07T12:00:00Z".parse().unwrap(),
    )
    .await
    .expect("hash-equal calendar cannot accrue work with horizon");
    assert_eq!(summary.unchanged, 212);
    tx.commit().await.unwrap();
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
#[ignore = "large parse fixture; scripts/ci.sh --full"]
async fn fix1_r12_ten_thousand_expanded_instances(pool: PgPool) {
    let (s, c, id) = setup(&pool).await;
    // R12 is the parse cap only; R76's measured apply cap rejects this atomically.
    let content = cal(&event("hourly", "RRULE:FREQ=HOURLY;COUNT=10000\r\n"));
    let error = apply(&s, &c, id, &content)
        .await
        .err()
        .expect("above measured cap");
    assert_eq!(error.slug(), "calendar-too-large");
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn fix1_budget_rejection_has_distinct_reason(pool: PgPool) {
    let (s, c, id) = setup(&pool).await;
    apply(&s, &c, id, &cal(&event("previous", "")))
        .await
        .unwrap();
    let oversized = cal(&(0..100)
        .map(|i| event(&format!("daily-{i}"), "RRULE:FREQ=DAILY\r\n"))
        .collect::<String>());
    let e = apply(&s, &c, id, &oversized)
        .await
        .err()
        .expect("must reject");
    assert_eq!(e.slug(), "calendar-too-large");
    let rows: i64 =
        sqlx::query_scalar("select count(*) from porchlight.items where ics_source_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rows, 1);
    use porchlight_api::{
        routes,
        server::{ConnInfo, Transport},
    };
    use porchlight_core::auth::session::Platform;
    use tower::ServiceExt;
    let device = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let router = routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(device.fingerprint),
    }));
    let response = router
        .oneshot(device.request(
            "POST",
            "/v1/imports/ics",
            serde_json::json!({
                "content":oversized,"member_id":s.init.admin_member_id,"scope":"oversized"
            }),
            Some("oversized-key"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), 422);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["type"], "urn:porchlight:problem:calendar-too-large");
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn fix1_deadline_cancels_and_rolls_back_after_domain_write(pool: PgPool) {
    use porchlight_api::testing::{CalendarApplyProbe, with_calendar_apply_probe};
    let (s, c, id) = setup(&pool).await;
    let (result, _) = with_calendar_apply_probe(
        CalendarApplyProbe {
            duration: Some(Duration::from_secs(120)),
            expire_after_write: true,
            ..Default::default()
        },
        apply(&s, &c, id, &cal(&event("new", ""))),
    )
    .await;
    let e = result
        .err()
        .expect("deterministically expired after a domain write");
    assert!(
        e.is_transient(),
        "apply deadline is retryable, not calendar validation: {e:?}"
    );
    let rows: i64 =
        sqlx::query_scalar("select count(*) from porchlight.items where ics_source_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rows, 0, "cancelled apply must roll back its created item");
    apply(&s, &c, id, &cal(&event("new", ""))).await.unwrap();
    s.stop().await;
}

async fn measured_apply(
    s: &TestServer,
    c: &Caller,
    id: Uuid,
    content: &str,
    label: &str,
) -> calendar::ImportSummary {
    use porchlight_api::testing::{CalendarApplyProbe, with_calendar_apply_probe};
    let (result, probe) =
        with_calendar_apply_probe(CalendarApplyProbe::default(), apply(s, c, id, content)).await;
    let summary = result.unwrap_or_else(|e| panic!("{label}: {e:?}"));
    let (items,rows):(i64,i64) = sqlx::query_as("select count(distinct i.id),count(o.id) from porchlight.items i left join porchlight.item_occurrences o on o.item_id=i.id where i.ics_source_id=$1").bind(id).fetch_one(&s.state.pool).await.unwrap();
    println!(
        "MEASURE {label}: budget={:?}, projected_rows={:?}, charged_existing_rows={:?}, persisted_items={items}, persisted_occurrences={rows}, created={}, updated={}, unchanged={}",
        probe.work,
        probe.projected_rows,
        probe.existing_rows,
        summary.created,
        summary.updated,
        summary.unchanged
    );
    let (projected, existing) = match label {
        "worked first" => (3032, 0),
        "worked one-change" => (236, 236),
        "10k first" => (10000, 0),
        "10k one-change" => (1, 1),
        "near-limit first" => (3258, 0),
        "near-limit one-change" => (543, 543),
        _ => panic!("unknown measurement fixture"),
    };
    assert_eq!(probe.projected_rows, vec![projected]);
    assert_eq!(probe.existing_rows, vec![existing]);
    summary
}
#[sqlx::test(migrations = "../db/migrations")]
#[ignore = "large occurrence fixture; scripts/ci.sh --full"]
async fn fix1_near_limit_calendar_admits_one_change(pool: PgPool) {
    let (s, c, id) = setup(&pool).await;
    let content = cal(&(0..6)
        .map(|i| event(&format!("daily-{i}"), "RRULE:FREQ=DAILY\r\n"))
        .collect::<String>());
    measured_apply(&s, &c, id, &content, "near-limit first").await;
    let changed = content.replacen("SUMMARY:Class", "SUMMARY:Changed", 1);
    let summary = measured_apply(&s, &c, id, &changed, "near-limit one-change").await;
    assert_eq!(summary.updated, 1);
    assert_eq!(summary.unchanged, 5);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn fix1_import_deadline_is_not_sealed_by_idempotency(pool: PgPool) {
    use porchlight_api::{
        routes,
        server::{ConnInfo, Transport},
        testing::{CalendarApplyProbe, with_calendar_apply_probe},
    };
    use porchlight_core::auth::session::Platform;
    use tower::ServiceExt;
    let (s, _, _) = setup(&pool).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let router = routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(d.fingerprint),
    }));
    let body = serde_json::json!({"content":cal(&event("new", "")),"member_id":s.init.admin_member_id,"scope":"deadline"});
    let (response, _) = with_calendar_apply_probe(
        CalendarApplyProbe {
            duration: Some(Duration::from_secs(120)),
            expire_after_write: true,
            ..Default::default()
        },
        router.clone().oneshot(d.request(
            "POST",
            "/v1/imports/ics",
            body.clone(),
            Some("deadline-key"),
        )),
    )
    .await;
    let response = response.unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(response.headers()["retry-after"], "1");
    let claims: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.idempotency_keys where idem_key='deadline-key'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(claims, 0, "transient failure must discard its claim");
    let response = router
        .oneshot(d.request("POST", "/v1/imports/ics", body, Some("deadline-key")))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        200,
        "same key must execute, not replay failure"
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn fix1_unchanged_calendar_does_not_drift_over_budget(pool: PgPool) {
    use porchlight_api::testing::{CalendarApplyProbe, with_calendar_apply_probe};
    let (s, c, id) = setup(&pool).await;
    let content = cal(&(0..3)
        .map(|i| event(&format!("daily-{i}"), "RRULE:FREQ=DAILY\r\n"))
        .collect::<String>());
    apply(&s, &c, id, &content).await.unwrap();
    let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    let source = calendar::load_source(&mut tx, s.init.household_id, id, true)
        .await
        .unwrap();
    let (result, probe) = with_calendar_apply_probe(
        CalendarApplyProbe::default(),
        ics_import::apply_calendar(
            &mut tx,
            &c,
            &source,
            &content,
            true,
            "2027-10-07T12:00:00Z".parse().unwrap(),
        ),
    )
    .await;
    assert_eq!(result.unwrap().unchanged, 3);
    assert_eq!(
        probe.work,
        vec![3],
        "unchanged stored rows and future anchors are not charged"
    );
    tx.commit().await.unwrap();
    s.stop().await;
}

// Counts SQLx execution events in this apply future, not concurrent test/setup work.
#[derive(Clone)]
struct StatementCounter(Arc<std::sync::atomic::AtomicUsize>);
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for StatementCounter {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if event.metadata().target() == "sqlx::query" {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}
fn standalone_calendar(n: usize) -> String {
    standalone_calendar_with(n, "Measured")
}
fn standalone_calendar_with(n: usize, summary: &str) -> String {
    let start = now();
    cal(&(0..n).map(|i| {
        let t = start + chrono::Duration::hours(i as i64 * 2);
        format!("BEGIN:VEVENT\r\nUID:item-{i}\r\nDTSTART:{}\r\nDTEND:{}\r\nSUMMARY:{summary}\r\nEND:VEVENT\r\n", t.format("%Y%m%dT%H%M%SZ"), (t + chrono::Duration::hours(1)).format("%Y%m%dT%H%M%SZ"))
    }).collect::<String>())
}
async fn count_apply(s: &TestServer, c: &Caller, id: Uuid, n: usize) -> (usize, Duration) {
    let (statements, elapsed, result) =
        count_content(s, c, id, &standalone_calendar(n), "new_items").await;
    assert_eq!(result.created as usize, n);
    assert!(statements > n, "query instrumentation must be active");
    (statements, elapsed)
}
#[sqlx::test(migrations = "../db/migrations")]
async fn fix2_statement_budget(pool: PgPool) {
    let (s, c, id) = setup(&pool).await;
    use porchlight_api::testing::{CalendarApplyProbe, with_calendar_apply_probe};
    let ((statements, _), probe) =
        with_calendar_apply_probe(CalendarApplyProbe::default(), count_apply(&s, &c, id, 200))
            .await;
    assert_eq!(probe.work, vec![400]);
    assert_eq!(ics_import::MAX_CALENDAR_WORK, 4000);
    assert!(
        statements <= 12 * probe.work[0] + 40 + 200_usize.div_ceil(500),
        "statement budget exceeded: {statements}"
    );
    let ((statements, _, changed), probe) = with_calendar_apply_probe(
        CalendarApplyProbe::default(),
        count_content(
            &s,
            &c,
            id,
            &standalone_calendar_with(200, "Changed"),
            "small change",
        ),
    )
    .await;
    assert_eq!(changed.updated, 200);
    assert_eq!(probe.work, vec![3 * 200]);
    assert!(statements > 200, "query instrumentation must be active");
    // Changes also replace feed relationships: measured 53 statements/item + 45 fixed.
    assert!(
        statements <= 54 * 200 + 64,
        "change statements: {statements}"
    );
    let ((statements, _, removed), probe) = with_calendar_apply_probe(
        CalendarApplyProbe::default(),
        count_content(
            &s,
            &c,
            id,
            &standalone_calendar_with(1, "Changed"),
            "small removal",
        ),
    )
    .await;
    assert_eq!(removed.removed, 199);
    assert_eq!(probe.work, vec![1 + 3 * 199]);
    assert!(statements > 199, "query instrumentation must be active");
    assert!(
        statements <= 12 * probe.work[0] + 40 + 199_usize.div_ceil(500),
        "removal statements: {statements}"
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
#[ignore = "wall-clock full-cap measurement; scripts/ci.sh --full"]
async fn fix2_full_cap_production_deadline(pool: PgPool) {
    let (s, c, id) = setup(&pool).await;
    use porchlight_api::testing::{CalendarApplyProbe, with_calendar_apply_probe};
    let ((statements, elapsed), probe) = with_calendar_apply_probe(
        CalendarApplyProbe::default(),
        count_apply(&s, &c, id, ics_import::MAX_CALENDAR_WORK / 2),
    )
    .await;
    assert_eq!(probe.work, vec![ics_import::MAX_CALENDAR_WORK]);
    assert!(
        statements <= 12 * probe.work[0] + 40 + (ics_import::MAX_CALENDAR_WORK / 2).div_ceil(500)
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "full-cap new-item apply must use at most half the fixed 20s deadline: {elapsed:?}"
    );
    s.stop().await;
}

// M3: statement count and wall time of one apply future (setup excluded by the caller).
async fn count_content(
    s: &TestServer,
    c: &Caller,
    id: Uuid,
    content: &str,
    label: &str,
) -> (usize, Duration, calendar::ImportSummary) {
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::prelude::*;
    // Keep callsites enabled even between polls of the scoped future.
    let _ = tracing::subscriber::set_global_default(
        tracing_subscriber::registry().with(tracing_subscriber::filter::LevelFilter::TRACE),
    );
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry()
        .with(StatementCounter(count.clone()))
        .with(tracing_subscriber::filter::LevelFilter::TRACE);
    let start = std::time::Instant::now();
    let summary = async { apply(s, c, id, content).await }
        .with_subscriber(subscriber)
        .await
        .unwrap();
    let elapsed = start.elapsed();
    let statements = count.load(std::sync::atomic::Ordering::Relaxed);
    println!(
        "COST {label} statements={statements} elapsed_ms={}",
        elapsed.as_millis()
    );
    assert!(statements > 0, "query instrumentation must be active");
    (statements, elapsed, summary)
}
#[sqlx::test(migrations = "../db/migrations")]
#[ignore = "wall-clock full-cap removal measurement; scripts/ci.sh --full"]
async fn fix3_full_cap_removal_apply(pool: PgPool) {
    use porchlight_api::testing::{CalendarApplyProbe, with_calendar_apply_probe};
    let (s, c, id) = setup(&pool).await;
    // M3: a removed standalone item costs 2 units + its 1 occurrence row; the kept event costs 1.
    let removed = (ics_import::MAX_CALENDAR_WORK - 1) / 3;
    let n = removed + 1;
    apply(&s, &c, id, &standalone_calendar(n)).await.unwrap();
    let ((statements, elapsed, summary), probe) = with_calendar_apply_probe(
        CalendarApplyProbe::default(),
        count_content(&s, &c, id, &standalone_calendar(1), "full-cap removal"),
    )
    .await;
    println!(
        "MEASURE removal items_before={n} work={:?} removed={}",
        probe.work, summary.removed
    );
    assert_eq!(probe.work, vec![1 + 3 * removed]);
    assert_eq!(summary.removed as usize, removed);
    assert!(
        elapsed < Duration::from_secs(10),
        "full-cap removal apply: {elapsed:?} ({statements} statements)"
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
#[ignore = "wall-clock full-cap change measurement; scripts/ci.sh --full"]
async fn fix3_full_cap_change_apply(pool: PgPool) {
    use porchlight_api::testing::{CalendarApplyProbe, with_calendar_apply_probe};
    let (s, c, id) = setup(&pool).await;
    let n = ics_import::MAX_CALENDAR_WORK / 3;
    apply(&s, &c, id, &standalone_calendar(n)).await.unwrap();
    let ((statements, elapsed, summary), probe) = with_calendar_apply_probe(
        CalendarApplyProbe::default(),
        count_content(
            &s,
            &c,
            id,
            &standalone_calendar_with(n, "Changed"),
            "full-cap change",
        ),
    )
    .await;
    println!(
        "MEASURE change items={n} work={:?} updated={}",
        probe.work, summary.updated
    );
    assert_eq!(summary.updated as usize, n);
    assert_eq!(probe.work, vec![3 * n]);
    assert!(
        statements <= 54 * n + 64,
        "full-cap change statements: {statements}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "full-cap change apply: {elapsed:?} ({statements} statements)"
    );
    s.stop().await;
}

// I-new-3: earlier removals keep their mappings but cost nothing on later refreshes.
#[sqlx::test(migrations = "../db/migrations")]
async fn fix4_already_removed_items_are_not_recharged(pool: PgPool) {
    use porchlight_api::testing::{CalendarApplyProbe, with_calendar_apply_probe};
    let (s, c, id) = setup(&pool).await;
    let events = |uids: &[&str]| cal(&uids.iter().map(|u| event(u, "")).collect::<String>());
    apply(&s, &c, id, &events(&["live", "gone1", "gone2", "gone3"]))
        .await
        .unwrap();
    let gone: Uuid = sqlx::query_scalar(
        "SELECT item_id FROM porchlight.ics_uid_map WHERE source_id=$1 AND uid='gone1'",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (r, probe) = with_calendar_apply_probe(
        CalendarApplyProbe::default(),
        apply(&s, &c, id, &events(&["live"])),
    )
    .await;
    assert_eq!(r.unwrap().removed, 3);
    assert_eq!(
        probe.work,
        vec![1 + 3 * 3],
        "3 removals: 2 units + 1 row each"
    );
    // Same body again: the three soft-deleted mappings are retained but do no work.
    let (r, probe) = with_calendar_apply_probe(
        CalendarApplyProbe::default(),
        apply(&s, &c, id, &events(&["live"])),
    )
    .await;
    assert_eq!(r.unwrap().removed, 0);
    assert_eq!(probe.work, vec![1], "only the live event is charged");
    // A newly removed item is still charged, historical ones are not.
    apply(&s, &c, id, &events(&["live", "fresh"]))
        .await
        .unwrap();
    let (r, probe) = with_calendar_apply_probe(
        CalendarApplyProbe::default(),
        apply(&s, &c, id, &events(&["live"])),
    )
    .await;
    assert_eq!(r.unwrap().removed, 1);
    assert_eq!(probe.work, vec![1 + 3]);
    // Reappearance restores the original mapping/item instead of duplicating it.
    let (r, probe) = with_calendar_apply_probe(
        CalendarApplyProbe::default(),
        apply(&s, &c, id, &events(&["live", "gone1"])),
    )
    .await;
    let restored = r.unwrap();
    assert_eq!(
        (restored.created, restored.updated, restored.removed),
        (0, 1, 0)
    );
    assert_eq!(
        probe.work,
        vec![4],
        "two parsed events + retained/restored occurrence"
    );
    let mappings: Vec<(Uuid, bool)> = sqlx::query_as("SELECT m.item_id,i.deleted_at IS NULL FROM porchlight.ics_uid_map m JOIN porchlight.items i ON i.id=m.item_id WHERE m.source_id=$1 AND m.uid='gone1'")
        .bind(id).fetch_all(&pool).await.unwrap();
    assert_eq!(mappings, vec![(gone, true)]);
    s.stop().await;
}
