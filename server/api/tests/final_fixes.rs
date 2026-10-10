mod common;
use common::*;
use futures_util::{FutureExt, StreamExt};
use porchlight_api::{
    lifecycle,
    testing::{TestClient, TestOptions, TestServer},
};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

async fn head<I: AsyncWrite + Unpin>(
    io: &mut I,
    s: &TestServer,
    path: &str,
    token: &str,
    key: &str,
    size: usize,
) {
    let begun = s.state.mutation_body_started.notified();
    tokio::pin!(begun);
    begun.as_mut().enable();
    io.write_all(format!("POST {path} HTTP/1.1\r\nHost: localhost\r\nPorchlight-Client-Version: 0.1.0\r\nAuthorization: Bearer {token}\r\nIdempotency-Key: {key}\r\nContent-Type: application/json\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), begun)
        .await
        .expect("headers must authenticate before revocation");
}
async fn response<I: AsyncRead + Unpin>(io: &mut I) -> (u16, serde_json::Value) {
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(4), io.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = text.split_once("\r\n\r\n").unwrap().1;
    (status, serde_json::from_str(body).unwrap())
}
async fn no_claim(pool: &PgPool, key: &str) {
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM porchlight.idempotency_keys WHERE idem_key=$1")
            .bind(key)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(count, 0, "authorization failures are not stored");
}

#[sqlx::test(migrations = "../db/migrations")]
async fn held_pat_body_revalidates_revoke_demote_and_delete(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let target = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    for invalidate in ["revoke", "demote", "delete", "scope"] {
        for path in [
            format!("/v1/admin/devices/{}/revoke", target.device_id),
            "/v1/admin/tokens".to_owned(),
        ] {
            let user: Uuid = sqlx::query_scalar(
                "INSERT INTO porchlight.users(household_id,username) VALUES ($1,$2) RETURNING id",
            )
            .bind(s.init.household_id)
            .bind(format!("u{}", &Uuid::now_v7().simple().to_string()[..20]))
            .fetch_one(&pool)
            .await
            .unwrap();
            let member: Uuid = sqlx::query_scalar("INSERT INTO porchlight.household_members(household_id,user_id,role,display_name,color,tzid) VALUES ($1,$2,'super_admin','Caller','#AABBCC','UTC') RETURNING id").bind(s.init.household_id).bind(user).fetch_one(&pool).await.unwrap();
            let token = porchlight_core::auth::token::Token::generate(
                porchlight_core::auth::token::TokenKind::Pat,
            );
            let pat: Uuid = sqlx::query_scalar("INSERT INTO porchlight.api_tokens(household_id,owner_member_id,name,token_hash,scopes,expires_at) VALUES ($1,$2,'Caller',$3,ARRAY['admin'],now()+interval '1 day') RETURNING id").bind(s.init.household_id).bind(member).bind(token.hash().as_slice()).fetch_one(&pool).await.unwrap();
            let key = Uuid::now_v7().to_string();
            let body = if path.ends_with("tokens") {
                json!({"name":"held","scopes":["read"]})
            } else {
                json!({})
            }
            .to_string();
            let mut io = tokio::net::UnixStream::connect(&s.agent_socket)
                .await
                .unwrap();
            head(&mut io, &s, &path, token.expose(), &key, body.len()).await;
            match invalidate {
                "revoke" => {
                    sqlx::query("UPDATE porchlight.api_tokens SET revoked_at=now() WHERE id=$1")
                        .bind(pat)
                        .execute(&pool)
                        .await
                        .unwrap();
                }
                "demote" => {
                    sqlx::query("UPDATE porchlight.household_members SET role='user' WHERE id=$1")
                        .bind(member)
                        .execute(&pool)
                        .await
                        .unwrap();
                }
                "delete" => {
                    sqlx::query(
                        "UPDATE porchlight.household_members SET deleted_at=now() WHERE id=$1",
                    )
                    .bind(member)
                    .execute(&pool)
                    .await
                    .unwrap();
                }
                _ => {
                    sqlx::query(
                        "UPDATE porchlight.api_tokens SET scopes=ARRAY['read'] WHERE id=$1",
                    )
                    .bind(pat)
                    .execute(&pool)
                    .await
                    .unwrap();
                }
            }
            io.write_all(body.as_bytes()).await.unwrap();
            let (status, p) = response(&mut io).await;
            assert_eq!(status, if invalidate == "scope" { 403 } else { 401 });
            assert_eq!(
                p["type"],
                if invalidate == "scope" {
                    "urn:porchlight:problem:forbidden"
                } else {
                    "urn:porchlight:problem:unauthenticated"
                }
            );
            no_claim(&pool, &key).await;
            let revoked: bool = sqlx::query_scalar(
                "SELECT revoked_at IS NOT NULL FROM porchlight.devices WHERE id=$1",
            )
            .bind(target.device_id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(!revoked);
            let created: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM porchlight.api_tokens WHERE household_id=$1 AND name='held'",
            )
            .bind(s.init.household_id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(created, 0);
        }
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn held_session_and_device_body_cannot_renew_or_change_password(pool: PgPool) {
    let mut opts = TestOptions {
        watch_revocations: false,
        ..Default::default()
    };
    opts.settings.device_cert_days = 20;
    let s = TestServer::start_with(&pool, opts).await;
    let original: String =
        sqlx::query_scalar("SELECT password_hash FROM porchlight.user_credentials")
            .fetch_one(&pool)
            .await
            .unwrap();
    for path in ["/v1/devices/self/certificate", "/v1/auth/password"] {
        for revoke_device in [false, true] {
            let d = s
                .session_device(s.init.admin_member_id, Platform::Desktop)
                .await;
            let key = Uuid::now_v7().to_string();
            let (csr, _) = porchlight_pki::generate_csr("Held renewal").unwrap();
            let body = if path.ends_with("certificate") { json!({"csr":csr}) } else { json!({"current_password":s.admin_password,"new_password":"a changed private password"}) }.to_string();
            let mut io = TestClient::new(&s.ca_pem, Some(&d))
                .connect(s.lan)
                .await
                .unwrap();
            head(
                &mut io,
                &s,
                path,
                d.session.as_deref().unwrap(),
                &key,
                body.len(),
            )
            .await;
            if revoke_device {
                sqlx::query("UPDATE porchlight.devices SET revoked_at=now() WHERE id=$1")
                    .bind(d.device_id)
                    .execute(&pool)
                    .await
                    .unwrap();
            } else {
                sqlx::query("UPDATE porchlight.sessions SET revoked_at=now() WHERE device_id=$1 AND revoked_at IS NULL").bind(d.device_id).execute(&pool).await.unwrap();
            }
            io.write_all(body.as_bytes()).await.unwrap();
            let (status, p) = response(&mut io).await;
            assert_eq!(status, 401);
            assert_eq!(
                p["type"],
                if revoke_device {
                    "urn:porchlight:problem:device-revoked"
                } else {
                    "urn:porchlight:problem:session-invalid"
                }
            );
            no_claim(&pool, &key).await;
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM porchlight.device_certificates WHERE device_id=$1",
            )
            .bind(d.device_id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(count, 1);
        }
    }
    let failures: i32 =
        sqlx::query_scalar("SELECT failed_attempts FROM porchlight.user_credentials")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(failures, 0);
    let after: String = sqlx::query_scalar("SELECT password_hash FROM porchlight.user_credentials")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(original == after, "password must not change");
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn slow_bodies_close_and_release_listener_permits(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            max_connections: 2,
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let mut a = client.connect(s.lan).await.unwrap();
    let mut b = client.connect(s.lan).await.unwrap();
    head(
        &mut a,
        &s,
        "/v1/auth/password",
        d.session.as_deref().unwrap(),
        "slow-a",
        100,
    )
    .await;
    head(
        &mut b,
        &s,
        "/v1/auth/password",
        d.session.as_deref().unwrap(),
        "slow-b",
        100,
    )
    .await;
    let start = tokio::time::Instant::now();
    let (a, b) = tokio::join!(response(&mut a), response(&mut b));
    for (status, p) in [a, b] {
        assert_eq!(status, 408);
        assert_eq!(p["type"], "urn:porchlight:problem:request-timeout");
    }
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(
        client
            .request(s.lan, d.request("GET", "/v1/connection", json!({}), None))
            .await
            .unwrap()
            .status,
        200
    );
    no_claim(&pool, "slow-a").await;
    no_claim(&pool, "slow-b").await;
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn expired_certificate_keepalive_stream_and_renewal(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let (mut sender, connection) = hyper::client::conn::http1::handshake(
        hyper_util::rt::TokioIo::new(client.connect(s.lan).await.unwrap()),
    )
    .await
    .unwrap();
    let task = tokio::spawn(connection);
    use http_body_util::BodyExt;
    let first = sender
        .send_request(d.request("GET", "/v1/connection", json!({}), None))
        .await
        .unwrap();
    assert_eq!(first.status(), 200);
    first.into_body().collect().await.unwrap();
    let mut stream = client
        .stream(s.lan, d.request("GET", "/v1/stream", json!({}), None))
        .await
        .unwrap();
    assert!(stream.next().await.is_some());
    // Superuser-only clock simulation: certificate history remains immutable to application roles.
    let mut expiry = pool.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role=replica")
        .execute(&mut *expiry)
        .await
        .unwrap();
    sqlx::query("UPDATE porchlight.device_certificates SET not_after=now()-interval '1 second' WHERE device_id=$1").bind(d.device_id).execute(&mut *expiry).await.unwrap();
    expiry.commit().await.unwrap();
    let r = sender
        .send_request(d.request("GET", "/v1/connection", json!({}), None))
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let p: serde_json::Value =
        serde_json::from_slice(&r.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(
        p["type"],
        "urn:porchlight:problem:device-certificate-expired"
    );
    let (csr, _) = porchlight_pki::generate_csr("Expired").unwrap();
    let r = client
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/devices/self/certificate",
                json!({"csr":csr}),
                Some("expired-renewal"),
            ),
        )
        .await
        .unwrap();
    problem(&r, 401, "device-certificate-expired");
    tokio::time::timeout(Duration::from_secs(3), async {
        while stream.next().await.is_some() {}
    })
    .await
    .expect("expired stream must end at heartbeat");
    no_claim(&pool, "expired-renewal").await;
    task.abort();
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn readiness_barrier_survives_failed_seed(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            fail_seed_once: true,
            ..Default::default()
        },
    )
    .await;
    let client = TestClient::new(&s.ca_pem, None);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), client.connect(s.enroll))
            .await
            .is_err(),
        "TLS/HTTP cannot be accepted before readiness"
    );
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let r = TestClient::new(&s.ca_pem, Some(&d))
        .request(s.lan, d.request("GET", "/v1/connection", json!({}), None))
        .await
        .unwrap();
    assert_eq!(r.status, 200);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn startup_purges_expired_sealed_responses_only(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    for (key, seconds) in [("expired", -1), ("live", 3600)] {
        sqlx::query("INSERT INTO porchlight.idempotency_keys(household_id,scope_kind,scope_id,idem_key,request_hash,response_status,response_sealed,expires_at) VALUES ($1,'pat',$2,$3,$4,200,$5,now()+$6*interval '1 second')")
            .bind(s.init.household_id).bind(Uuid::now_v7()).bind(key).bind([0u8;32].as_slice()).bind(b"sealed test response".as_slice()).bind(seconds).execute(&pool).await.unwrap();
    }
    let (stop, rx) = tokio::sync::oneshot::channel();
    let shutdown = async {
        let _ = rx.await;
    }
    .shared();
    lifecycle::run(s.state.clone(), shutdown, true, false, async {
        let keys: Vec<String> = sqlx::query_scalar(
            "SELECT idem_key FROM porchlight.idempotency_keys ORDER BY idem_key",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(keys, ["live"]);
        stop.send(()).unwrap();
        Ok(())
    })
    .await
    .unwrap();
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn enrollment_limits_address_before_slow_body_read(pool: PgPool) {
    let mut opts = TestOptions::default();
    opts.limits.auth_addr = 1;
    opts.settings.body_timeout = Duration::from_secs(3);
    let clock = std::time::Instant::now();
    opts.rate_clock = std::sync::Arc::new(move || clock);
    let s = TestServer::start_with(&pool, opts).await;
    let client = TestClient::new(&s.ca_pem, None);
    let mut first = client.connect(s.enroll).await.unwrap();
    head(
        &mut first,
        &s,
        "/v1/enrollment/complete",
        "unused",
        "enroll-slow",
        100,
    )
    .await;
    for _ in 0..3 {
        let mut io = client.connect(s.enroll).await.unwrap();
        io.write_all(b"POST /v1/enrollment/complete HTTP/1.1\r\nHost: localhost\r\nPorchlight-Client-Version: 0.1.0\r\nContent-Type: application/json\r\nIdempotency-Key: limited\r\nContent-Length: 100\r\n\r\n").await.unwrap();
        let (status, p) = tokio::time::timeout(Duration::from_millis(500), response(&mut io))
            .await
            .expect("limit must run before body read");
        assert_eq!(status, 429);
        assert_eq!(p["type"], "urn:porchlight:problem:rate-limited");
    }
    drop(first);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn shutdown_before_readiness_is_clean(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            fail_seed_once: true,
            ..Default::default()
        },
    )
    .await;
    s.stop().await;
}
