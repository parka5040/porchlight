//! M3 review regressions through the real porch process and agent API.
use porchlight_api::testing::{TestOptions, TestServer, trust};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    process::{Command, Stdio},
    sync::Arc,
};
async fn server(pool: &PgPool) -> TestServer {
    TestServer::start_with(
        pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(|| "2026-10-06T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await
}
async fn run(s: &TestServer, args: &[&str]) -> (i32, Value) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_porch"));
    c.args(args)
        .arg("--json")
        .env("PORCHLIGHT_SOCKET", &s.agent_socket)
        .env("PORCHLIGHT_TOKEN", s.bootstrap_token())
        .stdin(Stdio::null());
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::task::spawn_blocking(move || c.output().unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    (
        out.status.code().unwrap(),
        serde_json::from_slice(&out.stdout).unwrap(),
    )
}
async fn ok(s: &TestServer, args: &[&str]) -> Value {
    let (code, v) = run(s, args).await;
    assert_eq!(code, 0, "command {args:?}: {v}");
    v
}
async fn occurrences(s: &TestServer, id: &Value) -> Value {
    ok(s, &["occurrences", "list", id.as_str().unwrap()]).await["items"].clone()
}
#[sqlx::test(migrations = "../db/migrations")]
async fn this_edit_rejects_other_items_occurrence_with_and_without_version(pool: PgPool) {
    let s = server(&pool).await;
    let a = ok(
        &s,
        &[
            "items",
            "add",
            "--title",
            "A",
            "--floating",
            "--due",
            "2026-10-07T10:00",
        ],
    )
    .await;
    let c = ok(
        &s,
        &[
            "items",
            "add",
            "--title",
            "C",
            "--floating",
            "--due",
            "2026-10-07T11:00",
        ],
    )
    .await;
    let before = occurrences(&s, &c["id"]).await;
    let oid = before[0]["id"].as_str().unwrap();
    let ver = before[0]["version"].to_string();
    for explicit in [false, true] {
        let mut args = vec![
            "items",
            "edit",
            a["id"].as_str().unwrap(),
            "--scope",
            "this",
            "--occurrence",
            oid,
            "--due",
            "2026-10-07T12:00",
        ];
        if explicit {
            args.extend(["--version", &ver]);
        }
        let (code, _) = run(&s, &args).await;
        assert_ne!(
            code, 0,
            "contradictory item/occurrence must fail before mutation"
        );
        assert_eq!(occurrences(&s, &c["id"]).await, before);
    }
    // Matching parent still works, including explicit versions and dates outside snapshot.
    ok(
        &s,
        &[
            "items",
            "edit",
            c["id"].as_str().unwrap(),
            "--scope",
            "this",
            "--occurrence",
            oid,
            "--due",
            "2026-10-07T12:00",
        ],
    )
    .await;
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn this_edit_accepts_fixed_wall_and_offset_times(pool: PgPool) {
    let s = server(&pool).await;
    for (start, end, expected) in [
        (
            "2026-10-07T12:00",
            "2026-10-07T13:00",
            "2026-10-07T12:00:00",
        ),
        (
            "2026-10-07T16:00:00Z",
            "2026-10-07T17:00:00Z",
            "2026-10-07T12:00:00",
        ),
    ] {
        let item = ok(
            &s,
            &[
                "items",
                "add",
                "--title",
                "Fixed",
                "--tz",
                "America/New_York",
                "--start",
                "2026-10-07T10:00",
                "--end",
                "2026-10-07T11:00",
            ],
        )
        .await;
        let os = occurrences(&s, &item["id"]).await;
        let oid = os[0]["id"].as_str().unwrap();
        ok(
            &s,
            &[
                "items",
                "edit",
                item["id"].as_str().unwrap(),
                "--scope",
                "this",
                "--occurrence",
                oid,
                "--start",
                start,
                "--end",
                end,
            ],
        )
        .await;
        let after = occurrences(&s, &item["id"]).await;
        assert_eq!(after[0]["local_start"], expected);
        assert_eq!(after[0]["starts_at"], "2026-10-07T16:00:00Z");
        assert_eq!(after[0]["is_detached"], true);
    }
    let item = ok(
        &s,
        &[
            "items",
            "add",
            "--title",
            "Deadline",
            "--tz",
            "America/New_York",
            "--due",
            "2026-10-07T10:00",
        ],
    )
    .await;
    let os = occurrences(&s, &item["id"]).await;
    ok(
        &s,
        &[
            "items",
            "edit",
            item["id"].as_str().unwrap(),
            "--scope",
            "this",
            "--occurrence",
            os[0]["id"].as_str().unwrap(),
            "--due",
            "2026-10-07T16:00:00Z",
        ],
    )
    .await;
    assert_eq!(
        occurrences(&s, &item["id"]).await[0]["local_due"],
        "2026-10-07T12:00:00"
    );
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn detached_occurrence_remains_editable_after_rule_shrinks(pool: PgPool) {
    let s = server(&pool).await;
    let item = ok(
        &s,
        &[
            "items",
            "add",
            "--title",
            "Preserved",
            "--floating",
            "--due",
            "2026-10-07T10:00",
            "--rrule",
            "FREQ=DAILY;COUNT=3",
        ],
    )
    .await;
    let id = item["id"].as_str().unwrap();
    let os = occurrences(&s, &item["id"]).await;
    let oid = os[2]["id"].as_str().unwrap();
    ok(
        &s,
        &[
            "items",
            "edit",
            id,
            "--scope",
            "this",
            "--occurrence",
            oid,
            "--due",
            "2026-10-09T12:00",
        ],
    )
    .await;
    ok(&s, &["items", "edit", id, "--rrule", "FREQ=DAILY;COUNT=1"]).await;
    let preserved = occurrences(&s, &item["id"]).await;
    assert_eq!(preserved.as_array().unwrap().len(), 2);
    assert!(
        preserved
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["id"] == oid && o["is_detached"] == true)
    );
    ok(
        &s,
        &[
            "items",
            "edit",
            id,
            "--scope",
            "this",
            "--occurrence",
            oid,
            "--due",
            "2026-10-09T13:00",
        ],
    )
    .await;
    let after = occurrences(&s, &item["id"]).await;
    assert_eq!(after[1]["local_due"], "2026-10-09T13:00:00");
    assert_eq!(after[1]["id"], oid);
    // Invalid interval/date validation is not bypassed for preserved rows.
    let (code, _) = run(
        &s,
        &[
            "items",
            "edit",
            id,
            "--scope",
            "this",
            "--occurrence",
            oid,
            "--due",
            "bad-date",
        ],
    )
    .await;
    assert_ne!(code, 0);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn relationship_update_preserves_unchanged_rows_and_assignment_history(pool: PgPool) {
    let s = server(&pool).await;
    // A real user distinct from owner/actor: assignment notifications are not
    // excluded as dependent-only or self-suppressed.
    let member = trust::user(&pool, &s).await;
    let item = trust::admin(
        &s,
        "POST",
        "/v1/items",
        json!({"title":"Relationships","kind":"todo","assignees":[member],"reminder_offsets":[30]}),
    )
    .await;
    assert_eq!(item.status, 201, "{item:?}");
    let id: uuid::Uuid = serde_json::from_value(item.json()["id"].clone()).unwrap();
    let before: Vec<uuid::Uuid> =
        sqlx::query_scalar("select id from porchlight.item_reminders where item_id=$1")
            .bind(id)
            .fetch_all(&pool)
            .await
            .unwrap();
    let events: i64 = sqlx::query_scalar("select count(*) from porchlight.task_events where item_id=$1 and kind in ('assigned','unassigned')").bind(id).fetch_one(&pool).await.unwrap();
    let notifications: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.notification_events where item_id=$1 and kind='assigned'",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(notifications, 1, "non-vacuous assignment notification");
    for (suffix, body) in [
        ("reminders", json!({"reminder_offsets":[30,60]})),
        ("viewers", json!({"viewers":[member]})),
        ("assignees", json!({"assignees":[member]})),
    ] {
        let current = trust::admin(&s, "GET", &format!("/v1/items/{id}"), json!({})).await;
        let mut req = trust::req(
            "PUT",
            &format!("/v1/items/{id}/{suffix}"),
            body,
            &uuid::Uuid::now_v7().to_string(),
        );
        req.headers_mut().insert(
            "if-match",
            current.json()["version"].to_string().parse().unwrap(),
        );
        let r =
            porchlight_api::testing::agent_request(&s.agent_socket, Some(s.bootstrap_token()), req)
                .await;
        assert_eq!(r.status, 200, "{r:?}");
        let after: Vec<uuid::Uuid> = sqlx::query_scalar(
            "select id from porchlight.item_reminders where item_id=$1 and offset_minutes=30",
        )
        .bind(id)
        .fetch_all(&pool)
        .await
        .unwrap();
        let after_events: i64 = sqlx::query_scalar("select count(*) from porchlight.task_events where item_id=$1 and kind in ('assigned','unassigned')").bind(id).fetch_one(&pool).await.unwrap();
        let after_notifications: i64 = sqlx::query_scalar("select count(*) from porchlight.notification_events where item_id=$1 and kind='assigned'").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(
            (&after, after_events, after_notifications),
            (&before, events, notifications),
            "unchanged reminder identity, assignment history and notifications"
        );
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn this_edit_preserves_fixed_gap_nominal_wall_time(pool: PgPool) {
    let s = server(&pool).await;
    let item = ok(
        &s,
        &[
            "items",
            "add",
            "--title",
            "Gap",
            "--tz",
            "America/New_York",
            "--start",
            "2027-03-14T01:30",
            "--end",
            "2027-03-14T01:50",
        ],
    )
    .await;
    let os = occurrences(&s, &item["id"]).await;
    ok(
        &s,
        &[
            "items",
            "edit",
            item["id"].as_str().unwrap(),
            "--scope",
            "this",
            "--occurrence",
            os[0]["id"].as_str().unwrap(),
            "--start",
            "2027-03-14T02:10",
            "--end",
            "2027-03-14T02:50",
        ],
    )
    .await;
    let after = occurrences(&s, &item["id"]).await;
    assert_eq!(after[0]["local_start"], "2027-03-14T02:10:00");
    assert_eq!(after[0]["starts_at"], "2027-03-14T07:00:00Z");
    assert_eq!(after[0]["ends_at"], "2027-03-14T07:40:00Z");
    let (code, _) = run(
        &s,
        &[
            "items",
            "edit",
            item["id"].as_str().unwrap(),
            "--scope",
            "this",
            "--occurrence",
            os[0]["id"].as_str().unwrap(),
            "--title",
            "Must not change series",
        ],
    )
    .await;
    assert_ne!(code, 0);
    let (code, _) = run(
        &s,
        &[
            "items",
            "edit",
            item["id"].as_str().unwrap(),
            "--scope",
            "this",
            "--occurrence",
            os[0]["id"].as_str().unwrap(),
            "--start",
            "2027-03-14T04:00",
            "--end",
            "2027-03-14T03:00",
        ],
    )
    .await;
    assert_ne!(code, 0, "invalid complete interval still rejected");
    assert_eq!(occurrences(&s, &item["id"]).await, after);
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn this_edit_parent_check_includes_distant_and_paged_occurrences(pool: PgPool) {
    let s = server(&pool).await;
    let distant = ok(
        &s,
        &[
            "items",
            "add",
            "--title",
            "Distant",
            "--floating",
            "--due",
            "2030-10-07T10:00",
        ],
    )
    .await;
    let os = occurrences(&s, &distant["id"]).await;
    let snapshot = trust::admin(&s, "GET", "/v1/snapshot", json!({}))
        .await
        .json();
    assert!(
        !snapshot["occurrences"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["id"] == os[0]["id"])
    );
    ok(
        &s,
        &[
            "items",
            "edit",
            distant["id"].as_str().unwrap(),
            "--scope",
            "this",
            "--occurrence",
            os[0]["id"].as_str().unwrap(),
            "--due",
            "2030-10-07T12:00",
        ],
    )
    .await;
    let series = ok(
        &s,
        &[
            "items",
            "add",
            "--title",
            "Paged",
            "--floating",
            "--due",
            "2026-10-07T10:00",
            "--rrule",
            "FREQ=DAILY;COUNT=120",
        ],
    )
    .await;
    let os = occurrences(&s, &series["id"]).await;
    assert_eq!(os.as_array().unwrap().len(), 120);
    let target = &os[119];
    ok(
        &s,
        &[
            "items",
            "edit",
            series["id"].as_str().unwrap(),
            "--scope",
            "this",
            "--occurrence",
            target["id"].as_str().unwrap(),
            "--version",
            &target["version"].to_string(),
            "--due",
            "2027-02-03T12:00",
        ],
    )
    .await;
    let after = occurrences(&s, &series["id"]).await;
    assert_eq!(after[119]["local_due"], "2027-02-03T12:00:00");
    s.stop().await;
}
