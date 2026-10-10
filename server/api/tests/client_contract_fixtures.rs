//! Offline recordings through the production M2 handlers and problem serializer.
use axum::{Extension, body::to_bytes, response::IntoResponse};
use porchlight_api::{
    error::{ApiError, Problem},
    server::{ConnInfo, Transport},
    testing::TestServer,
};
use porchlight_core::auth::session::Platform;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

fn id(n: u8) -> Value {
    json!(format!("01999999-0000-7000-8000-{n:012}"))
}
fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../client/contract-check/src/jvmTest/resources/fixtures")
        .join(format!("{name}.json"));
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn real_me_and_devices_handlers_equal_recorded_client_fixtures(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let phone = s
        .session_device(s.init.admin_member_id, Platform::Android)
        .await;
    let old = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    sqlx::query(
        "UPDATE porchlight.household_members SET display_name='Alex',role='user' WHERE id=$1",
    )
    .bind(s.init.admin_member_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE porchlight.users SET username='alex' WHERE id=(SELECT user_id FROM porchlight.household_members WHERE id=$1)")
        .bind(s.init.admin_member_id).execute(&pool).await.unwrap();
    for (device, label, revoked, seen) in [
        (phone.device_id, "Phone", None, None),
        (
            old.device_id,
            "Old laptop",
            Some("2026-10-04T12:00:00Z"),
            Some("2026-10-03T12:00:00Z"),
        ),
    ] {
        sqlx::query("UPDATE porchlight.devices SET label=$2,revoked_at=$3::text::timestamptz,last_seen_at=$4::text::timestamptz WHERE id=$1")
            .bind(device).bind(label).bind(revoked).bind(seen)
            .execute(&pool).await.unwrap();
    }
    let router = porchlight_api::routes::mtls_router(s.state.clone()).layer(Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: Some("192.0.2.10:12345".parse().unwrap()),
        client_cert_fingerprint: Some(phone.fingerprint),
    }));
    for name in ["me", "devices"] {
        let response = router
            .clone()
            .oneshot(phone.request("GET", &format!("/v1/{name}"), json!({}), None))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
        let mut actual: Value = serde_json::from_slice(&bytes).unwrap();
        // Only volatile identity/timing values are normalized. Optional-key presence is not masked.
        if name == "me" {
            actual["member_id"] = id(1);
            actual["user_id"] = id(2);
            actual["household_id"] = id(3);
            actual["device"]["id"] = id(4);
            actual["session_expires_at"] = json!("2027-04-01T00:00:00Z");
        } else {
            for key in ["devices", "items"] {
                let rows = actual[key].as_array_mut().unwrap();
                for row in rows.iter_mut() {
                    let raw = Uuid::parse_str(row["id"].as_str().unwrap()).unwrap();
                    row["id"] = if raw == phone.device_id {
                        id(4)
                    } else {
                        assert_eq!(raw, old.device_id);
                        id(5)
                    };
                    if raw == phone.device_id {
                        assert!(row["certificate_expires_at"].is_string());
                        row["certificate_expires_at"] = json!("2027-10-05T00:00:00Z");
                    } else {
                        assert!(
                            row.get("certificate_expires_at")
                                .is_some_and(Value::is_null)
                        );
                    }
                    row["member_id"] = id(1);
                    row["created_at"] = json!(if raw == phone.device_id {
                        "2026-10-05T00:00:00Z"
                    } else {
                        "2026-10-01T00:00:00Z"
                    });
                }
                rows.sort_by_key(|row| row["id"].as_str().unwrap().to_owned());
            }
        }
        if std::env::var_os("PORCHLIGHT_RECORD_CLIENT_FIXTURES").is_some() {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../client/contract-check/src/jvmTest/resources/fixtures")
                .join(format!("{name}.json"));
            std::fs::write(path, serde_json::to_vec_pretty(&actual).unwrap()).unwrap();
        }
        assert_eq!(actual, fixture(name), "{name} fixture drift");
    }
    s.stop().await;
}

#[tokio::test]
async fn real_426_problem_equals_recorded_client_fixture() {
    let response = ApiError::new(Problem::UpgradeRequired)
        .extension("min_client_version", json!("0.2.0"))
        .into_response();
    assert_eq!(response.status(), 426);
    assert_eq!(
        response.headers()["content-type"],
        "application/problem+json"
    );
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        fixture("problem")
    );
}
