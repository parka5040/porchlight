use axum::{Extension, body::to_bytes};
use porchlight_api::{
    routes,
    server::{ConnInfo, Transport},
    testing::{TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;

fn normalize(value: &mut Value, ids: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if matches!(
                    key.as_str(),
                    "created_at" | "updated_at" | "derived_at" | "checked_at"
                ) && !value.is_null()
                {
                    *value = json!("2026-10-10T12:05:00Z");
                } else if key == "cursor" {
                    *value = json!(0);
                } else {
                    normalize(value, ids);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                normalize(value, ids);
            }
        }
        Value::String(text) if uuid::Uuid::parse_str(text).is_ok() => {
            let index = ids.iter().position(|id| id == text).unwrap_or_else(|| {
                ids.push(text.clone());
                ids.len() - 1
            });
            *text = format!("01999999-0000-7000-8000-{:012}", index + 1);
        }
        _ => (),
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn recipient_disposition_wire_covers_both_zone_orderings(pool: PgPool) {
    let now = "2026-10-10T12:05:00Z".parse().unwrap();
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: Arc::new(move || now),
            ..Default::default()
        },
    )
    .await;
    let member = s.init.admin_member_id;
    let user: uuid::Uuid = sqlx::query_scalar("insert into porchlight.users(household_id,username) values($1,'calendar-owner') returning id").bind(s.init.household_id).fetch_one(&pool).await.unwrap();
    let owner: uuid::Uuid = sqlx::query_scalar("insert into porchlight.household_members(household_id,user_id,role,display_name,color,tzid) values($1,$2,'user','Sam Rivera','#8EC07C','America/New_York') returning id").bind(s.init.household_id).bind(user).fetch_one(&pool).await.unwrap();
    let item: uuid::Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,time_mode,local_due,missed_policy,recurrence_mode,rrule) values($1,$2,'Recipient work','deadline','floating','2026-10-10 08:00','skip','calendar','FREQ=DAILY;COUNT=1') returning id").bind(s.init.household_id).bind(owner).fetch_one(&pool).await.unwrap();
    sqlx::query(
        "insert into porchlight.item_assignees(household_id,item_id,member_id) values($1,$2,$3)",
    )
    .bind(s.init.household_id)
    .bind(item)
    .bind(member)
    .execute(&pool)
    .await
    .unwrap();
    let device = s.session_device(member, Platform::Desktop).await;
    let router = routes::mtls_router(s.state.clone()).layer(Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(device.fingerprint),
    }));
    for (name, recipient_zone, owner_zone, global) in [
        (
            "later",
            "America/Los_Angeles",
            "America/New_York",
            "expired",
        ),
        ("earlier", "America/New_York", "America/Los_Angeles", "open"),
    ] {
        sqlx::query("update porchlight.household_members set display_name='Sam Rivera',tzid=case when id=$1 then $3 else $4 end,tz_source='manual' where id in ($1,$2)").bind(member).bind(owner).bind(recipient_zone).bind(owner_zone).execute(&pool).await.unwrap();
        let mut tx =
            porchlight_db::tx::begin_as(&s.state.pool, &porchlight_db::tx::Actor::system())
                .await
                .unwrap();
        sqlx::query("select id from porchlight.items where id=$1 for update")
            .bind(item)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        porchlight_db::derive::derive_item(&mut tx, item, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let mut recorded = json!({"member":member,"zone":recipient_zone,"now":now});
        for (key, path) in [
            (
                "agenda",
                "/v1/agenda?from=2026-10-10T00:00:00Z&to=2026-10-11T00:00:00Z",
            ),
            ("snapshot", "/v1/snapshot"),
        ] {
            let response = router
                .clone()
                .oneshot(device.request("GET", path, json!({}), None))
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            recorded[key] =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
        }
        assert_eq!(
            recorded["snapshot"]["occurrences"][0]["disposition"],
            global
        );
        let agenda = recorded["agenda"]["items"].as_array().unwrap();
        if name == "later" {
            assert_eq!(agenda.len(), 1);
            assert_eq!(
                agenda[0]["agenda"]["disposition"], "open",
                "recipient agenda must expose local disposition"
            );
        } else {
            assert!(agenda.is_empty());
        }
        normalize(&mut recorded, &mut Vec::new());
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../client/shared/src/jvmTest/resources/item-fixtures/audit-today-{name}.json"
        ));
        if std::env::var_os("PORCHLIGHT_RECORD_CLIENT_FIXTURES").is_some() {
            std::fs::write(
                &path,
                format!("{}\n", serde_json::to_string_pretty(&recorded).unwrap()),
            )
            .unwrap();
        }
        let expected: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(recorded, expected, "recipient Today fixture drift");
    }
}
