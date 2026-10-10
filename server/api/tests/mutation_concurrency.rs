//! N1 regression: real overlapping requests, in both submission orders, repeated twenty times.
use porchlight_api::testing::{TestClient, TestOptions, TestServer, agent_request};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use tracing::instrument::WithSubscriber;

// Keep a WARN-enabled dispatcher alive process-wide so callsite interest never
// comes from another test thread's no-op default while a scoped capture is live.
fn ensure_global_warn_sink() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::WARN)
                .with_writer(std::io::sink)
                .finish(),
        )
        .expect("install the test binary's global WARN sink once");
    });
}

#[derive(Clone, Default)]
struct SqlstateLogs(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for SqlstateLogs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl SqlstateLogs {
    fn dispatch(&self) -> tracing::Dispatch {
        ensure_global_warn_sink();
        let writer = self.clone();
        assert!(
            tracing::enabled!(tracing::Level::WARN),
            "global WARN interest must precede scoped capture"
        );
        tracing::Dispatch::new(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::WARN)
                .with_ansi(false)
                .without_time()
                .with_writer(move || writer.clone())
                .finish(),
        )
    }
    fn assert_only_nowait(&self, conflicts: usize) {
        let text = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
        assert!(
            !text.contains("40P01"),
            "deadlock is never an accepted cross-revoke retry"
        );
        assert!(
            !text.contains("40001"),
            "cross-revoke conflict must be NOWAIT"
        );
        assert_eq!(
            text.matches("sqlstate=\"55P03\"").count(),
            conflicts,
            "every 409 must be a logged NOWAIT conflict"
        );
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn same_device_mutation_pairs_do_not_deadlock(pool: PgPool) {
    let mut options = TestOptions::default();
    options.settings.device_renew_days = 365;
    let s = TestServer::start_with(&pool, options).await;
    for pair in 0..5 {
        for reverse in [false, true] {
            for _ in 0..20 {
                let d = s
                    .session_device(s.init.admin_member_id, Platform::Desktop)
                    .await;
                let (csr, _) = porchlight_pki::generate_csr("Renewal").unwrap();
                let renewal = ("/v1/devices/self/certificate", json!({"csr":csr}));
                let reauth = (
                    "/v1/auth/reauth",
                    json!({"password":s.admin_password,"action":"device.revoke"}),
                );
                let (a, b) = match pair {
                    0 => (renewal, ("/v1/auth/logout", json!({}))),
                    1 => (
                        ("/v1/auth/login", json!({"password":s.admin_password})),
                        renewal,
                    ),
                    2 => (
                        (
                            "/v1/auth/password",
                            json!({"current_password":s.admin_password,"new_password":s.admin_password}),
                        ),
                        renewal,
                    ),
                    3 => (
                        ("/v1/auth/logout", json!({})),
                        ("/v1/auth/logout", json!({})),
                    ),
                    _ => (renewal, reauth),
                };
                let client = TestClient::new(&s.ca_pem, Some(&d));
                let request_a = d.request("POST", a.0, a.1, None);
                let request_b = d.request("POST", b.0, b.1, None);
                let (left, right) = if pair == 4 {
                    let revoke = axum::http::Request::builder()
                        .method("POST")
                        .uri(format!("/v1/admin/devices/{}/revoke", d.device_id))
                        .header("host", "localhost")
                        .header("content-type", "application/json")
                        .header("porchlight-client-version", porchlight_core::VERSION)
                        .header("idempotency-key", uuid::Uuid::now_v7().to_string())
                        .body(http_body_util::Full::new(hyper::body::Bytes::from_static(
                            b"{}",
                        )))
                        .unwrap();
                    // Confirm the device handshake with an HTTP round trip before racing
                    // revocation. A new handshake racing revoke is correctly a TLS refusal,
                    // not a mutation protocol result; this test targets the in-tx locks.
                    use http_body_util::BodyExt;
                    let tls = client.connect(s.lan).await.unwrap();
                    let (mut sender, connection) =
                        hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(tls))
                            .await
                            .unwrap();
                    let connection_task = tokio::spawn(connection);
                    let warmup = sender
                        .send_request(d.request("GET", "/v1/me", json!({}), None))
                        .await
                        .unwrap();
                    assert_eq!(warmup.status(), 200);
                    warmup.into_body().collect().await.unwrap();
                    let mutation = async {
                        let response = sender.send_request(request_a).await.unwrap();
                        let (parts, body) = response.into_parts();
                        porchlight_api::testing::TestResponse {
                            status: parts.status.as_u16(),
                            headers: parts.headers,
                            body: body.collect().await.unwrap().to_bytes(),
                        }
                    };
                    let responses = if reverse {
                        tokio::join!(
                            agent_request(&s.agent_socket, Some(s.bootstrap_token()), revoke),
                            mutation
                        )
                    } else {
                        tokio::join!(
                            mutation,
                            agent_request(&s.agent_socket, Some(s.bootstrap_token()), revoke)
                        )
                    };
                    drop(sender);
                    connection_task.abort();
                    let _ = connection_task.await;
                    responses
                } else if reverse {
                    let (x, y) = tokio::join!(
                        client.request(s.lan, request_b),
                        client.request(s.lan, request_a)
                    );
                    (x.unwrap(), y.unwrap())
                } else {
                    let (x, y) = tokio::join!(
                        client.request(s.lan, request_a),
                        client.request(s.lan, request_b)
                    );
                    (x.unwrap(), y.unwrap())
                };
                for response in [&left, &right] {
                    assert!(
                        matches!(response.status, 200 | 204 | 401),
                        "pair {pair}, reverse {reverse}: {response:?}"
                    );
                }
                let live: i64 = sqlx::query_scalar("SELECT count(*) FROM porchlight.sessions WHERE device_id=$1 AND revoked_at IS NULL")
                    .bind(d.device_id).fetch_one(&pool).await.unwrap();
                if pair == 1 || pair == 2 {
                    assert_eq!(live, 1, "one live session is retained");
                }
                if pair == 2 {
                    let mut statuses = [left.status, right.status];
                    statuses.sort();
                    assert_eq!(
                        statuses,
                        [200, 204],
                        "password change preserves the renewing session"
                    );
                }
                if pair == 4 {
                    let fps = porchlight_db::devices::revoked_fingerprints(&pool)
                        .await
                        .unwrap();
                    let certs:Vec<Vec<u8>> = sqlx::query_scalar("SELECT fingerprint_sha256 FROM porchlight.device_certificates WHERE device_id=$1").bind(d.device_id).fetch_all(&pool).await.unwrap();
                    for cert in certs {
                        let fp: [u8; 32] = cert.try_into().unwrap();
                        assert!(fps.contains(&fp));
                        assert!(s.state.revoked.contains(&fp));
                    }
                    if let Some(response) = [&left, &right]
                        .into_iter()
                        .find(|r| r.status == 200 && r.json().get("certificate").is_some())
                    {
                        let pem = response.json()["certificate"].as_str().unwrap().to_owned();
                        let fp = porchlight_pki::parse_cert_pem(&pem).unwrap().fingerprint;
                        assert!(fps.contains(&fp));
                        assert!(s.state.revoked.contains(&fp));
                    }
                }
                if pair == 0 || pair == 3 || pair == 4 {
                    assert_eq!(live, 0);
                }
            }
        }
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn different_device_password_changes_never_hold_sessions_across_credentials(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    for reverse in [false, true] {
        for _ in 0..20 {
            let a = s
                .session_device(s.init.admin_member_id, Platform::Desktop)
                .await;
            let b = s
                .session_device(s.init.admin_member_id, Platform::Desktop)
                .await;
            let ca = TestClient::new(&s.ca_pem, Some(&a));
            let cb = TestClient::new(&s.ca_pem, Some(&b));
            let body = json!({"current_password":s.admin_password,"new_password":s.admin_password});
            let ra = a.request("POST", "/v1/auth/password", body.clone(), None);
            let rb = b.request("POST", "/v1/auth/password", body, None);
            let (x, y) = if reverse {
                let (x, y) = tokio::join!(cb.request(s.lan, rb), ca.request(s.lan, ra));
                (x.unwrap(), y.unwrap())
            } else {
                let (x, y) = tokio::join!(ca.request(s.lan, ra), cb.request(s.lan, rb));
                (x.unwrap(), y.unwrap())
            };
            let mut statuses = [x.status, y.status];
            statuses.sort();
            assert_eq!(statuses, [204, 401]);
            let live:i64=sqlx::query_scalar("SELECT count(*) FROM porchlight.sessions WHERE device_id IN ($1,$2) AND revoked_at IS NULL").bind(a.device_id).bind(b.device_id).fetch_one(&pool).await.unwrap();
            assert_eq!(live, 1);
        }
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn legacy_password_lock_timeout_is_unstored_transient_conflict(pool: PgPool) {
    use porchlight_api::{
        routes,
        server::{ConnInfo, Transport},
    };
    use tower::ServiceExt;
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            db_pool_size: 1,
            ..Default::default()
        },
    )
    .await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    // Same production lock-timeout path, shortened on the sole test app connection.
    sqlx::query("SET lock_timeout='100ms'")
        .execute(&s.state.pool)
        .await
        .unwrap();
    let user_id: uuid::Uuid =
        sqlx::query_scalar("SELECT user_id FROM porchlight.household_members WHERE id=$1")
            .bind(s.init.admin_member_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let before: (String, i32) = sqlx::query_as(
        "SELECT password_hash, failed_attempts FROM porchlight.user_credentials WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("SELECT user_id FROM porchlight.user_credentials WHERE user_id=$1 FOR UPDATE")
        .bind(user_id)
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    let router = routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(d.fingerprint),
    }));
    let request = || {
        d.request("POST", "/v1/auth/password", json!({"current_password":s.admin_password,"new_password":"another correct horse battery staple"}), Some("legacy-credentials-lock")).map(axum::body::Body::new)
    };
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        router.clone().oneshot(request()),
    )
    .await
    .expect("credential lock wait must be bounded")
    .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(response.headers()["retry-after"], "1");
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["type"],
        "urn:porchlight:problem:conflict"
    );
    let claims: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.idempotency_keys WHERE idem_key='legacy-credentials-lock'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(claims, 0);
    let after: (String, i32) = sqlx::query_as(
        "SELECT password_hash, failed_attempts FROM porchlight.user_credentials WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, after);
    lock.rollback().await.unwrap();
    assert_eq!(router.oneshot(request()).await.unwrap().status(), 204);
    let claims: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM porchlight.idempotency_keys WHERE idem_key='legacy-credentials-lock'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(claims, 1);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn app_cross_revoke_is_nowait_conflict_not_deadlock(pool: PgPool) {
    use porchlight_api::{
        routes,
        server::{ConnInfo, Transport},
    };
    use tower::ServiceExt;
    let s = TestServer::start(&pool).await;
    for reverse in [false, true] {
        for _ in 0..20 {
            let a = s
                .session_device(s.init.admin_member_id, Platform::Desktop)
                .await;
            let b = s
                .session_device(s.init.admin_member_id, Platform::Desktop)
                .await;
            let request = |d: &porchlight_api::testing::TestDevice, target: uuid::Uuid| {
                let conn = ConnInfo {
                    transport: Transport::Lan,
                    client_addr: None,
                    client_cert_fingerprint: Some(d.fingerprint),
                };
                let router = routes::mtls_router(s.state.clone()).layer(axum::Extension(conn));
                let mut req = d.request(
                    "POST",
                    &format!("/v1/devices/{target}/revoke"),
                    json!({}),
                    None,
                );
                (router, req.headers_mut().clone(), req)
            };
            // Each caller obtains a real device-bound single-use step-up before the race.
            let stepup = |d: &porchlight_api::testing::TestDevice| {
                let router =
                    routes::mtls_router(s.state.clone()).layer(axum::Extension(ConnInfo {
                        transport: Transport::Lan,
                        client_addr: None,
                        client_cert_fingerprint: Some(d.fingerprint),
                    }));
                let req = d
                    .request(
                        "POST",
                        "/v1/auth/reauth",
                        json!({"password":s.admin_password,"action":"device.revoke"}),
                        None,
                    )
                    .map(axum::body::Body::new);
                async move {
                    let response = router.oneshot(req).await.unwrap();
                    assert_eq!(response.status(), 200);
                    let bytes = axum::body::to_bytes(response.into_body(), 4096)
                        .await
                        .unwrap();
                    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["step_up_token"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                }
            };
            let (sa, sb) = tokio::join!(stepup(&a), stepup(&b));
            let (ra, _, mut qa) = request(&a, b.device_id);
            qa.headers_mut()
                .insert("porchlight-step-up", sa.parse().unwrap());
            let (rb, _, mut qb) = request(&b, a.device_id);
            qb.headers_mut()
                .insert("porchlight-step-up", sb.parse().unwrap());
            let logs = SqlstateLogs::default();
            let dispatch = logs.dispatch();
            let (x, y) = if reverse {
                let (y, x) = tokio::join!(
                    rb.oneshot(qb.map(axum::body::Body::new))
                        .with_subscriber(dispatch.clone()),
                    ra.oneshot(qa.map(axum::body::Body::new))
                        .with_subscriber(dispatch.clone())
                );
                (x.unwrap(), y.unwrap())
            } else {
                let (x, y) = tokio::join!(
                    ra.oneshot(qa.map(axum::body::Body::new))
                        .with_subscriber(dispatch.clone()),
                    rb.oneshot(qb.map(axum::body::Body::new))
                        .with_subscriber(dispatch.clone())
                );
                (x.unwrap(), y.unwrap())
            };
            let mut successes = 0;
            let mut conflicts = 0;
            for response in [x, y] {
                match response.status().as_u16() {
                    200 => successes += 1,
                    401 => {}
                    409 => {
                        conflicts += 1;
                        assert_eq!(response.headers()["retry-after"], "1");
                        let bytes = axum::body::to_bytes(response.into_body(), 4096)
                            .await
                            .unwrap();
                        assert_eq!(
                            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["type"],
                            "urn:porchlight:problem:conflict"
                        );
                    }
                    status => panic!("cross revoke returned {status}"),
                }
            }
            logs.assert_only_nowait(conflicts);
            assert!(successes <= 1);
            let revoked:i64=sqlx::query_scalar("SELECT count(*) FROM porchlight.devices WHERE id IN ($1,$2) AND revoked_at IS NOT NULL").bind(a.device_id).bind(b.device_id).fetch_one(&pool).await.unwrap();
            assert_eq!(revoked, successes);
            let live:i64=sqlx::query_scalar("SELECT count(*) FROM porchlight.sessions WHERE device_id IN ($1,$2) AND revoked_at IS NULL").bind(a.device_id).bind(b.device_id).fetch_one(&pool).await.unwrap();
            assert_eq!(live, 2 - successes);
        }
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn locked_cross_revoke_target_returns_retryable_conflict_and_rolls_back_stepup(pool: PgPool) {
    use porchlight_api::{
        routes,
        server::{ConnInfo, Transport},
    };
    use tower::ServiceExt;
    let s = TestServer::start(&pool).await;
    let a = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let b = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let reauth = TestClient::new(&s.ca_pem, Some(&a))
        .request(
            s.lan,
            a.request(
                "POST",
                "/v1/auth/reauth",
                json!({"password":s.admin_password,"action":"device.revoke"}),
                None,
            ),
        )
        .await
        .unwrap();
    let stepup = reauth.json()["step_up_token"].as_str().unwrap().to_owned();
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM porchlight.devices WHERE id=$1 FOR UPDATE")
        .bind(b.device_id)
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    let conn = ConnInfo {
        transport: Transport::Lan,
        client_addr: None,
        client_cert_fingerprint: Some(a.fingerprint),
    };
    let router = routes::mtls_router(s.state.clone()).layer(axum::Extension(conn));
    let make_request = || {
        let mut req = a.request(
            "POST",
            &format!("/v1/devices/{}/revoke", b.device_id),
            json!({}),
            Some("locked-cross-revoke"),
        );
        req.headers_mut()
            .insert("porchlight-step-up", stepup.parse().unwrap());
        req.map(axum::body::Body::new)
    };
    let logs = SqlstateLogs::default();
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        router
            .clone()
            .oneshot(make_request())
            .with_subscriber(logs.dispatch()),
    )
    .await
    .expect("cross-target lock must be NOWAIT, not the five-second lock timeout")
    .unwrap();
    assert_eq!(response.status(), 409);
    assert_eq!(response.headers()["retry-after"], "1");
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["type"],
        "urn:porchlight:problem:conflict"
    );
    lock.rollback().await.unwrap();
    logs.assert_only_nowait(1);
    // Same key and same single-use step-up succeed once the target lock is released.
    assert_eq!(router.oneshot(make_request()).await.unwrap().status(), 200);
    s.stop().await;
}
