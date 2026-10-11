//! C2b offline recordings through real item/read handlers. Only volatile identities/timing are normalized.
use axum::{Extension, body::to_bytes};
use porchlight_api::{
    routes,
    server::{ConnInfo, Transport},
    testing::{TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

fn normalized_id(n: usize) -> String {
    format!("01999999-0000-7000-8000-{n:012}")
}
fn normalize(value: &mut Value, identities: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                if matches!(
                    key.as_str(),
                    "created_at" | "updated_at" | "derived_at" | "checked_at"
                ) && !value.is_null()
                {
                    *value = json!("2026-10-06T12:00:00Z");
                } else {
                    normalize(value, identities);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(|v| normalize(v, identities)),
        Value::String(text) if uuid::Uuid::parse_str(text).is_ok() => {
            let index = identities
                .iter()
                .position(|id| id == text)
                .unwrap_or_else(|| {
                    identities.push(text.clone());
                    identities.len() - 1
                });
            *text = normalized_id(index + 1);
        }
        _ => {}
    }
}
fn fixture(name: &str, actual: &Value) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../client/shared/src/jvmTest/resources/item-fixtures")
        .join(format!("{name}.json"));
    if std::env::var_os("PORCHLIGHT_RECORD_CLIENT_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string_pretty(actual).unwrap()),
        )
        .unwrap();
    }
    let recorded: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let mut recorded = recorded;
    let mut actual = actual.clone();
    if name == "snapshot" {
        // Publication watermark is nondeterministic; retain/type-check the real recording,
        // and compare every other field without normalizing its presence or shape.
        for value in [&mut recorded, &mut actual] {
            assert!(
                value["cursor"].as_u64().is_some(),
                "snapshot cursor must be a non-negative integer"
            );
            value["cursor"] = Value::Null;
        }
    }
    assert_eq!(recorded, actual, "{name} client fixture drift");
}
#[sqlx::test(migrations = "../db/migrations")]
async fn real_item_agenda_board_snapshot_match_client_recordings(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-06T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let device = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    // Explicit profile fields prevent host zone/seed colour variation in a client contract recording.
    sqlx::query("UPDATE porchlight.household_members SET display_name='Alex',color='#ABCDEF',tz_source='manual',tzid='UTC' WHERE id=$1")
        .bind(s.init.admin_member_id).execute(&pool).await.unwrap();
    let router = routes::mtls_router(s.state.clone()).layer(Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(device.fingerprint),
    }));
    let response = router.clone().oneshot(device.request("POST", "/v1/items", json!({
        "title":"Recorded task", "kind":"deadline", "time_mode":"fixed", "tzid":"UTC", "due_at":"2026-10-06T13:00:00Z",
        "assignees":[s.init.admin_member_id], "reminder_offsets":[15]
    }), Some("c2b-create-fixture"))).await.unwrap();
    let status = response.status();
    let created: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(status, 201, "fixture create: {}", created);
    let item = created["id"].as_str().unwrap();
    // S7b snapshot sections: one real comment and one real attachment, so the client decodes non-empty rows.
    let response = router
        .clone()
        .oneshot(device.request(
            "POST",
            &format!("/v1/items/{item}/comments"),
            json!({"body":"Recorded comment"}),
            Some("c2b-comment-fixture"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), 201, "fixture comment");
    let body = b"--c2b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"notes.txt\"\r\nContent-Type: text/plain\r\n\r\nrecorded attachment\r\n--c2b--\r\n".to_vec();
    let mut upload = device
        .request(
            "POST",
            &format!("/v1/items/{item}/attachments"),
            json!({}),
            Some("c2b-attachment-fixture"),
        )
        .map(|_| http_body_util::Full::new(hyper::body::Bytes::from(body)));
    upload.headers_mut().insert(
        "content-type",
        "multipart/form-data; boundary=c2b".parse().unwrap(),
    );
    let response = router.clone().oneshot(upload).await.unwrap();
    let status = response.status();
    let attachment = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    assert_eq!(
        status,
        201,
        "fixture attachment: {}",
        String::from_utf8_lossy(&attachment)
    );
    // C5: standalone list templates, ordered entries and Got it are real route shapes too.
    let response = router
        .clone()
        .oneshot(device.request(
            "POST",
            "/v1/lists",
            json!({"name":"Groceries","mode":"persistent","visibility":"household"}),
            Some("c5-list-fixture"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let list: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    let list_id = list["id"].as_str().unwrap();
    for (label, key) in [("Oat milk", "c5-entry-one"), ("Bread", "c5-entry-two")] {
        let response = router
            .clone()
            .oneshot(device.request(
                "POST",
                &format!("/v1/lists/{list_id}/entries"),
                json!({"label":label}),
                Some(key),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
        if label == "Bread" {
            let entry: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            let mut request = device.request(
                "POST",
                &format!("/v1/list-entries/{}/check", entry["id"].as_str().unwrap()),
                json!({}),
                Some("c5-check-fixture"),
            );
            request.headers_mut().insert(
                "If-Match",
                entry["version"]
                    .as_i64()
                    .unwrap()
                    .to_string()
                    .parse()
                    .unwrap(),
            );
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), 200);
        }
    }
    let mut identities = vec![
        s.init.household_id.to_string(),
        s.init.admin_member_id.to_string(),
        item.to_owned(),
    ];
    for (name, path, row) in [
        ("item", format!("/v1/items/{item}"), false),
        (
            "agenda",
            "/v1/agenda?from=2026-10-01T00:00:00Z&to=2026-11-01T00:00:00Z".into(),
            true,
        ),
        ("board", "/v1/board?scope=household".into(), true),
        ("list", format!("/v1/lists/{list_id}"), false),
        ("snapshot", "/v1/snapshot".into(), false),
    ] {
        let response = router
            .clone()
            .oneshot(device.request("GET", &path, json!({}), None))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let mut actual: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        if row {
            let rows = actual["items"].as_array().unwrap();
            assert_eq!(rows.len(), 1);
            actual = rows[0].clone();
        }
        normalize(&mut actual, &mut identities);
        fixture(name, &actual);
        if name == "snapshot" {
            fixture("comment", &actual["comments"][0]);
            fixture("attachment", &actual["attachments"][0]);
        }
    }
    s.stop().await;
}

#[test]
fn normalizer_preserves_null_check_state() {
    let mut row = json!({"checked_at":null,"checked_by":null,"updated_at":"volatile"});
    normalize(&mut row, &mut vec![]);
    assert!(row["checked_at"].is_null());
    assert!(row["checked_by"].is_null());
    assert_eq!(row["updated_at"], "2026-10-06T12:00:00Z");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn real_occurrence_list_check_uncheck_recordings(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-06T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let device = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let (_, assignee) =
        porchlight_api::testing::trust::user_named(&pool, &s, "assignee", "Assignee").await;
    let assignee_device = s.session_device(assignee, Platform::Desktop).await;
    let assignee_router = routes::mtls_router(s.state.clone()).layer(Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(assignee_device.fingerprint),
    }));
    let router = routes::mtls_router(s.state.clone()).layer(Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(device.fingerprint),
    }));
    let response = router.clone().oneshot(device.request("POST", "/v1/items", json!({
        "title":"Daily groceries", "kind":"deadline", "time_mode":"fixed", "tzid":"UTC",
        "due_at":"2026-10-06T13:00:00Z", "rrule":"FREQ=DAILY;COUNT=3", "recurrence_mode":"calendar", "assignees":[assignee]
    }), Some("c5-occurrence-item"))).await.unwrap();
    assert_eq!(response.status(), 201);
    let item: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    let item_id = item["id"].as_str().unwrap();
    let response = router
        .clone()
        .oneshot(device.request(
            "GET",
            &format!("/v1/items/{item_id}/occurrences"),
            json!({}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let occurrences: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    let occurrence_id = occurrences["items"][0]["id"].as_str().unwrap();
    let other_id = occurrences["items"][1]["id"].as_str().unwrap();
    let mut identities = vec![
        s.init.household_id.to_string(),
        s.init.admin_member_id.to_string(),
        item_id.to_owned(),
    ];
    for mode in ["persistent", "per_occurrence"] {
        let response = router
            .clone()
            .oneshot(device.request(
                "POST",
                "/v1/lists",
                json!({
                    "name":"Daily groceries", "mode":mode, "item_id":item_id
                }),
                Some(&format!("c5-attached-{mode}")),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), 201);
        let list: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        let list_id = list["id"].as_str().unwrap();
        let mut first = Value::Null;
        for (i, label) in ["Oat milk", "Bread"].iter().enumerate() {
            let response = router
                .clone()
                .oneshot(device.request(
                    "POST",
                    &format!("/v1/lists/{list_id}/entries"),
                    json!({"label":label}),
                    Some(&format!("c5-{mode}-entry-{i}")),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), 201);
            let entry: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            if i == 0 {
                first = entry;
            }
        }
        if mode == "persistent" {
            let response = router
                .clone()
                .oneshot(device.request("GET", &format!("/v1/lists/{list_id}"), json!({}), None))
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let mut actual: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            normalize(&mut actual, &mut identities);
            fixture("list-attached-persistent", &actual);
            let mut request = device.request(
                "POST",
                &format!("/v1/list-entries/{}/check", first["id"].as_str().unwrap()),
                json!({}),
                Some("c5-persistent-check-only"),
            );
            request.headers_mut().insert(
                "If-Match",
                first["version"]
                    .as_i64()
                    .unwrap()
                    .to_string()
                    .parse()
                    .unwrap(),
            );
            assert_eq!(router.clone().oneshot(request).await.unwrap().status(), 200);
            let response = assignee_router
                .clone()
                .oneshot(assignee_device.request(
                    "GET",
                    &format!("/v1/lists/{list_id}"),
                    json!({}),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let mut actual: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(actual["capabilities"], json!(["view", "check"]));
            normalize(&mut actual, &mut identities);
            fixture("list-persistent-assignee", &actual);
            continue;
        }
        let entry_id = first["id"].as_str().unwrap().to_owned();
        for action in ["check", "uncheck"] {
            let mut request = device.request(
                "POST",
                &format!("/v1/list-entries/{entry_id}/{action}"),
                json!({"occurrence_id":occurrence_id}),
                Some(&format!("c5-occurrence-{action}")),
            );
            request.headers_mut().insert(
                "If-Match",
                first["version"]
                    .as_i64()
                    .unwrap()
                    .to_string()
                    .parse()
                    .unwrap(),
            );
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), 200);
            first =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            let mut actual = first.clone();
            normalize(&mut actual, &mut identities);
            fixture(&format!("entry-occurrence-{action}"), &actual);
            let response = router
                .clone()
                .oneshot(device.request(
                    "GET",
                    &format!("/v1/lists/{list_id}?occurrence_id={occurrence_id}"),
                    json!({}),
                    None,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let mut actual: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            normalize(&mut actual, &mut identities);
            fixture(&format!("list-occurrence-{action}"), &actual);
            if action == "check" {
                let response = assignee_router
                    .clone()
                    .oneshot(assignee_device.request(
                        "GET",
                        &format!("/v1/lists/{list_id}?occurrence_id={occurrence_id}"),
                        json!({}),
                        None,
                    ))
                    .await
                    .unwrap();
                assert_eq!(response.status(), 200);
                let mut actual: Value = serde_json::from_slice(
                    &to_bytes(response.into_body(), 1024 * 1024).await.unwrap(),
                )
                .unwrap();
                assert_eq!(actual["capabilities"], json!(["view", "check"]));
                normalize(&mut actual, &mut identities);
                fixture("list-occurrence-assignee", &actual);
                // A regenerated-away occurrence is a body reference error, not a missing list.
                let missing = uuid::Uuid::now_v7();
                let response = router
                    .clone()
                    .oneshot(device.request(
                        "GET",
                        &format!("/v1/lists/{list_id}?occurrence_id={missing}"),
                        json!({}),
                        None,
                    ))
                    .await
                    .unwrap();
                assert_eq!(response.status(), 422);
                let actual: Value = serde_json::from_slice(
                    &to_bytes(response.into_body(), 1024 * 1024).await.unwrap(),
                )
                .unwrap();
                assert_eq!(actual["errors"][0]["field"], "occurrence_id");
                assert_eq!(actual["errors"][0]["code"], "invalid-reference");
                fixture("list-occurrence-invalid-reference", &actual);
                for (name, path) in [
                    ("list-occurrence-template", format!("/v1/lists/{list_id}")),
                    (
                        "list-occurrence-other",
                        format!("/v1/lists/{list_id}?occurrence_id={other_id}"),
                    ),
                ] {
                    let response = router
                        .clone()
                        .oneshot(device.request("GET", &path, json!({}), None))
                        .await
                        .unwrap();
                    assert_eq!(response.status(), 200);
                    let mut actual: Value = serde_json::from_slice(
                        &to_bytes(response.into_body(), 1024 * 1024).await.unwrap(),
                    )
                    .unwrap();
                    normalize(&mut actual, &mut identities);
                    fixture(name, &actual);
                }
            }
        }
    }
    s.stop().await;
}
