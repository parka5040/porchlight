use crate::common;
use common::*;
use futures_util::StreamExt;
use porchlight_api::testing::{TestClient, TestOptions, TestServer, proxy_v2_header};
use porchlight_core::auth::session::Platform;
use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;

struct ReleaseFixture {
    dir: tempfile::TempDir,
    key: String,
}
impl ReleaseFixture {
    fn new() -> Self {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use blake2::{Blake2b512, Digest};
        use ring::signature::{Ed25519KeyPair, KeyPair};
        use std::{collections::BTreeMap, fs};
        let dir = tempfile::tempdir_in(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var"),
        )
        .unwrap();
        let version = porchlight_core::VERSION;
        let parts: Vec<u64> = version.split('.').map(|p| p.parse().unwrap()).collect();
        let metadata = json!({"format_version":1,"version":version,
            "android":{"apk":"client.apk","package":"app.porchlight","version_code":parts[0]*10000+parts[1]*100+parts[2],"signer_sha256":["ab".repeat(32)]},
            "desktop":{"archive":"desktop.tar.gz","files":["desktop.tar.gz","release.json"]}});
        let mut files = BTreeMap::new();
        for (name, bytes) in [
            ("client.apk", b"0123456789".to_vec()),
            ("desktop.tar.gz", b"archive fixture".to_vec()),
            ("release.json", serde_json::to_vec(&metadata).unwrap()),
        ] {
            fs::write(dir.path().join(name), &bytes).unwrap();
            files.insert(
                name,
                json!({"size":bytes.len(),"sha256":hex::encode(sha2::Sha256::digest(&bytes))}),
            );
        }
        let manifest =
            serde_json::to_vec(&json!({"format_version":1,"version":version,"files":files}))
                .unwrap();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let key = STANDARD.encode([b"Ed".as_slice(), &[7; 8], pair.public_key().as_ref()].concat());
        let signature = pair.sign(&Blake2b512::digest(&manifest));
        let global = pair.sign(&[signature.as_ref(), b"fixture"].concat());
        let signature = format!(
            "untrusted comment: fixture\n{}\ntrusted comment: fixture\n{}\n",
            STANDARD.encode([b"ED".as_slice(), &[7; 8], signature.as_ref()].concat()),
            STANDARD.encode(global.as_ref())
        );
        fs::write(dir.path().join("manifest.json"), manifest).unwrap();
        fs::write(dir.path().join("manifest.json.minisig"), signature).unwrap();
        Self { dir, key }
    }
    async fn start(&self, pool: &PgPool) -> TestServer {
        let mut settings = porchlight_api::config::Settings::for_tests();
        settings.release_dir = Some(self.dir.path().into());
        settings.release_public_key = Some(self.key.clone());
        let s = TestServer::start_with(
            pool,
            TestOptions {
                settings,
                ..Default::default()
            },
        )
        .await;
        porchlight_api::release::initialize(&s.state).await;
        s
    }
}

