//! C6 real-handler recordings with additive read-field drift checks.
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

fn normalize(value: &mut Value, ids: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if matches!(
                    key.as_str(),
                    "created_at"
                        | "updated_at"
                        | "certificate_expires_at"
                        | "next_fetch_at"
                        | "last_seen_at"
                ) {
                    *value = json!("2026-10-06T12:00:00Z");
                } else {
                    normalize(value, ids);
                }
            }
        }
        Value::Array(rows) => rows.iter_mut().for_each(|v| normalize(v, ids)),
        Value::String(text) if uuid::Uuid::parse_str(text).is_ok() => {
            let n = ids.iter().position(|id| id == text).unwrap_or_else(|| {
                ids.push(text.clone());
                ids.len() - 1
            });
            *text = format!("01999999-0000-7000-8000-{:012}", n + 1);
        }
        _ => {}
    }
}
fn fixture(name: &str, value: &Value) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../client/shared/src/jvmTest/resources/planning-fixtures")
        .join(format!("{name}.json"));
    if std::env::var_os("PORCHLIGHT_RECORD_PLANNING_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }
    let recorded: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("Run recording command first"))
            .unwrap();
    assert_eq!(recorded, *value, "{name} planning fixture drift");
}
async fn read(response: axum::response::Response, status: u16) -> Value {
    let actual_status = response.status();
    let value = serde_json::from_slice(
        &to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(actual_status.as_u16(), status, "{value}");
    value
}
#[sqlx::test(migrations = "../db/migrations")]
async fn real_planning_handlers_match_client_recordings(pool: PgPool) {
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
    sqlx::query(
        "UPDATE porchlight.household_members SET tzid='UTC',tz_source='manual' WHERE id=$1",
    )
    .bind(s.init.admin_member_id)
    .execute(&pool)
    .await
    .unwrap();
    let router = routes::mtls_router(s.state.clone()).layer(Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(device.fingerprint),
    }));
    let mut ids = vec![s.init.admin_member_id.to_string()];
    let mut preview = read(
        router
            .clone()
            .oneshot(device.request(
                "POST",
                "/v1/imports/ics",
                json!({
                    "member_id":s.init.admin_member_id,"name":"Recorded calendar", "preview":true,
                    "content":include_str!("../../core/tests/fixtures/ics/google.ics")
                }),
                Some("c6-preview"),
            ))
            .await
            .unwrap(),
        200,
    )
    .await;
    normalize(&mut preview, &mut ids);
    fixture("preview", &preview);
    let path = format!("/v1/members/{}/availability", s.init.admin_member_id);
    let windows: Vec<Value> = (0..7)
        .map(|day| json!({"weekday":day,"start":"09:00:00","end":"17:00:00"}))
        .collect();
    let mut request = device.request(
        "PUT",
        &path,
        json!({"windows":windows}),
        Some("c6-availability"),
    );
    request
        .headers_mut()
        .insert("if-match", "0".parse().unwrap());
    let mut availability = read(router.clone().oneshot(request).await.unwrap(), 200).await;
    normalize(&mut availability, &mut ids);
    fixture("availability", &availability);
    read(router.clone().oneshot(device.request("POST", "/v1/items", json!({"title":"Recorded EDF task","kind":"todo","time_mode":"floating","due_date":"2026-10-12","est_minutes":30,"priority":1}), Some("c6-task"))).await.unwrap(), 201).await;
    let suggestions = read(
        router
            .clone()
            .oneshot(device.request(
                "POST",
                "/v1/schedule/suggest",
                json!({}),
                Some("c6-suggest"),
            ))
            .await
            .unwrap(),
        200,
    )
    .await;
    let mut suggestion = suggestions["suggestions"]
        .as_array()
        .unwrap()
        .first()
        .expect("EDF proposal")
        .clone();
    normalize(&mut suggestion, &mut ids);
    fixture("suggestion", &suggestion);
    let mut feed = read(router.clone().oneshot(device.request("POST", "/v1/feeds", json!({
        "name":"Recorded feed","url":"https://example.org/school.ics","target_member_id":s.init.admin_member_id,
        "visibility":"private","refresh_seconds":3600
    }), Some("c6-feed"))).await.unwrap(), 201).await;
    normalize(&mut feed, &mut ids);
    fixture("feed", &feed);
    let devices = read(
        router
            .oneshot(device.request("GET", "/v1/devices", json!({}), None))
            .await
            .unwrap(),
        200,
    )
    .await;
    let mut device_row = devices["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == device.device_id.to_string())
        .unwrap()
        .clone();
    normalize(&mut device_row, &mut ids);
    fixture("device", &device_row);
}
