use axum::{Extension, body::to_bytes};
use porchlight_api::{
    routes,
    server::{ConnInfo, Transport},
    testing::{TestOptions, TestServer, trust},
};
use porchlight_core::auth::session::Platform;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

#[sqlx::test(migrations = "../db/migrations")]
async fn additive_client_read_fields_preserve_existing_semantics(pool: PgPool) {
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
    let expected: chrono::DateTime<chrono::Utc> = "2030-01-01T00:00:00Z".parse().unwrap();
    sqlx::query("insert into porchlight.device_certificates(household_id,device_id,fingerprint_sha256,serial,not_before,not_after,created_at) values($1,$2,$3,'client-read-renewal','2029-01-01'::timestamptz,$4,now()+interval '1 second')")
        .bind(s.init.household_id).bind(device.device_id).bind([0x65_u8;32].as_slice()).bind(expected).execute(&pool).await.unwrap();
    let missing = uuid::Uuid::now_v7();
    let revoked = uuid::Uuid::now_v7();
    for (id, label) in [(missing, "No certificate"), (revoked, "Revoked device")] {
        sqlx::query("insert into porchlight.devices(id,household_id,member_id,label,platform) values($1,$2,$3,$4,'desktop')")
            .bind(id).bind(s.init.household_id).bind(s.init.admin_member_id).bind(label).execute(&pool).await.unwrap();
    }
    sqlx::query("insert into porchlight.device_certificates(household_id,device_id,fingerprint_sha256,serial,not_before,not_after) values($1,$2,$3,'revoked-cert','2029-01-01'::timestamptz,$4)")
        .bind(s.init.household_id).bind(revoked).bind([0x66_u8;32].as_slice()).bind(expected).execute(&pool).await.unwrap();
    sqlx::query("update porchlight.devices set revoked_at=now() where id=$1")
        .bind(revoked)
        .execute(&pool)
        .await
        .unwrap();
    let router = routes::mtls_router(s.state.clone()).layer(Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(device.fingerprint),
    }));
    let response = router
        .clone()
        .oneshot(device.request("GET", "/v1/devices", json!({}), None))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    let row = value["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == device.device_id.to_string())
        .unwrap();
    let expiry = row["certificate_expires_at"]
        .as_str()
        .expect("expiry exposed");
    assert_eq!(
        expiry.parse::<chrono::DateTime<chrono::Utc>>().unwrap(),
        expected
    );
    assert_eq!(value["items"], value["devices"]);
    for id in [missing, revoked] {
        let row = value["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id.to_string())
            .unwrap();
        assert!(row["certificate_expires_at"].is_null());
    }
    let (_, token) = trust::seed_pat(&pool, &s, &["admin"]).await;
    let admin = trust::agent(
        &s,
        Some(&token),
        "GET",
        "/v1/admin/devices",
        json!({}),
        "read",
    )
    .await;
    assert_eq!(admin.status, 200);
    let admin_value = admin.json();
    let admin_row = admin_value["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == device.device_id.to_string())
        .unwrap();
    assert_eq!(
        admin_row["certificate_expires_at"],
        row["certificate_expires_at"]
    );
    // The aggregate skipped field remains stable; invalid events and EXDATE/cancellations become distinguishable.
    let content = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:series\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nRRULE:FREQ=DAILY;COUNT=2\r\nEXDATE:20261013T090000Z\r\nSUMMARY:School\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:bad\r\nDTSTART:20261012T090000Z\r\nDTEND:20261012T100000Z\r\nSUMMARY:xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let response = router
        .oneshot(device.request(
            "POST",
            "/v1/imports/ics",
            json!({"content":content,"member_id":s.init.admin_member_id,"preview":true}),
            Some("split-summary"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let value: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(value["invalid_skipped"], 1, "{value}");
    assert_eq!(value["cancelled_skipped"], 1, "{value}");
    assert_eq!(value["skipped"], 2);
    assert!(
        value["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "InvalidEvent")
    );
}
