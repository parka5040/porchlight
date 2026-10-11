use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use porchlight_api::{
    routes,
    testing::{TestOptions, TestServer},
};
use porchlight_db::{
    derive, items,
    tx::{Actor, begin_as},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn call(
    s: &TestServer,
    method: &str,
    path: &str,
    value: Value,
    version: Option<i64>,
) -> (u16, Value) {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("porchlight-client-version", porchlight_core::VERSION)
        .header("authorization", format!("Bearer {}", s.bootstrap_token()))
        .header("content-type", "application/json")
        .header("idempotency-key", Uuid::now_v7().to_string());
    if let Some(v) = version {
        b = b.header("if-match", v.to_string());
    }
    let r = routes::agent_router(s.state.clone())
        .layer(axum::Extension(porchlight_api::server::ConnInfo {
            transport: porchlight_api::server::Transport::Agent,
            client_addr: None,
            client_cert_fingerprint: None,
        }))
        .oneshot(b.body(Body::from(value.to_string())).unwrap())
        .await
        .unwrap();
    let status = r.status().as_u16();
    let bytes = to_bytes(r.into_body(), 8 * 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
async fn server(pool: &PgPool) -> TestServer {
    TestServer::start_with(
        pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-06T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await
}

#[sqlx::test(migrations = "../db/migrations")]
async fn snapshot_effective_override_window_and_reminder_references(pool: PgPool) {
    let s = server(&pool).await;
    let (status, item) = call(
        &s,
        "POST",
        "/v1/items",
        json!({
            "title":"Moved due", "kind":"deadline", "time_mode":"fixed", "tzid":"UTC",
            "due_at":"2026-01-01T12:00:00Z", "reminder_offsets":[0]
        }),
        None,
    )
    .await;
    assert_eq!(status, 201, "{item}");
    let id: Uuid = item["id"].as_str().unwrap().parse().unwrap();
    let occ: Uuid =
        sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    for (local_due, included) in [
        ("2026-10-07T12:00:00", true),
        ("2029-10-07T12:00:00", false),
    ] {
        sqlx::query("update porchlight.item_occurrences set override_local_due=$2::text::timestamp where id=$1").bind(occ).bind(local_due).execute(&pool).await.unwrap();
        let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
        derive::derive_item(&mut tx, id, "2026-10-06T12:00:00Z".parse().unwrap())
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let (status, snapshot) = call(&s, "GET", "/v1/snapshot", json!({}), None).await;
        assert_eq!(status, 200, "{snapshot}");
        let occurrences = snapshot["occurrences"].as_array().unwrap();
        let found = occurrences.iter().any(|o| o["id"] == occ.to_string());
        assert_eq!(
            found, included,
            "membership must use effective due {local_due}"
        );
        for reminder in snapshot["scheduled_reminders"].as_array().unwrap() {
            assert!(
                occurrences
                    .iter()
                    .any(|o| o["id"] == reminder["occurrence_id"]),
                "reminder without cached occurrence"
            );
        }
        if included {
            assert!(
                !snapshot["scheduled_reminders"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        }
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn revert_pre_split_head_rejects_without_duplicates_or_lost_tail_state(pool: PgPool) {
    let s = server(&pool).await;
    let (status, item) = call(&s, "POST", "/v1/items", json!({
        "title":"Split undo", "kind":"deadline", "time_mode":"fixed", "tzid":"UTC",
        "due_at":"2026-10-07T12:00:00Z", "rrule":"FREQ=DAILY;COUNT=5", "recurrence_mode":"calendar"
    }), None).await;
    assert_eq!(status, 201, "{item}");
    let id: Uuid = item["id"].as_str().unwrap().parse().unwrap();
    let revision: Uuid = sqlx::query_scalar(
        "select id from porchlight.item_revisions where item_id=$1 order by rev desc limit 1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    let head = derive::load_item(&mut tx, id).await.unwrap();
    let split = items::split(
        &mut tx,
        s.init.household_id,
        id,
        head.version,
        2,
        "2026-10-06T12:00:00Z".parse().unwrap(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    sqlx::query("update porchlight.item_occurrences set status='done', completed_at='2026-10-06T12:00:00Z', completed_by=(select owner_id from porchlight.items where id=$1) where item_id=$1 and seq=0").bind(split.tail_id).execute(&pool).await.unwrap();
    let before: Value = sqlx::query_scalar("select jsonb_agg(to_jsonb(o) order by o.id) from porchlight.item_occurrences o where item_id=any($1)").bind(vec![id, split.tail_id]).fetch_one(&pool).await.unwrap();
    let head_version: i32 = sqlx::query_scalar("select version from porchlight.items where id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let (status, response) = call(
        &s,
        "POST",
        &format!("/v1/admin/items/{id}/revert"),
        json!({"revision_id":revision}),
        Some(head_version.into()),
    )
    .await;
    let after: Value = sqlx::query_scalar("select jsonb_agg(to_jsonb(o) order by o.id) from porchlight.item_occurrences o where item_id=any($1)").bind(vec![id, split.tail_id]).fetch_one(&pool).await.unwrap();
    assert_eq!(
        status,
        422,
        "pre-split restore must reject: occurrence count {} -> {}; response {response}",
        before.as_array().unwrap().len(),
        after.as_array().unwrap().len()
    );
    assert_eq!(
        after, before,
        "tail identities/completion and head rows must remain untouched"
    );
    let version_after: i32 = sqlx::query_scalar("select version from porchlight.items where id=$1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(version_after, head_version);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn snapshot_override_modes_shapes_and_inverse_window(pool: PgPool) {
    let s = server(&pool).await;
    for mode in ["fixed", "floating"] {
        for kind in ["deadline", "appointment", "todo"] {
            let mut fields =
                json!({"title":format!("{mode} {kind}"), "kind":kind, "time_mode":mode});
            if mode == "fixed" {
                fields["tzid"] = json!("America/New_York");
            }
            match (kind, mode) {
                ("todo", _) => fields["due_date"] = json!("2026-10-07"),
                ("deadline", "fixed") => fields["due_at"] = json!("2026-10-07T12:00:00Z"),
                ("deadline", _) => fields["local_due"] = json!("2026-10-07T12:00:00"),
                ("appointment", "fixed") => {
                    fields["starts_at"] = json!("2026-10-07T12:00:00Z");
                    fields["ends_at"] = json!("2026-10-07T13:00:00Z");
                }
                _ => {
                    fields["local_start"] = json!("2026-10-07T12:00:00");
                    fields["local_end"] = json!("2026-10-07T13:00:00");
                }
            }
            let (status, item) = call(&s, "POST", "/v1/items", fields, None).await;
            assert_eq!(status, 201, "{mode} {kind}: {item}");
            let id: Uuid = item["id"].as_str().unwrap().parse().unwrap();
            let occ: Uuid =
                sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=$1")
                    .bind(id)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            for (date, included) in [
                ("2026-01-01", false),
                ("2026-10-08", true),
                ("2029-10-08", false),
            ] {
                let overrides = match kind {
                    "todo" => json!({"override_due_date":date}),
                    "deadline" => json!({"override_local_due":format!("{date}T12:00:00")}),
                    _ => {
                        json!({"override_local_start":format!("{date}T12:00:00"),"override_local_end":format!("{date}T13:00:00")})
                    }
                };
                sqlx::query("update porchlight.item_occurrences o set (override_local_start,override_local_end,override_local_due,override_due_date)=(r.override_local_start,r.override_local_end,r.override_local_due,r.override_due_date) from jsonb_populate_record(null::porchlight.item_occurrences,$2) r where o.id=$1").bind(occ).bind(overrides).execute(&pool).await.unwrap();
                let (status, snapshot) = call(&s, "GET", "/v1/snapshot", json!({}), None).await;
                assert_eq!(status, 200, "{snapshot}");
                assert_eq!(
                    snapshot["occurrences"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|o| o["id"] == occ.to_string()),
                    included,
                    "{mode} {kind} moved to {date}"
                );
            }
        }
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn revert_metadata_on_split_head_and_deleted_tail_guard(pool: PgPool) {
    let s = server(&pool).await;
    let (status, item) = call(&s, "POST", "/v1/items", json!({
        "title":"Original", "kind":"deadline", "time_mode":"fixed", "tzid":"UTC",
        "due_at":"2026-10-07T12:00:00.5Z", "rrule":"FREQ=DAILY;COUNT=5", "recurrence_mode":"calendar"
    }), None).await;
    assert_eq!(status, 201, "{item}");
    let id: Uuid = item["id"].as_str().unwrap().parse().unwrap();
    let pool_ref = &pool;
    let revision = |id| async move {
        sqlx::query_scalar::<_, Uuid>(
            "select id from porchlight.item_revisions where item_id=$1 order by rev desc limit 1",
        )
        .bind(id)
        .fetch_one(pool_ref)
        .await
        .unwrap()
    };
    let pre_split = revision(id).await;
    let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    let head = derive::load_item(&mut tx, id).await.unwrap();
    let split = items::split(
        &mut tx,
        s.init.household_id,
        id,
        head.version,
        2,
        "2026-10-06T12:00:00Z".parse().unwrap(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    // Raw-SQL anchors may contain fractional seconds even if the API's calendar
    // expansion normalizes them. PostgreSQL JSON and chrono use different text.
    sqlx::query(
        "update porchlight.items set nominal_local_anchor='2026-10-07T12:00:00.5' where id=$1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    let post_split = revision(id).await;
    let version: i32 = sqlx::query_scalar(
        "update porchlight.items set title='Changed' where id=$1 returning version",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (status, restored) = call(
        &s,
        "POST",
        &format!("/v1/admin/items/{id}/revert"),
        json!({"revision_id":post_split}),
        Some(version.into()),
    )
    .await;
    assert_eq!(
        status, 200,
        "same timing metadata revert must remain supported: {restored}"
    );
    assert_eq!(restored["title"], "Original");
    assert_eq!(restored["rrule"], "FREQ=DAILY;COUNT=2");
    sqlx::query("update porchlight.items set deleted_at='2026-10-06T12:00:00Z' where id=$1")
        .bind(split.tail_id)
        .execute(&pool)
        .await
        .unwrap();
    let (status, rejected) = call(
        &s,
        "POST",
        &format!("/v1/admin/items/{id}/revert"),
        json!({"revision_id":pre_split}),
        restored["version"].as_i64(),
    )
    .await;
    assert_eq!(
        status, 422,
        "soft-deleted tails still own their authoritative state: {rejected}"
    );
    let count: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.item_occurrences where item_id=any($1)",
    )
    .bind(vec![id, split.tail_id])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 5);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn clock_refresh_agrees_with_persisted_carry_identity(pool: PgPool) {
    use porchlight_core::items as core;
    let s = server(&pool).await;
    let start = "2026-01-01T13:00:00Z".parse().unwrap();
    let later = "2026-01-04T13:00:00Z".parse().unwrap();
    let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    let fields = items::ItemWrite {
        title: "Refresh carry".into(),
        owner_id: s.init.admin_member_id,
        timing: core::SeriesInput {
            kind: core::Kind::Deadline,
            tzid: Some("UTC".into()),
            due_at: Some("2026-01-01T12:00:00Z".parse().unwrap()),
            rrule: Some("FREQ=DAILY;COUNT=3".into()),
            recurrence_mode: Some(core::RecurrenceMode::Calendar),
            missed_policy: core::MissedPolicy::CarryOver,
            ..Default::default()
        },
        ..Default::default()
    };
    let out = items::create(
        &mut tx,
        s.init.household_id,
        &fields,
        &Default::default(),
        start,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    porchlight_api::scheduler::refresh_dispositions(&s.state.pool, later)
        .await
        .unwrap();
    let pending = |id| {
        let pool = &pool;
        async move {
            sqlx::query_scalar::<_, bool>(
                "select applied_gen<requested_gen from porchlight.item_derivation where item_id=$1",
            )
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
        }
    };
    assert!(
        pending(out.item_id).await,
        "newly missed peers enqueue a refresh"
    );
    let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    derive::derive_item(&mut tx, out.item_id, later)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    porchlight_api::scheduler::refresh_dispositions(&s.state.pool, later)
        .await
        .unwrap();
    assert!(
        !pending(out.item_id).await,
        "refresh must not re-elect a different carried row"
    );
    let states: Vec<String> = sqlx::query_scalar(
        "select disposition from porchlight.item_occurrences where item_id=$1 order by seq",
    )
    .bind(out.item_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(states, ["overdue", "expired", "expired"]);

    let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    sqlx::query("update porchlight.item_occurrences set override_local_due='2026-01-06T12:00:00' where item_id=$1 and seq=0")
        .bind(out.item_id).execute(&mut *tx).await.unwrap();
    derive::derive_item(&mut tx, out.item_id, later)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    porchlight_api::scheduler::refresh_dispositions(&s.state.pool, later)
        .await
        .unwrap();
    assert!(
        !pending(out.item_id).await,
        "refresh must not resurrect peers of moved work"
    );
    let passed = "2026-01-07T13:00:00Z".parse().unwrap();
    porchlight_api::scheduler::refresh_dispositions(&s.state.pool, passed)
        .await
        .unwrap();
    assert!(
        pending(out.item_id).await,
        "new boundary must enqueue moved work"
    );
    let mut tx = begin_as(&s.state.pool, &Actor::system()).await.unwrap();
    derive::derive_item(&mut tx, out.item_id, passed)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    porchlight_api::scheduler::refresh_dispositions(&s.state.pool, passed)
        .await
        .unwrap();
    assert!(!pending(out.item_id).await);
    let states: Vec<String> = sqlx::query_scalar(
        "select disposition from porchlight.item_occurrences where item_id=$1 order by seq",
    )
    .bind(out.item_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(states, ["overdue", "expired", "expired"]);
}

#[sqlx::test(migrations = "../db/migrations")]
async fn revert_skip_to_carry_over_restores_one_overdue(pool: PgPool) {
    let s = server(&pool).await;
    let (status, item) = call(&s, "POST", "/v1/items", json!({
        "title":"Policy undo", "kind":"deadline", "time_mode":"fixed", "tzid":"UTC",
        "due_at":"2026-10-01T12:00:00Z", "rrule":"FREQ=DAILY;COUNT=3", "recurrence_mode":"calendar", "missed_policy":"carry_over"
    }), None).await;
    assert_eq!(status, 201, "{item}");
    let id: Uuid = item["id"].as_str().unwrap().parse().unwrap();
    let revision: Uuid = sqlx::query_scalar(
        "select id from porchlight.item_revisions where item_id=$1 order by rev desc limit 1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (status, item) = call(
        &s,
        "PATCH",
        &format!("/v1/items/{id}"),
        json!({"missed_policy":"skip"}),
        item["version"].as_i64(),
    )
    .await;
    assert_eq!(status, 200, "{item}");
    let (status, item) = call(
        &s,
        "POST",
        &format!("/v1/admin/items/{id}/revert"),
        json!({"revision_id":revision}),
        item["version"].as_i64(),
    )
    .await;
    assert_eq!(status, 200, "{item}");
    let overdue: i64 = sqlx::query_scalar("select count(*) from porchlight.item_occurrences where item_id=$1 and disposition='overdue' and status in ('todo','doing','backlog')").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(
        overdue, 1,
        "policy revert must not leave the backlog hidden"
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn snapshot_poisoned_time_is_row_local_not_household_failure(pool: PgPool) {
    let s = server(&pool).await;
    let (status, item) = call(&s, "POST", "/v1/items", json!({"title":"Raw infinity", "kind":"deadline", "tzid":"UTC", "time_mode":"fixed", "due_at":"2026-10-07T12:00:00Z"}), None).await;
    assert_eq!(status, 201, "{item}");
    let id: Uuid = item["id"].as_str().unwrap().parse().unwrap();
    sqlx::query("update porchlight.item_occurrences set due_at='infinity' where item_id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    for poison_item in [false, true] {
        if poison_item {
            sqlx::query("update porchlight.items set due_at='infinity' where id=$1")
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
        let (status, snapshot) = call(&s, "GET", "/v1/snapshot", json!({}), None).await;
        assert_eq!(status, 200, "poison_item={poison_item}: {snapshot}");
        assert!(
            snapshot["occurrences"]
                .as_array()
                .unwrap()
                .iter()
                .any(|o| o["item_id"] == id.to_string())
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn snapshot_reminder_at_horizon_requires_cached_occurrence(pool: PgPool) {
    let s = server(&pool).await;
    let (status, item) = call(&s, "POST", "/v1/items", json!({"title":"Outside window reminder", "kind":"deadline", "tzid":"UTC", "time_mode":"fixed", "due_at":"2028-04-06T13:00:00Z", "reminder_offsets":[120]}), None).await;
    assert_eq!(status, 201, "{item}");
    let id: Uuid = item["id"].as_str().unwrap().parse().unwrap();
    let reminders: i64 =
        sqlx::query_scalar("select count(*) from porchlight.scheduled_reminders where item_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(reminders, 1, "fixture needs a reminder inside the window");
    let (status, snapshot) = call(&s, "GET", "/v1/snapshot", json!({}), None).await;
    assert_eq!(status, 200, "{snapshot}");
    assert!(
        snapshot["occurrences"]
            .as_array()
            .unwrap()
            .iter()
            .all(|o| o["item_id"] != id.to_string())
    );
    assert!(
        snapshot["scheduled_reminders"]
            .as_array()
            .unwrap()
            .is_empty(),
        "reminder must not reference an excluded occurrence"
    );
}
