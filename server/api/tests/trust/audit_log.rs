use crate::common::*;
use porchlight_api::testing::agent_request;
use porchlight_api::testing::trust::{admin, agent, req, seed_pat, user_named};
use porchlight_api::testing::{TestDevice, TestOptions, TestServer};
use porchlight_core::auth::session::Platform;
use porchlight_pki::{generate_csr, parse_cert_pem};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn security_operations_are_attributed_secret_free_and_admin_only(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let mut d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let other = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let (pat_id, pat) = seed_pat(&pool, &s, &["admin"]).await;
    let changed = "correct new fixture phrase for tests";
    assert_eq!(
        post(
            &s,
            &d,
            "/v1/auth/password",
            json!({"current_password":s.admin_password,"new_password":changed})
        )
        .await
        .status,
        204
    );
    let reset = admin(
        &s,
        "POST",
        "/v1/admin/users/admin/password-reset",
        json!({}),
    )
    .await;
    assert_eq!(reset.status, 201);
    let code = reset.json()["code"].as_str().unwrap().to_owned();
    let redeemed = post(
        &s,
        &d,
        "/v1/auth/reset-password",
        json!({"code":code,"new_password":s.admin_password}),
    )
    .await;
    assert_eq!(redeemed.status, 200);
    d.session = Some(
        redeemed.json()["session"]["token"]
            .as_str()
            .unwrap()
            .to_owned(),
    );
    assert_eq!(
        agent(
            &s,
            Some(&pat),
            "POST",
            &format!("/v1/admin/devices/{}/revoke", other.device_id),
            json!({}),
            "revoke-fixture"
        )
        .await
        .status,
        200
    );
    let created = admin(
        &s,
        "POST",
        "/v1/admin/tokens",
        json!({"name":"fixture token","scopes":["read"]}),
    )
    .await;
    assert_eq!(created.status, 201);
    let token = created.json();
    assert_eq!(
        admin(
            &s,
            "POST",
            &format!("/v1/admin/tokens/{}/revoke", token["id"].as_str().unwrap()),
            json!({})
        )
        .await
        .status,
        200
    );
    let recovered =
        porchlight_api::admin_recovery::mint(&s.state.pool, "admin", "fixture recovery", 30)
            .await
            .unwrap();
    let (_, member) = user_named(&pool, &s, "sam", "Sam Rivera").await;
    let mut version: i32 =
        sqlx::query_scalar("SELECT version FROM porchlight.household_members WHERE id=$1")
            .bind(member)
            .fetch_one(&pool)
            .await
            .unwrap();
    for (method, body) in [
        ("PATCH", json!({"role":"super_admin"})),
        ("DELETE", json!({})),
    ] {
        let path = format!("/v1/admin/users/{member}");
        let mut request = req(method, &path, body, &Uuid::now_v7().to_string());
        request
            .headers_mut()
            .insert("if-match", version.to_string().parse().unwrap());
        let response = agent_request(&s.agent_socket, Some(&pat), request).await;
        assert_eq!(response.status, 200);
        version = response.json()["version"].as_i64().unwrap() as i32;
    }
    let response = admin(&s, "GET", "/v1/admin/security-events", json!({})).await;
    assert_eq!(response.status, 200);
    let data = response.json();
    let events = data["items"].as_array().unwrap();
    for kind in [
        "password_changed",
        "password_reset_issued",
        "password_reset_redeemed",
        "session_revoked",
        "device_enrolled",
        "device_revoked",
        "token_created",
        "token_revoked",
        "admin_token_recovered",
        "role_changed",
        "member_removed",
    ] {
        assert!(events.iter().any(|e| e["kind"] == kind), "missing {kind}");
    }
    let event = events
        .iter()
        .find(|e| e["kind"] == "device_revoked")
        .unwrap();
    assert_eq!(event["target_id"], other.device_id.to_string());
    assert_eq!(event["actor"]["token_id"], pat_id.to_string());
    assert_eq!(event["actor"]["application_name"], "agent:Test PAT");
    assert!(event["actor"]["request_id"].is_string());
    let raw: serde_json::Value =
        sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(e)) FROM porchlight.security_events e")
            .fetch_one(&pool)
            .await
            .unwrap();
    let encoded = raw.to_string();
    for secret in [
        s.admin_password.as_str(),
        changed,
        code.as_str(),
        pat.as_str(),
        s.bootstrap_token(),
        recovered.expose(),
        token["token"].as_str().unwrap(),
        d.session.as_deref().unwrap(),
        d.key_pem.as_str(),
        "password_hash",
        "token_hash",
        "code_hash",
        "csr",
    ] {
        assert!(
            !encoded.contains(secret),
            "security history must not contain credential material"
        );
    }
    for scope in ["read", "write"] {
        let (_, scoped) = seed_pat(&pool, &s, &[scope]).await;
        assert_eq!(
            agent(
                &s,
                Some(&scoped),
                "GET",
                "/v1/admin/security-events",
                json!({}),
                "unused"
            )
            .await
            .status,
            403
        );
    }
    assert_eq!(get(&s, &d, "/v1/admin/security-events").await.status, 404);
    let first = admin(
        &s,
        "GET",
        "/v1/admin/security-events?kind=device_enrolled&limit=1",
        json!({}),
    )
    .await
    .json();
    assert_eq!(first["items"].as_array().unwrap().len(), 1);
    let cursor = first["next_cursor"].as_str().unwrap();
    let next = admin(
        &s,
        "GET",
        &format!("/v1/admin/security-events?kind=device_enrolled&limit=1&cursor={cursor}"),
        json!({}),
    )
    .await
    .json();
    assert_ne!(first["items"][0]["id"], next["items"][0]["id"]);
    for path in [
        "/v1/admin/security-events?limit=201".to_owned(),
        "/v1/admin/security-events?since=invalid".to_owned(),
        format!("/v1/admin/security-events?kind=login_failed&cursor={cursor}"),
    ] {
        assert_eq!(admin(&s, "GET", &path, json!({})).await.status, 400);
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn security_history_is_append_only_retained_and_exported(pool: PgPool) {
    use std::io::Read;
    let s = TestServer::start(&pool).await;
    let now = chrono::Utc::now();
    for offset in [0, 1] {
        sqlx::query("INSERT INTO porchlight.security_events(household_id,kind,target_type,target_id,outcome,occurred_at) VALUES($1,'login_failed','member',$2,'failed',$3::timestamptz-interval '1 year'-$4*interval '1 microsecond')")
            .bind(s.init.household_id).bind(s.init.admin_member_id).bind(now).bind(offset).execute(&pool).await.unwrap();
    }
    for statement in [
        "UPDATE porchlight.security_events SET outcome='succeeded'",
        "DELETE FROM porchlight.security_events",
        "TRUNCATE porchlight.security_events",
    ] {
        let e = sqlx::query(statement).execute(&pool).await.unwrap_err();
        assert_eq!(
            e.as_database_error().unwrap().code().as_deref(),
            Some("PL403")
        );
        assert!(sqlx::query(statement).execute(&s.state.pool).await.is_err());
    }
    let allowed: bool = sqlx::query_scalar("SELECT has_table_privilege('porchlight_agent','porchlight.security_events','SELECT') OR has_table_privilege('porchlight_agent','porchlight.security_events','INSERT') OR has_table_privilege('porchlight_app','porchlight.security_events','DELETE')").fetch_one(&pool).await.unwrap();
    assert!(!allowed);
    let count: serde_json::Value = sqlx::query_scalar("SELECT porchlight.pl_retention($1)")
        .bind(now)
        .fetch_one(&s.state.pool)
        .await
        .unwrap();
    assert_eq!(count["security_events"], 1);
    let logged: i64 = sqlx::query_scalar("SELECT (counts->>'security_events')::bigint FROM porchlight.purge_log ORDER BY created_at DESC LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(logged, 1);
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.security_events WHERE kind='login_failed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(remaining, 1);
    let member = admin(&s, "GET", "/v1/export/member", json!({})).await;
    assert_eq!(member.status, 200);
    assert!(member.json()["tables"].get("security_events").is_none());
    let export = admin(&s, "GET", "/v1/admin/export/household", json!({})).await;
    assert_eq!(export.status, 200);
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(export.body.as_ref()));
    let mut rows = None;
    let mut complete = None;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().to_string_lossy().to_string();
        if path == "tables/security_events.jsonl" || path == "complete.json" {
            let mut bytes = String::new();
            entry.read_to_string(&mut bytes).unwrap();
            if path == "complete.json" {
                complete = Some(serde_json::from_str::<serde_json::Value>(&bytes).unwrap());
            } else {
                rows = Some(bytes.lines().count());
            }
        }
    }
    assert!(rows.unwrap() > 0);
    assert_eq!(
        complete.unwrap()["table_rows"]["security_events"]
            .as_u64()
            .unwrap() as usize,
        rows.unwrap()
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn login_history_survives_success(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let mut d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    assert_eq!(
        post(
            &s,
            &d,
            "/v1/auth/login",
            json!({"password":"incorrect fixture"})
        )
        .await
        .status,
        401
    );
    login(&s, &mut d).await;
    let exists: bool =
        sqlx::query_scalar("SELECT to_regclass('porchlight.security_events') IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(exists, "security event history is missing");
    let events: Vec<(String, serde_json::Value)> = sqlx::query_as("SELECT kind, actor FROM porchlight.security_events WHERE kind IN ('login_failed','login_succeeded') ORDER BY occurred_at,id")
        .fetch_all(&pool).await.unwrap();
    assert_eq!(
        events.iter().map(|e| e.0.as_str()).collect::<Vec<_>>(),
        ["login_failed", "login_succeeded"]
    );
    for (_, actor) in events {
        assert_eq!(actor["device_id"], d.device_id.to_string());
        assert_eq!(actor["member_id"], s.init.admin_member_id.to_string());
        assert!(actor["request_id"].is_string());
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn renewal_successor_only_replays_and_daily_budget_is_durable(pool: PgPool) {
    let mut opts = TestOptions::default();
    opts.settings.device_cert_days = 20;
    let s = TestServer::start_with(&pool, opts).await;
    let mut d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let (csr, key) = generate_csr("renew fixture").unwrap();
    let body = json!({"csr":csr});
    let idem = Uuid::now_v7().to_string();
    let first = send(
        &s,
        &d,
        "POST",
        "/v1/devices/self/certificate",
        body.clone(),
        Some(&idem),
    )
    .await;
    assert_eq!(first.status, 200);
    let replay = send(
        &s,
        &d,
        "POST",
        "/v1/devices/self/certificate",
        body.clone(),
        Some(&idem),
    )
    .await;
    assert_eq!(replay.status, 200);
    assert!(first.body == replay.body);
    let cert = first.json()["certificate"].as_str().unwrap().to_owned();
    let successor = TestDevice {
        device_id: d.device_id,
        fingerprint: parse_cert_pem(&cert).unwrap().fingerprint,
        cert_pem: cert,
        key_pem: key.to_string(),
        session: d.session.clone(),
    };
    assert_eq!(get(&s, &successor, "/v1/me").await.status, 200);
    assert_eq!(
        post(&s, &d, "/v1/devices/self/certificate", body.clone())
            .await
            .status,
        429,
        "an old certificate with a used successor must not issue again"
    );
    assert_eq!(get(&s, &d, "/v1/me").await.status, 200);
    let mut cert = first.json()["certificate"].as_str().unwrap().to_owned();
    // Enrollment counts as issuance: one more renewal reaches three in 24 hours.
    for expected in [200, 429] {
        d = TestDevice {
            device_id: d.device_id,
            fingerprint: parse_cert_pem(&cert).unwrap().fingerprint,
            cert_pem: cert.clone(),
            key_pem: key.to_string(),
            session: d.session.clone(),
        };
        let r = post(&s, &d, "/v1/devices/self/certificate", body.clone()).await;
        assert_eq!(r.status, expected);
        if expected == 200 {
            cert = r.json()["certificate"].as_str().unwrap().to_owned();
        }
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.device_certificates WHERE device_id=$1",
    )
    .bind(d.device_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 3);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn failed_login_replay_and_lockout_keep_separate_history(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    for _ in 0..2 {
        assert_eq!(
            send(
                &s,
                &d,
                "POST",
                "/v1/auth/login",
                json!({"password":"incorrect fixture"}),
                Some("same-failed-login")
            )
            .await
            .status,
            401
        );
    }
    sqlx::query("UPDATE porchlight.user_credentials SET locked_until=now()+interval '1 minute'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        post(
            &s,
            &d,
            "/v1/auth/login",
            json!({"password":s.admin_password})
        )
        .await
        .status,
        429
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.security_events WHERE kind='login_failed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 2,
        "replay is not another attempt; a fresh locked login is"
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn concurrent_renewals_replace_only_unused_successors_within_budget(pool: PgPool) {
    let mut opts = TestOptions::default();
    opts.settings.device_cert_days = 20;
    let s = TestServer::start_with(&pool, opts).await;
    let d = s
        .enroll("admin", &s.admin_password, Platform::Desktop)
        .await;
    let (csr, _) = generate_csr("concurrent fixture").unwrap();
    let (a, b) = tokio::join!(
        send(
            &s,
            &d,
            "POST",
            "/v1/devices/self/certificate",
            json!({"csr":csr}),
            Some("renew-a")
        ),
        send(
            &s,
            &d,
            "POST",
            "/v1/devices/self/certificate",
            json!({"csr":csr}),
            Some("renew-b")
        )
    );
    let mut statuses = [a.status, b.status];
    statuses.sort();
    assert_eq!(statuses, [200, 200]);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.device_certificates WHERE device_id=$1",
    )
    .bind(d.device_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 3);
    let live: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.device_certificates WHERE device_id=$1 AND first_seen_at IS NULL AND revoked_at IS NULL")
        .bind(d.device_id).fetch_one(&pool).await.unwrap();
    assert_eq!(live, 1);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.security_events WHERE kind='certificate_renewed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 2);
    s.stop().await;
}
