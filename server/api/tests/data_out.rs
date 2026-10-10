use porchlight_api::testing::{HouseholdFixture, TestClient, TestOptions, trust};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::io::Read;
use uuid::Uuid;
async fn app(
    f: &HouseholdFixture,
    who: &porchlight_api::testing::FixtureUser,
    method: &str,
    path: &str,
    body: Value,
    key: Option<&str>,
) -> porchlight_api::testing::TestResponse {
    TestClient::new(&f.server.ca_pem, Some(&who.device))
        .request(f.server.lan, who.device.request(method, path, body, key))
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn member_export_acl_and_ics_and_household_secret_exclusion(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let hidden = trust::admin(
        &f.server,
        "POST",
        "/v1/items",
        json!({"title":"invisible sentinel","visibility":"private"}),
    )
    .await;
    assert_eq!(hidden.status, 201);
    let appointment = app(&f,&f.users[0],"POST","/v1/items",json!({"title":"My appointment","kind":"appointment","time_mode":"fixed","starts_at":"2026-10-12T09:00:00Z","ends_at":"2026-10-12T10:00:00Z","tzid":"UTC"}),Some(&Uuid::now_v7().to_string())).await;
    assert_eq!(appointment.status, 201, "{}", appointment.json());
    let r = app(&f, &f.users[0], "GET", "/v1/export/member", json!({}), None).await;
    assert_eq!(r.status, 200, "{}", r.json());
    let data = r.json();
    assert!(!data.to_string().contains("invisible sentinel"));
    assert!(
        data["appointments_ics"]
            .as_str()
            .unwrap()
            .contains("BEGIN:VEVENT")
    );
    let parsed = porchlight_core::ics::parse(
        data["appointments_ics"].as_str().unwrap(),
        porchlight_core::ics::ParseOptions {
            fallback_tz: chrono_tz::UTC,
            horizon_start: "2026-01-01T00:00:00Z".parse().unwrap(),
            horizon_end: "2028-01-01T00:00:00Z".parse().unwrap(),
        },
    );
    assert_eq!(parsed.events.len(), 1);
    let denied = app(
        &f,
        &f.users[0],
        "GET",
        &format!("/v1/export/member?member_id={}", f.admin.member_id),
        json!({}),
        None,
    )
    .await;
    assert_eq!(denied.status, 403);
    let denied = app(
        &f,
        &f.admin,
        "GET",
        "/v1/admin/export/household",
        json!({}),
        None,
    )
    .await;
    assert_eq!(denied.status, 403, "step-up required");
    let r = trust::admin(&f.server, "GET", "/v1/admin/export/household", json!({})).await;
    assert_eq!(r.status, 200);
    let gzip = flate2::read::GzDecoder::new(r.body.as_ref());
    let mut archive = tar::Archive::new(gzip);
    let mut text = String::new();
    let mut tables = serde_json::Map::new();
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
        text.push_str(&entry);
    }
    let v = json!({"tables":tables});
    assert!(text.contains("invisible sentinel"));
    for key in [
        "user_credentials",
        "sessions",
        "enrollment_tokens",
        "password_resets",
        "api_tokens",
        "step_ups",
        "idempotency_keys",
        "install_state",
    ] {
        assert!(v["tables"].get(key).is_none(), "{key}");
    }
    assert!(!text.contains("password_hash"));
    f.server.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn backup_status_range_checksum_and_stale_recipient(pool: PgPool) {
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let bytes = b"encrypted-fixture-not-a-key";
    use sha2::{Digest, Sha256};
    let hash = hex::encode(Sha256::digest(bytes));
    let name = "porchlight-20261001T030000000000Z.tar.age";
    let root = f.server.state.settings.backup_dir.clone();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(name), bytes).unwrap();
    sqlx::query("insert into porchlight.backup_runs(status,started_at,finished_at,next_due_at,bundle_file,size,sha256) values('success','2026-10-01T03:00:00Z','2026-10-01T03:01:00Z','2026-10-02T03:00:00Z',$1,$2,$3)").bind(name).bind(bytes.len() as i64).bind(&hash).execute(&pool).await.unwrap();
    let status = app(&f, &f.admin, "GET", "/v1/backups", json!({}), None).await;
    assert_eq!(status.status, 200);
    assert_eq!(status.json()["latest"]["sha256"], hash);
    let denied = app(&f, &f.users[0], "GET", "/v1/backups", json!({}), None).await;
    assert_eq!(denied.status, 403);
    let mut req = f
        .admin
        .device
        .request("GET", "/v1/backups/latest/download", json!({}), None);
    req.headers_mut()
        .insert("range", "bytes=3-9".parse().unwrap());
    let r = TestClient::new(&f.server.ca_pem, Some(&f.admin.device))
        .request(f.server.lan, req)
        .await
        .unwrap();
    assert_eq!(r.status, 206);
    assert_eq!(r.body.as_ref(), &bytes[3..10]);
    // Exact 26h cutoff is not stale; one deduped system alert per stale episode,
    // only live super admins, regardless of PAT owner.
    let clock: chrono::DateTime<chrono::Utc> = "2026-10-02T05:01:00Z".parse().unwrap();
    let mut tx =
        porchlight_db::tx::begin_as(&f.server.state.pool, &porchlight_db::tx::Actor::system())
            .await
            .unwrap();
    porchlight_api::routes::backups::emit_stale(&mut tx, clock)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let count: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.notification_events where kind='backup_stale'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
    for _ in 0..2 {
        let mut tx =
            porchlight_db::tx::begin_as(&f.server.state.pool, &porchlight_db::tx::Actor::system())
                .await
                .unwrap();
        porchlight_api::routes::backups::emit_stale(&mut tx, clock + chrono::Duration::seconds(1))
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let recipients: Vec<Uuid> = sqlx::query_scalar(
        "select recipient_member_id from porchlight.notification_events where kind='backup_stale'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(recipients, vec![f.admin.member_id]);
    for scope in ["read", "write", "admin"] {
        let (_, token) = trust::seed_pat(&pool, &f.server, &[scope]).await;
        let r = trust::agent(
            &f.server,
            Some(&token),
            "GET",
            "/v1/admin/backups",
            json!({}),
            &Uuid::now_v7().to_string(),
        )
        .await;
        assert_eq!(r.status, if scope == "admin" { 200 } else { 403 });
        let export = trust::agent(
            &f.server,
            Some(&token),
            "GET",
            "/v1/admin/export/household",
            json!({}),
            &Uuid::now_v7().to_string(),
        )
        .await;
        assert_eq!(export.status, if scope == "admin" { 200 } else { 403 });
        let absent = trust::agent(
            &f.server,
            Some(&token),
            "GET",
            "/v1/backups/latest/download",
            json!({}),
            &Uuid::now_v7().to_string(),
        )
        .await;
        assert_eq!(absent.status, 404);
    }
    // If-Range mismatch restarts at 200, unsatisfiable ranges have a mapped 416.
    for (value, expected) in [("bytes=-4", 206), ("bytes=99-", 416)] {
        let mut req = f
            .admin
            .device
            .request("GET", "/v1/backups/latest/download", json!({}), None);
        req.headers_mut().insert("range", value.parse().unwrap());
        let r = TestClient::new(&f.server.ca_pem, Some(&f.admin.device))
            .request(f.server.lan, req)
            .await
            .unwrap();
        assert_eq!(r.status, expected);
        if expected == 416 {
            assert_eq!(
                r.headers["content-range"],
                format!("bytes */{}", bytes.len())
            );
        }
    }
    for (validator, expected) in [
        (format!("\"{hash}\""), 206),
        ("\"different-bundle\"".to_owned(), 200),
    ] {
        let mut req = f
            .admin
            .device
            .request("GET", "/v1/backups/latest/download", json!({}), None);
        req.headers_mut()
            .insert("range", "bytes=3-9".parse().unwrap());
        req.headers_mut()
            .insert("if-range", validator.parse().unwrap());
        let r = TestClient::new(&f.server.ca_pem, Some(&f.admin.device))
            .request(f.server.lan, req)
            .await
            .unwrap();
        assert_eq!(r.status, expected);
        assert_eq!(
            r.body.as_ref(),
            if expected == 206 {
                &bytes[3..10]
            } else {
                bytes.as_slice()
            }
        );
    }
    std::fs::write(root.join(name), vec![b'x'; bytes.len()]).unwrap();
    let bad = app(
        &f,
        &f.admin,
        "GET",
        "/v1/backups/latest/download",
        json!({}),
        None,
    )
    .await;
    assert_eq!(bad.status, 409);
    // A new successful backup ends the old episode and re-arms dedupe only after
    // the new success itself passes the strict 26h boundary.
    sqlx::query("insert into porchlight.backup_runs(status,started_at,finished_at,next_due_at,bundle_file,size,sha256) values('success',$1,$1,$2,'porchlight-20261001T040100000000Z.tar.age',$3,$4)").bind(clock-chrono::Duration::hours(25)).bind(clock).bind(bytes.len() as i64).bind(&hash).execute(&pool).await.unwrap();
    for (now, expected) in [
        (clock + chrono::Duration::hours(1), 1_i64),
        (
            clock + chrono::Duration::hours(1) + chrono::Duration::seconds(1),
            2,
        ),
    ] {
        let mut tx =
            porchlight_db::tx::begin_as(&f.server.state.pool, &porchlight_db::tx::Actor::system())
                .await
                .unwrap();
        porchlight_api::routes::backups::emit_stale(&mut tx, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let count: i64 = sqlx::query_scalar(
            "select count(*) from porchlight.notification_events where kind='backup_stale'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, expected);
    }
    f.server.stop().await;
}