#[sqlx::test(migrations = "../db/migrations")]
async fn release_ranges_precede_hash_and_unchanged_files_are_cached(pool: PgPool) {
    let fixture = ReleaseFixture::new();
    let s = fixture.start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let path = "/v1/client-release/files/client.apk";
    for _ in 0..3 {
        let mut req = d.request("GET", path, json!({}), None);
        req.headers_mut()
            .insert("range", "bytes=0-0".parse().unwrap());
        assert_eq!(client.request(s.lan, req).await.unwrap().status, 206);
    }
    let store = s.state.client_release.get().unwrap().as_ref().unwrap();
    assert!(
        store
            .verification_starts
            .load(std::sync::atomic::Ordering::SeqCst)
            <= 1,
        "unchanged artifacts must not be rehashed for each range"
    );
    std::fs::write(fixture.dir.path().join("client.apk"), b"abcdefghij").unwrap();
    let before = store
        .verification_starts
        .load(std::sync::atomic::Ordering::SeqCst);
    let mut req = d.request("GET", path, json!({}), None);
    req.headers_mut()
        .insert("range", "bytes=999-".parse().unwrap());
    assert_eq!(client.request(s.lan, req).await.unwrap().status, 416);
    assert_eq!(
        store
            .verification_starts
            .load(std::sync::atomic::Ordering::SeqCst),
        before
    );
    assert_eq!(
        get(&s, &d, path).await.status,
        404,
        "changed artifact must be verified and refused"
    );
    // Atomic replacement with the same length/mtime still invalidates the inode cache key.
    let original = fixture.dir.path().join("client.apk");
    std::fs::write(&original, b"0123456789").unwrap();
    assert_eq!(get(&s, &d, path).await.status, 200);
    let replacement = fixture.dir.path().join("replacement");
    std::fs::write(&replacement, b"abcdefghij").unwrap();
    let modified = std::fs::metadata(&original).unwrap().modified().unwrap();
    std::fs::File::options()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    std::fs::rename(replacement, original).unwrap();
    assert_eq!(get(&s, &d, path).await.status, 404);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn release_download_body_and_hash_admission_are_bounded(pool: PgPool) {
    use axum::{
        extract::{Path, State},
        http::HeaderMap,
        response::IntoResponse,
    };
    use porchlight_api::{extract::DeviceAuth, routes::release::download};
    let fixture = ReleaseFixture::new();
    let s = fixture.start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let identity = || async {
        let identity = porchlight_db::auth::device_by_fingerprint(&pool, &d.fingerprint)
            .await
            .unwrap()
            .unwrap();
        DeviceAuth(identity, uuid::Uuid::now_v7())
    };
    let response = || async {
        download(
            State(s.state.clone()),
            identity().await,
            Path("client.apk".to_owned()),
            HeaderMap::new(),
        )
        .await
        .unwrap_or_else(IntoResponse::into_response)
    };
    let a = response().await;
    let b = response().await;
    assert_eq!(a.status(), 200);
    assert_eq!(b.status(), 200);
    let rejected = response().await;
    assert_eq!(
        rejected.status(),
        429,
        "download body must retain the per-device permit"
    );
    assert!(rejected.headers().contains_key("retry-after"));
    drop(a);
    assert_eq!(response().await.status(), 200);
    drop(b);
    let slots = s
        .state
        .release_hash_slots
        .clone()
        .acquire_many_owned(2)
        .await
        .unwrap();
    let rejected = response().await;
    assert_eq!(
        rejected.status(),
        429,
        "hash jobs must not queue beyond two admitted workers"
    );
    drop(slots);
    assert_eq!(response().await.status(), 200);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn cancelled_downloads_retain_hash_and_device_permits_until_work_finishes(pool: PgPool) {
    use axum::{
        extract::{Path, State},
        http::HeaderMap,
        response::IntoResponse,
    };
    use porchlight_api::{extract::DeviceAuth, routes::release::download};
    use std::sync::{Arc, Condvar, Mutex};
    struct Gate(Arc<(Mutex<bool>, Condvar)>);
    impl Drop for Gate {
        fn drop(&mut self) {
            *self.0.0.lock().unwrap() = true;
            self.0.1.notify_all();
        }
    }
    let fixture = ReleaseFixture::new();
    let s = fixture.start(&pool).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let gate = Gate(Arc::new((Mutex::new(false), Condvar::new())));
    let (started, wait) = std::sync::mpsc::channel();
    *s.state.release_open_hook.lock().unwrap() = Some(Arc::new({
        let gate = gate.0.clone();
        move || {
            started.send(()).unwrap();
            let _held = gate
                .1
                .wait_while(gate.0.lock().unwrap(), |open| !*open)
                .unwrap();
        }
    }));
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let identity = porchlight_db::auth::device_by_fingerprint(&pool, &d.fingerprint)
            .await
            .unwrap()
            .unwrap();
        let state = s.state.clone();
        tasks.push(tokio::spawn(async move {
            download(
                State(state),
                DeviceAuth(identity, uuid::Uuid::now_v7()),
                Path("client.apk".into()),
                HeaderMap::new(),
            )
            .await
        }));
    }
    tokio::task::spawn_blocking(move || {
        for _ in 0..2 {
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    })
    .await
    .unwrap();
    for task in tasks {
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    }
    assert_eq!(
        s.state.release_hash_slots.available_permits(),
        0,
        "cancelled handlers must not free active hashing slots"
    );
    *s.state.release_open_hook.lock().unwrap() = None;
    let identity = porchlight_db::auth::device_by_fingerprint(&pool, &d.fingerprint)
        .await
        .unwrap()
        .unwrap();
    let rejected = download(
        State(s.state.clone()),
        DeviceAuth(identity, uuid::Uuid::now_v7()),
        Path("client.apk".into()),
        HeaderMap::new(),
    )
    .await
    .unwrap_or_else(IntoResponse::into_response);
    assert_eq!(rejected.status(), 429);
    drop(gate);
    tokio::time::timeout(Duration::from_secs(5), async {
        while s.state.release_hash_slots.available_permits() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        get(&s, &d, "/v1/client-release/files/client.apk")
            .await
            .status,
        200
    );
    s.stop().await;
}

async fn recover(pool: &PgPool, member: &str, days: &str) -> std::process::Output {
    use sqlx::{ConnectOptions, postgres::PgConnectOptions};
    let database = pool.connect_options().get_database().unwrap().to_owned();
    let app: PgConnectOptions = std::env::var("PORCHLIGHT_DATABASE_URL")
        .unwrap()
        .parse()
        .unwrap();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_porchlight"));
    command
        .args([
            "admin-token",
            "--member",
            member,
            "--name",
            "recovery fixture",
            "--expires-days",
            days,
        ])
        .env(
            "PORCHLIGHT_DATABASE_URL",
            app.database(&database).to_url_lossy().as_str(),
        )
        .env_remove("PORCHLIGHT_MIGRATOR_URL");
    tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn local_admin_recovery_without_a_live_pat(pool: PgPool) {
    use porchlight_api::testing::{agent_request, trust::req};
    let s = TestServer::start(&pool).await;
    sqlx::query("UPDATE porchlight.api_tokens SET expires_at=now()-interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    let expired = agent_request(
        &s.agent_socket,
        Some(s.bootstrap_token()),
        req("GET", "/v1/admin/whoami", json!({}), "expired"),
    )
    .await;
    assert_eq!(expired.status, 401);
    let output = recover(&pool, &s.init.admin_member_id.to_string(), "90").await;
    assert!(
        output.status.success(),
        "owner-run admin-token must recover without any live PAT using the app DB role"
    );
    let token = String::from_utf8(output.stdout).unwrap();
    assert_eq!(token.lines().count(), 1);
    let token = token.trim();
    assert!(token.starts_with("plp_"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(token));
    let who = agent_request(
        &s.agent_socket,
        Some(token),
        req("GET", "/v1/admin/whoami", json!({}), "recovered"),
    )
    .await;
    assert_eq!(who.status, 200);
    assert_eq!(who.json()["scopes"], json!(["admin"]));
    let enrollment = agent_request(
        &s.agent_socket,
        Some(token),
        req(
            "POST",
            "/v1/admin/enrollment-tokens",
            json!({"username":"admin","device_label":"Recovery fixture"}),
            "recovered-enrollment",
        ),
    )
    .await;
    assert_eq!(enrollment.status, 201);
    let attributed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM porchlight.outbox WHERE entity_id=$1 AND kind='token.created' AND payload->'actor'->>'application_name'='agent:local-recovery')")
        .bind(uuid::Uuid::parse_str(who.json()["token_id"].as_str().unwrap()).unwrap()).fetch_one(&pool).await.unwrap();
    assert!(attributed, "local recovery must attribute its PAT creation");
    assert!(
        recover(&pool, "admin", "45").await.status.success(),
        "username lookup is supported"
    );
    let member = porchlight_api::testing::trust::user(&pool, &s).await;
    for (selector, days) in [
        (member.to_string(), "90"),
        (uuid::Uuid::now_v7().to_string(), "90"),
        ("admin".into(), "0"),
    ] {
        let rejected = recover(&pool, &selector, days).await;
        assert!(!rejected.status.success());
        assert!(
            rejected.stdout.is_empty(),
            "failed recovery must not print a token"
        );
    }
    sqlx::query("UPDATE porchlight.users SET deleted_at=now() WHERE username='admin'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(!recover(&pool, "admin", "90").await.status.success());
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn admin_recovery_requires_initialized_household(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    assert!(recover(&pool, "admin", "90").await.status.success());
    sqlx::query("DELETE FROM porchlight.install_state")
        .execute(&pool)
        .await
        .unwrap();
    let rejected = recover(&pool, "admin", "90").await;
    assert!(
        !rejected.status.success(),
        "local recovery must reject an uninitialized household with a live super admin"
    );
    assert!(
        rejected.stdout.is_empty(),
        "failed recovery must not print a token"
    );
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn anonymous_saturation_does_not_block_device_login(pool: PgPool) {
    use porchlight_api::ratelimit::{Class, RateLimits};
    use std::sync::{Arc, Mutex};
    let now = std::time::Instant::now();
    let clock = Arc::new(Mutex::new(now));
    let read_clock = clock.clone();
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            limits: RateLimits::production(),
            rate_clock: Arc::new(move || *read_clock.lock().unwrap()),
            ..Default::default()
        },
    )
    .await;
    let mut device = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let prefix = |n: u16| std::net::Ipv6Addr::new(0x2001, 0xdb8, n, 0, 0, 0, 0, 1).into();
    // Use the enrollment middleware's admission path without needing thousands of local IPs.
    let admitted: Vec<_> = (0..4096)
        .filter(|&n| {
            s.state
                .limits
                .check(Class::Auth, None, Some(prefix(n)))
                .is_ok()
        })
        .collect();
    assert!(!admitted.is_empty());
    *clock.lock().unwrap() = now + Duration::from_secs(119);
    for n in admitted {
        assert!(
            s.state
                .limits
                .check(Class::Auth, None, Some(prefix(n)))
                .is_ok()
        );
    }
    assert!(
        s.state
            .limits
            .check(Class::Auth, None, Some(prefix(4096)))
            .is_err()
    );
    // Stay saturated past the original entries' idle deadline: refreshed entries are still live.
    *clock.lock().unwrap() = now + Duration::from_secs(121);
    for _ in 0..5 {
        assert_eq!(
            s.login(&mut device, &s.admin_password).await.status,
            200,
            "anonymous enrollment saturation must not deny an authenticated device login"
        );
    }
    let limited = s.login(&mut device, &s.admin_password).await;
    assert_eq!(
        limited.status, 429,
        "device throttling must remain enforced"
    );
    assert!(limited.headers.contains_key("retry-after"));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn token_list_surfaces_expiry_and_recovery_health(pool: PgPool) {
    use porchlight_api::testing::trust::admin;
    let s = TestServer::start(&pool).await;
    sqlx::query("UPDATE porchlight.api_tokens SET expires_at=now()+interval '29 days'")
        .execute(&pool)
        .await
        .unwrap();
    let response = admin(&s, "GET", "/v1/admin/tokens", json!({})).await;
    assert_eq!(
        response.status, 200,
        "admin token list must expose expiry and rotation health"
    );
    let body = response.json();
    assert!(body["tokens"][0]["expires_at"].is_string());
    assert_eq!(body["admin_token_health"]["expiring_soon"], true);
    assert_eq!(body["admin_token_health"]["recovery_required"], false);
    assert!(body["tokens"][0].get("token_hash").is_none());
    assert!(body["tokens"][0].get("token").is_none());
    for scope in ["read", "write"] {
        let (_, token) = porchlight_api::testing::trust::seed_pat(&pool, &s, &[scope]).await;
        let response = porchlight_api::testing::agent_request(
            &s.agent_socket,
            Some(&token),
            porchlight_api::testing::trust::req("GET", "/v1/admin/tokens", json!({}), "list-scope"),
        )
        .await;
        assert_eq!(response.status, 403);
    }
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    assert_eq!(get(&s, &d, "/v1/admin/tokens").await.status, 404);
    use tracing::instrument::WithSubscriber;
    let sink = WarnSink::default();
    porchlight_api::admin_recovery::warn(&s.state.pool)
        .with_subscriber(sink.dispatch())
        .await;
    assert!(sink.text().contains("WARN") && sink.text().contains("admin-token-expiring"));
    assert!(!sink.text().contains(s.bootstrap_token()));
    assert!(recover(&pool, "admin", "90").await.status.success());
    let body = admin(&s, "GET", "/v1/admin/tokens", json!({})).await.json();
    assert_eq!(body["admin_token_health"]["expiring_soon"], false);
    // Non-admin tokens cannot hide the absence of a live admin PAT.
    sqlx::query("UPDATE porchlight.api_tokens SET revoked_at=now() WHERE 'admin'=ANY(scopes)")
        .execute(&pool)
        .await
        .unwrap();
    let health = porchlight_api::admin_recovery::health(&s.state.pool)
        .await
        .unwrap();
    assert!(health.recovery_required);
    assert!(health.newest_live_expires_at.is_none());
    let sink = WarnSink::default();
    porchlight_api::admin_recovery::warn(&s.state.pool)
        .with_subscriber(sink.dispatch())
        .await;
    assert!(sink.text().contains("admin-token-missing"));
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn password_change_voids_outstanding_reset(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    let a = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let mut b = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let user = sqlx::query_scalar("SELECT user_id FROM porchlight.household_members WHERE id=$1")
        .bind(s.init.admin_member_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let code = s.reset_code(user).await;
    let new_password = "a changed household password";
    let changed = post(
        &s,
        &a,
        "/v1/auth/password",
        json!({"current_password":s.admin_password,"new_password":new_password}),
    )
    .await;
    assert_eq!(changed.status, 204);
    problem(&get(&s, &b, "/v1/me").await, 401, "session-invalid");
    b.session = None;
    let reset = post(
        &s,
        &b,
        "/v1/auth/reset-password",
        json!({"code":code,"new_password":"an obsolete recovery password"}),
    )
    .await;
    assert_eq!(
        reset.status, 401,
        "password change must void the old reset code"
    );
    problem(&reset, 401, "reset-code-invalid");
    assert_eq!(s.login(&mut b, new_password).await.status, 200);
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn stream_quotas_on_lan_and_funnel(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            max_connections: 16,
            ..Default::default()
        },
    )
    .await;
    let other = porchlight_api::testing::trust::user(&pool, &s).await;
    let b = s.session_device(other, Platform::Desktop).await;
    for addr in [s.lan, s.funnel] {
        let prefix = if addr == s.funnel {
            proxy_v2_header("192.168.77.7:4444".parse().unwrap(), addr)
        } else {
            Vec::new()
        };
        let mut devices = Vec::new();
        for _ in 0..3 {
            devices.push(
                s.session_device(s.init.admin_member_id, Platform::Desktop)
                    .await,
            );
        }
        let mut streams = Vec::new();
        for (i, d) in devices.iter().enumerate() {
            let client = TestClient::new(&s.ca_pem, Some(d));
            for _ in 0..if i == 2 { 2 } else { 3 } {
                let mut stream = client
                    .stream_with_prefix(
                        addr,
                        &prefix,
                        d.request("GET", "/v1/stream", json!({}), None),
                    )
                    .await
                    .unwrap();
                assert_eq!(stream.status, 200);
                let ready = tokio::time::timeout(Duration::from_secs(3), stream.next())
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(ready.event.as_deref(), Some("ready"));
                streams.push(stream);
            }
            let rejected = client
                .stream_with_prefix(
                    addr,
                    &prefix,
                    d.request("GET", "/v1/stream", json!({}), None),
                )
                .await
                .unwrap();
            assert_eq!(
                rejected.status, 429,
                "device/member stream budget must reject excess streams"
            );
            assert!(rejected.headers.contains_key("retry-after"));
            assert_eq!(rejected.headers["connection"], "close");
        }
        // A different member still has ordinary and stream access.
        let client = TestClient::new(&s.ca_pem, Some(&b));
        assert_eq!(
            client
                .request_with_prefix(
                    addr,
                    &prefix,
                    b.request("GET", "/v1/connection", json!({}), None)
                )
                .await
                .unwrap()
                .status,
            200
        );
        let stream = client
            .stream_with_prefix(
                addr,
                &prefix,
                b.request("GET", "/v1/stream", json!({}), None),
            )
            .await
            .unwrap();
        assert_eq!(stream.status, 200);
        drop(stream);
        streams.clear();
        // Disconnect cancellation releases both quotas (including cross-listener counts).
        let d = &devices[0];
        let client = TestClient::new(&s.ca_pem, Some(d));
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let stream = client
                    .stream_with_prefix(
                        addr,
                        &prefix,
                        d.request("GET", "/v1/stream", json!({}), None),
                    )
                    .await
                    .unwrap();
                if stream.status == 200 {
                    break;
                }
                assert_eq!(stream.status, 429);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    s.stop().await;
}
