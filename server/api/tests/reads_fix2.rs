use porchlight_api::{
    routes,
    server::{ConnInfo, Transport},
    testing::{TestClient, TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use tower::ServiceExt;

#[sqlx::test(migrations = "../db/migrations")]
async fn item_writes_store_canonical_zones(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(|| "2026-10-05T12:00:00Z".parse().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    for (zone, canonical) in [
        ("GMT", "UTC"),
        ("Etc/UTC", "UTC"),
        ("US/Eastern", "America/New_York"),
    ] {
        let response=client.request(s.lan,d.request("POST","/v1/items",json!({"title":"zone","kind":"deadline","time_mode":"fixed","tzid":zone,"due_at":"2026-10-06T12:00:00Z"}),Some(zone))).await.unwrap();
        assert_eq!(
            response.status,
            201,
            "{zone}: {}",
            String::from_utf8_lossy(&response.body)
        );
        assert_eq!(response.json()["tzid"], canonical);
        let stored: String = sqlx::query_scalar("select tzid from porchlight.items where id=$1")
            .bind(
                response.json()["id"]
                    .as_str()
                    .unwrap()
                    .parse::<uuid::Uuid>()
                    .unwrap(),
            )
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, canonical);
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn zone_hint_is_canonical_or_ignored(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let r = routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(d.fingerprint),
    }));
    for (zone, expected) in [
        ("US/Eastern", "America/New_York"),
        ("Asia/Calcutta", "Asia/Kolkata"),
        ("CET", "Europe/Brussels"),
        ("GMT", "UTC"),
        ("Mars/Olympus_Mons", "UTC"),
    ] {
        let mut req = d.request("GET", "/v1/me", json!({}), None);
        req.headers_mut()
            .insert("porchlight-tz", zone.parse().unwrap());
        assert_eq!(r.clone().oneshot(req).await.unwrap().status(), 200);
        let stored: String =
            sqlx::query_scalar("select tzid from porchlight.household_members where id=$1")
                .bind(s.init.admin_member_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, expected);
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn member_pending_is_caller_only_in_list_and_snapshot(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let user:uuid::Uuid=sqlx::query_scalar("insert into porchlight.users(household_id,username) values($1,'pending-other') returning id").bind(s.init.household_id).fetch_one(&pool).await.unwrap();
    let other:uuid::Uuid=sqlx::query_scalar("insert into porchlight.household_members(household_id,user_id,display_name,role,color,tzid) values($1,$2,'Other','user','#ABCDEF','UTC') returning id").bind(s.init.household_id).bind(user).fetch_one(&pool).await.unwrap();
    for owner in [other, s.init.admin_member_id] {
        sqlx::query("insert into porchlight.items(household_id,owner_id,title,kind,visibility) values($1,$2,'Pending private','todo','private')").bind(s.init.household_id).bind(owner).execute(&pool).await.unwrap();
    }
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let r = routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(d.fingerprint),
    }));
    for path in ["/v1/members", "/v1/snapshot"] {
        let response = r
            .clone()
            .oneshot(d.request("GET", path, json!({}), None))
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        let members = body[if path == "/v1/members" {
            "items"
        } else {
            "members"
        }]
        .as_array()
        .unwrap();
        assert_eq!(
            members
                .iter()
                .find(|m| m["id"] == other.to_string())
                .unwrap()["pending_derivation"],
            false
        );
        assert_eq!(
            members
                .iter()
                .find(|m| m["id"] == s.init.admin_member_id.to_string())
                .unwrap()["pending_derivation"],
            true
        );
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn dependent_date_agenda_uses_household_zone(pool: PgPool) {
    let now = "2026-10-05T12:00:00Z".parse().unwrap();
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(move || now),
            ..Default::default()
        },
    )
    .await;
    sqlx::query("update porchlight.households set tzid='America/New_York' where id=$1")
        .bind(s.init.household_id)
        .execute(&pool)
        .await
        .unwrap();
    let dependent:uuid::Uuid=sqlx::query_scalar("insert into porchlight.household_members(household_id,display_name,role,color,tzid) values($1,'Dep','dependent','#ABCDEF','Asia/Tokyo') returning id").bind(s.init.household_id).fetch_one(&pool).await.unwrap();
    for (title, date) in [("Included", "2026-10-06"), ("Excluded", "2026-10-07")] {
        let item:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,due_date,subject_member_id) values($1,$2,$3,'todo',$4,$5) returning id").bind(s.init.household_id).bind(s.init.admin_member_id).bind(title).bind(date.parse::<chrono::NaiveDate>().unwrap()).bind(dependent).fetch_one(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        porchlight_db::derive::derive_item(&mut tx, item, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let response=client.request(s.lan,d.request("GET",&format!("/v1/agenda?member_id={dependent}&from=2026-10-06T04:00:00Z&to=2026-10-07T04:00:00Z"),json!({}),None)).await.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.json()["items"].as_array().unwrap().len(), 1);
    assert_eq!(response.json()["items"][0]["title"], "Included");
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn populated_agenda_conflicts_conform_to_openapi(pool: PgPool) {
    let now = "2026-10-05T12:00:00Z".parse().unwrap();
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            item_clock: std::sync::Arc::new(move || now),
            ..Default::default()
        },
    )
    .await;
    for title in ["One", "Two"] {
        let item:uuid::Uuid=sqlx::query_scalar("insert into porchlight.items(household_id,owner_id,title,kind,time_mode,tzid,starts_at,ends_at) values($1,$2,$3,'appointment','fixed','UTC','2026-10-06T12:00:00Z','2026-10-06T13:00:00Z') returning id").bind(s.init.household_id).bind(s.init.admin_member_id).bind(title).fetch_one(&pool).await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        porchlight_db::derive::derive_item(&mut tx, item, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let response = TestClient::new(&s.ca_pem, Some(&d))
        .request(
            s.lan,
            d.request(
                "GET",
                "/v1/agenda?from=2026-10-06T00:00:00Z&to=2026-10-07T00:00:00Z",
                json!({}),
                None,
            ),
        )
        .await
        .unwrap();
    assert_eq!(response.status, 200);
    let value = response.json();
    assert_eq!(value["items"].as_array().unwrap().len(), 2);
    assert!(
        !value["items"][0]["conflicts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let doc = serde_json::to_value(porchlight_api::openapi::document()).unwrap();
    conforms(
        &doc,
        &doc["paths"]["/v1/agenda"]["get"]["responses"]["200"]["content"]["application/json"]["schema"],
        &value,
    );
    s.stop().await;
}

fn conforms(doc: &serde_json::Value, schema: &serde_json::Value, value: &serde_json::Value) {
    if let Some(reference) = schema["$ref"].as_str() {
        return conforms(
            doc,
            doc.pointer(reference.strip_prefix('#').unwrap()).unwrap(),
            value,
        );
    }
    if let Some(variants) = schema
        .get("anyOf")
        .or_else(|| schema.get("oneOf"))
        .and_then(serde_json::Value::as_array)
    {
        let chosen = variants
            .iter()
            .find(|s| s["type"] == "null" && value.is_null())
            .or_else(|| variants.iter().find(|s| s["type"] != "null"))
            .unwrap();
        return conforms(doc, chosen, value);
    }
    let types = schema["type"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| schema["type"].as_str().into_iter().collect());
    if value.is_null() {
        assert!(types.contains(&"null"), "not nullable: {schema}");
        return;
    }
    if let Some(values) = schema["enum"].as_array() {
        assert!(values.contains(value), "enum {value}");
    }
    if types.contains(&"object") {
        let object = value.as_object().expect("object");
        if let Some(required) = schema["required"].as_array() {
            for key in required {
                assert!(object.contains_key(key.as_str().unwrap()), "missing {key}");
            }
        }
        for (key, val) in object {
            let property = schema["properties"]
                .get(key)
                .unwrap_or_else(|| panic!("undocumented field {key}"));
            conforms(doc, property, val);
        }
    } else if types.contains(&"array") {
        for val in value.as_array().expect("array") {
            conforms(doc, &schema["items"], val);
        }
    } else if types.contains(&"integer") {
        assert!(value.is_i64() || value.is_u64());
    } else if types.contains(&"boolean") {
        assert!(value.is_boolean());
    } else if types.contains(&"string") {
        let text = value.as_str().expect("string");
        match schema["format"].as_str() {
            Some("uuid") => {
                text.parse::<uuid::Uuid>().unwrap();
            }
            Some("date-time") => {
                chrono::DateTime::parse_from_rfc3339(text)
                    .unwrap_or_else(|e| panic!("invalid RFC3339 {text}: {e}"));
            }
            Some("local-date-time") => {
                text.parse::<chrono::NaiveDateTime>().unwrap();
            }
            Some("date") => {
                text.parse::<chrono::NaiveDate>().unwrap();
            }
            _ => {}
        }
    } else {
        panic!("unhandled schema {schema}");
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn successful_zone_drain_replay_keeps_refreshed_response(pool: PgPool) {
    let now = "2026-10-05T12:00:00Z".parse().unwrap();
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            db_pool_size: 1,
            item_clock: std::sync::Arc::new(move || now),
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let item=client.request(s.lan,d.request("POST","/v1/items",json!({"title":"Replay drain","kind":"appointment","time_mode":"floating","local_start":"2026-10-06T09:00:00","local_end":"2026-10-06T10:00:00"}),Some("replay-drain-item"))).await.unwrap();
    assert_eq!(item.status, 201);
    let version: i32 =
        sqlx::query_scalar("select version from porchlight.household_members where id=$1")
            .bind(s.init.admin_member_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let path = format!("/v1/members/{}", s.init.admin_member_id);
    let request = || {
        let mut req = d.request(
            "PATCH",
            &path,
            json!({"tz_source":"manual","tzid":"Europe/Berlin"}),
            Some("successful-drain"),
        );
        req.headers_mut()
            .insert("if-match", version.to_string().parse().unwrap());
        req
    };
    let response = client.request(s.lan, request()).await.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.json()["pending_derivation"], false);
    let before: (i64, i64) = sqlx::query_as(
        "select requested_gen,applied_gen from porchlight.item_derivation where item_id=$1",
    )
    .bind(
        item.json()["id"]
            .as_str()
            .unwrap()
            .parse::<uuid::Uuid>()
            .unwrap(),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before.0, before.1);
    let replay = client.request(s.lan, request()).await.unwrap();
    assert_eq!(replay.status, response.status);
    assert_eq!(replay.body, response.body);
    let after: (i64, i64) = sqlx::query_as(
        "select requested_gen,applied_gen from porchlight.item_derivation where item_id=$1",
    )
    .bind(
        item.json()["id"]
            .as_str()
            .unwrap()
            .parse::<uuid::Uuid>()
            .unwrap(),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, after);
    s.stop().await;
}
