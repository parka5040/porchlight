use porchlight_api::testing::{HouseholdFixture, TestClient, TestOptions, trust};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::io::Read;
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn complete_member_inventory_and_household_parent_scoping(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let other_h: Uuid = sqlx::query_scalar(
        "insert into porchlight.households(name,tzid) values('Other household','UTC') returning id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let other_m: Uuid = sqlx::query_scalar("insert into porchlight.household_members(household_id,display_name,role,color,tzid) values($1,'Other dependent','dependent','#123456','UTC') returning id").bind(other_h).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.member_daily_load(member_id,date,planned_minutes,available_minutes,starts_at,ends_at,derived_at) values($1,'2026-10-01',0,0,'2026-10-01','2026-10-02',now())").bind(other_m).execute(&pool).await.unwrap();
    let event: Uuid = sqlx::query_scalar("insert into porchlight.notification_events(household_id,recipient_member_id,kind,entity_type,entity_id) values($1,$2,'assigned','member',$2) returning id").bind(other_h).bind(other_m).fetch_one(&pool).await.unwrap();
    sqlx::query("insert into porchlight.notification_event_delivery(event_id,attempts,retry_at,last_error) values($1,1,now(),'database')").bind(event).execute(&pool).await.unwrap();
    let r = TestClient::new(&f.server.ca_pem, Some(&f.admin.device))
        .request(
            f.server.lan,
            f.admin
                .device
                .request("GET", "/v1/export/member", json!({}), None),
        )
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    for name in [
        "households",
        "ics_sources",
        "scheduled_reminders",
        "member_agenda",
        "member_daily_load",
    ] {
        assert!(
            r.json()["tables"].get(name).is_some(),
            "missing member inventory: {name}"
        );
    }
    let r = trust::admin(&f.server, "GET", "/v1/admin/export/household", json!({})).await;
    assert_eq!(r.status, 200);
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(r.body.as_ref()));
    let mut text = String::new();
    let mut tables = serde_json::Map::new();
    let mut last = String::new();
    let mut completion = Value::Null;
    for e in archive.entries().unwrap() {
        let mut e = e.unwrap();
        let path = e.path().unwrap().to_string_lossy().to_string();
        let mut entry = String::new();
        e.read_to_string(&mut entry).unwrap();
        if path == "data.json" {
            assert_eq!(
                serde_json::from_str::<Value>(&entry).unwrap()["format_version"],
                2
            );
        } else if let Some(name) = path
            .strip_prefix("tables/")
            .and_then(|p| p.strip_suffix(".jsonl"))
        {
            let rows: Vec<Value> = entry
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
            tables.insert(name.to_owned(), json!(rows));
        }
        if path == "complete.json" {
            completion = serde_json::from_str(&entry).unwrap();
        }
        last = path;
        text.push_str(&entry);
    }
    assert_eq!(last, "complete.json", "missing final completion entry");
    assert_eq!(completion["complete"], true);
    assert_eq!(
        completion["table_rows"].as_object().unwrap().len(),
        tables.len()
    );
    for (name, rows) in &tables {
        assert_eq!(
            completion["table_rows"][name],
            rows.as_array().unwrap().len()
        );
    }
    let data = json!({"tables":tables});
    assert!(
        !text.contains(&other_m.to_string()),
        "foreign member load exported"
    );
    assert!(
        !text.contains(&event.to_string()),
        "foreign event delivery exported"
    );
    assert!(
        data["tables"]["member_daily_load"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn household_export_step_up_is_action_bound_and_single_use(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    for action in ["device.revoke", "household.export"] {
        let r = TestClient::new(&f.server.ca_pem, Some(&f.admin.device))
            .request(
                f.server.lan,
                f.admin.device.request(
                    "POST",
                    "/v1/auth/reauth",
                    json!({"password":f.server.admin_password,"action":action}),
                    Some(&Uuid::now_v7().to_string()),
                ),
            )
            .await
            .unwrap();
        assert_eq!(r.status, 200);
        let token = r.json()["step_up_token"].as_str().unwrap().to_owned();
        for attempt in 0..2 {
            let mut req =
                f.admin
                    .device
                    .request("GET", "/v1/admin/export/household", json!({}), None);
            req.headers_mut()
                .insert("porchlight-step-up", token.parse().unwrap());
            let r = TestClient::new(&f.server.ca_pem, Some(&f.admin.device))
                .request(f.server.lan, req)
                .await
                .unwrap();
            assert_eq!(
                r.status,
                if action == "household.export" && attempt == 0 {
                    200
                } else {
                    403
                }
            );
        }
    }
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn household_streams_only_referenced_main_and_thumbnail_bytes(pool: PgPool) {
    use sha2::{Digest, Sha256};
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let i: Uuid = sqlx::query_scalar("insert into porchlight.items(household_id,title,kind,owner_id) values($1,'Blob export','todo',$2) returning id").bind(f.server.init.household_id).bind(f.admin.member_id).fetch_one(&pool).await.unwrap();
    let mut hashes = Vec::new();
    for (data, content_type) in [
        (b"main bytes".as_slice(), "text/plain"),
        (b"thumbnail bytes".as_slice(), "image/png"),
        (b"unreferenced bytes".as_slice(), "text/plain"),
    ] {
        let hash = hex::encode(Sha256::digest(data));
        sqlx::query("insert into porchlight.blobs(household_id,sha256,size,content_type) values($1,$2,$3,$4)").bind(f.server.init.household_id).bind(&hash).bind(data.len() as i64).bind(content_type).execute(&pool).await.unwrap();
        let path =
            porchlight_api::blobs::blob_path(&f.server.state.settings.data_dir, &hash).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
        hashes.push((hash, data.to_vec()));
    }
    sqlx::query("insert into porchlight.attachments(household_id,item_id,uploader_id,original_name,blob_sha256,thumbnail_sha256) values($1,$2,$3,'note.txt',$4,$5)").bind(f.server.init.household_id).bind(i).bind(f.admin.member_id).bind(&hashes[0].0).bind(&hashes[1].0).execute(&pool).await.unwrap();
    let r = trust::admin(&f.server, "GET", "/v1/admin/export/household", json!({})).await;
    assert_eq!(r.status, 200);
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(r.body.as_ref()));
    let mut blobs = std::collections::BTreeMap::new();
    let mut complete = Value::Null;
    for e in archive.entries().unwrap() {
        let mut e = e.unwrap();
        let path = e.path().unwrap().to_string_lossy().to_string();
        if path.starts_with("blobs/") {
            let mut bytes = Vec::new();
            e.read_to_end(&mut bytes).unwrap();
            blobs.insert(path, bytes);
        } else if path == "complete.json" {
            complete = serde_json::from_reader(&mut e).unwrap();
        }
    }
    assert_eq!(complete["blob_count"], 2);
    let mut expected: Vec<_> = hashes[..2].iter().map(|(h, _)| h.clone()).collect();
    expected.sort();
    assert_eq!(complete["blob_sha256"], json!(expected));
    assert_eq!(blobs.len(), 2);
    for (hash, bytes) in &hashes[..2] {
        assert_eq!(
            blobs[&format!("blobs/{}/{}/{}", &hash[..2], &hash[2..4], hash)],
            *bytes
        );
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let count: i64 = sqlx::query_scalar(
            "select count(*) from porchlight.blob_leases where holder='household-export'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        if count == 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "export lease not released"
        );
        tokio::task::yield_now().await;
    }
    f.server.stop().await;
}
