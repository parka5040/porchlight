use base64::{Engine, engine::general_purpose::STANDARD};
use blake2::{Blake2b512, Digest};
use porchlight_api::{
    config::Settings,
    outbound::Fetcher,
    release_watch::ReleaseWatch,
    testing::{HouseholdFixture, TestClient, TestDevice, TestOptions, TestServer, agent_request},
};
use porchlight_core::{VERSION, auth::session::Platform};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::PgPool;
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{Arc, RwLock},
    time::Duration,
};

struct SigningKey(Ed25519KeyPair);
impl SigningKey {
    fn new() -> Self {
        let key = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        Self(Ed25519KeyPair::from_pkcs8(key.as_ref()).unwrap())
    }
    fn public(&self) -> String {
        STANDARD.encode([b"Ed".as_slice(), &[7; 8], self.0.public_key().as_ref()].concat())
    }
    fn sign(&self, bytes: &[u8], legacy: bool) -> String {
        let hash = Blake2b512::digest(bytes);
        let signature = self.0.sign(if legacy { bytes } else { &hash });
        let packet = [
            if legacy {
                b"Ed".as_slice()
            } else {
                b"ED".as_slice()
            },
            &[7; 8],
            signature.as_ref(),
        ]
        .concat();
        let comment = "fixture release";
        let global = self
            .0
            .sign(&[signature.as_ref(), comment.as_bytes()].concat());
        format!(
            "untrusted comment: fixture\n{}\ntrusted comment: {comment}\n{}\n",
            STANDARD.encode(packet),
            STANDARD.encode(global.as_ref())
        )
    }
}
fn digest(bytes: &[u8]) -> Value {
    json!({"size": bytes.len(), "sha256": hex::encode(Sha256::digest(bytes))})
}
fn code(version: &str) -> u64 {
    let p: Vec<u64> = version.split('.').map(|p| p.parse().unwrap()).collect();
    p[0] * 10000 + p[1] * 100 + p[2]
}
struct Fixture {
    dir: tempfile::TempDir,
    key: SigningKey,
    manifest: Value,
    metadata: Value,
}
impl Fixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var");
        fs::create_dir_all(&root).unwrap();
        let dir = tempfile::tempdir_in(root).unwrap();
        let metadata = json!({
            "format_version": 1, "version": VERSION, "commit": null, "dirty": true,
            "source": {"github": "example-owner/porchlight"},
            "android": {"apk":"client.apk", "package":"app.porchlight", "version_code":code(VERSION), "signer_sha256":["ab".repeat(32)]},
            "desktop": {"archive":"desktop.tar.gz", "files":["desktop.tar.gz", "install.sh", "lib/operations.py", "release.json"]}
        });
        fs::create_dir(dir.path().join("lib")).unwrap();
        let members = BTreeMap::from([
            ("client.apk", b"0123456789".to_vec()),
            ("desktop.tar.gz", b"desktop fixture".to_vec()),
            ("install.sh", b"installer fixture".to_vec()),
            ("lib/operations.py", b"nested fixture".to_vec()),
            ("release.json", serde_json::to_vec(&metadata).unwrap()),
        ]);
        let mut files = BTreeMap::new();
        for (name, bytes) in members {
            fs::write(dir.path().join(name), &bytes).unwrap();
            files.insert(name, digest(&bytes));
        }
        let fixture = Self {
            dir,
            key: SigningKey::new(),
            metadata,
            manifest: json!({"format_version":1, "version":VERSION, "files":files}),
        };
        fixture.resign();
        fixture
    }
    fn resign(&self) {
        let bytes = serde_json::to_vec(&self.manifest).unwrap();
        fs::write(self.dir.path().join("manifest.json"), &bytes).unwrap();
        fs::write(
            self.dir.path().join("manifest.json.minisig"),
            self.key.sign(&bytes, false),
        )
        .unwrap();
    }
    fn write_metadata(&mut self) {
        let bytes = serde_json::to_vec(&self.metadata).unwrap();
        fs::write(self.dir.path().join("release.json"), &bytes).unwrap();
        self.manifest["files"]["release.json"] = digest(&bytes);
        self.resign();
    }
    fn settings(&self) -> Settings {
        let mut s = Settings::for_tests();
        s.release_dir = Some(self.dir.path().to_owned());
        s.release_public_key = Some(self.key.public());
        s
    }
    fn advertised(&self) -> Value {
        let mut names: Vec<String> = self.metadata["desktop"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        names.extend(["manifest.json".into(), "manifest.json.minisig".into()]);
        names.sort();
        let files: Vec<_> = names
            .iter()
            .map(|name| {
                let mut info = digest(&fs::read(self.dir.path().join(name)).unwrap());
                info["path"] = json!(name);
                info
            })
            .collect();
        let mut android = self.metadata["android"].clone();
        android.as_object_mut().unwrap().remove("apk");
        android["file"] = json!("client.apk");
        android["size"] = self.manifest["files"]["client.apk"]["size"].clone();
        android["sha256"] = self.manifest["files"]["client.apk"]["sha256"].clone();
        json!({"version":VERSION, "android":android, "desktop":{"archive":"desktop.tar.gz", "files":files}})
    }
}
async fn started(pool: &PgPool, settings: Settings) -> TestServer {
    let s = TestServer::start_with(
        pool,
        TestOptions {
            settings,
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while s.state.client_release.get().is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    s
}
async fn get(
    s: &TestServer,
    device: &TestDevice,
    path: &str,
) -> porchlight_api::testing::TestResponse {
    TestClient::new(&s.ca_pem, Some(device))
        .request(s.lan, device.request("GET", path, json!({}), None))
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../db/migrations")]
async fn absent_release_is_classified(pool: PgPool) {
    let s = started(&pool, Settings::for_tests()).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    for path in ["/v1/client-release", "/v1/client-release/files/client.apk"] {
        let r = get(&s, &d, path).await;
        assert_eq!(r.status, 404);
        assert_eq!(
            r.json()["type"],
            "urn:porchlight:problem:client-release-unavailable"
        );
    }
    s.stop().await;
}
#[sqlx::test(migrations = "../db/migrations")]
async fn advertised_bytes_ranges_allowlist_and_version_exemption(pool: PgPool) {
    let f = Fixture::new();
    let mut settings = f.settings();
    settings.min_client_version = "9.9.9".parse().unwrap();
    let s = started(&pool, settings).await;
    // No session: enrollment alone must suffice, including after a 426 on login.
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let r = get(&s, &d, "/v1/client-release").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json(), f.advertised());
    assert_eq!(get(&s, &d, "/v1/connection").await.status, 426);
    for path in [
        "/v1/client-release-other",
        "/v1/client-release/",
        "/v1/client-release/files",
        "/v1/server-release",
    ] {
        assert_eq!(get(&s, &d, path).await.status, 426, "{path}");
    }
    let path = "/v1/client-release/files/client.apk";
    let full = get(&s, &d, path).await;
    assert_eq!(full.status, 200);
    assert_eq!(&full.body[..], b"0123456789");
    let etag = format!(
        "\"{}\"",
        f.manifest["files"]["client.apk"]["sha256"]
            .as_str()
            .unwrap()
    );
    assert_eq!(full.headers["etag"], etag);
    assert_eq!(full.headers["cache-control"], "private, no-store");
    assert_eq!(full.headers["content-type"], "application/octet-stream");
    assert_eq!(full.headers["content-length"], "10");
    assert_eq!(full.headers["accept-ranges"], "bytes");
    for (range, if_range, status, body, content_range) in [
        (
            "bytes=2-4",
            Some(etag.as_str()),
            206,
            b"234".as_slice(),
            Some("bytes 2-4/10"),
        ),
        ("bytes=-3", None, 206, b"789", Some("bytes 7-9/10")),
        ("bytes=8-", None, 206, b"89", Some("bytes 8-9/10")),
        ("bytes=3-", Some("\"old\""), 200, b"0123456789", None),
        ("bytes=999-", Some("\"old\""), 200, b"0123456789", None),
    ] {
        let mut req = d.request("GET", path, json!({}), None);
        req.headers_mut().insert("range", range.parse().unwrap());
        if let Some(v) = if_range {
            req.headers_mut().insert("if-range", v.parse().unwrap());
        }
        let r = client.request(s.lan, req).await.unwrap();
        assert_eq!(r.status, status);
        assert_eq!(&r.body[..], body);
        assert_eq!(
            r.headers.get("content-range").map(|v| v.to_str().unwrap()),
            content_range
        );
    }
    for range in ["bytes=10-", "bytes=4-2", "bytes=-0", "bytes=1-2,4-5"] {
        let mut req = d.request("GET", path, json!({}), None);
        req.headers_mut().insert("range", range.parse().unwrap());
        let r = client.request(s.lan, req).await.unwrap();
        assert_eq!(r.status, 416);
        assert_eq!(r.headers["content-range"], "bytes */10");
    }
    for suffix in [
        "unlisted",
        "..",
        "../release.json",
        "/client.apk",
        "%2fclient.apk",
        "%2e%2e/release.json",
        "lib/%2e%2e/client.apk",
        "%252e%252e/client.apk",
    ] {
        assert_eq!(
            get(&s, &d, &format!("/v1/client-release/files/{suffix}"))
                .await
                .status,
            404,
            "{suffix}"
        );
    }
    for name in [
        "lib/operations.py",
        "manifest.json",
        "manifest.json.minisig",
        "release.json",
    ] {
        let r = get(&s, &d, &format!("/v1/client-release/files/{name}")).await;
        assert_eq!(r.status, 200);
        assert_eq!(&r.body[..], fs::read(f.dir.path().join(name)).unwrap());
    }
    for path in ["/v1/client-release", path] {
        for version in [None, Some("v1.0.0"), Some("01.0.0")] {
            let mut req = d.request("GET", path, json!({}), None);
            req.headers_mut().remove("porchlight-client-version");
            if let Some(v) = version {
                req.headers_mut()
                    .insert("porchlight-client-version", v.parse().unwrap());
            }
            assert_eq!(client.request(s.lan, req).await.unwrap().status, 400);
        }
    }
    // Changed files are never served with the verified checksum, even after advertisement.
    fs::write(f.dir.path().join("client.apk"), b"XXXXXXXXXX").unwrap();
    assert_eq!(get(&s, &d, path).await.status, 404);
    s.stop().await;
}

async fn unavailable(pool: PgPool, change: impl FnOnce(&mut Fixture)) {
    let mut f = Fixture::new();
    change(&mut f);
    let s = started(&pool, f.settings()).await;
    let d = s
        .issue_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let r = get(&s, &d, "/v1/client-release").await;
    assert_eq!(r.status, 404);
    assert_eq!(
        r.json()["type"],
        "urn:porchlight:problem:client-release-unavailable"
    );
    s.stop().await;
}
macro_rules! refused {
    ($name:ident, $change:expr) => {
        #[sqlx::test(migrations = "../db/migrations")]
        async fn $name(pool: PgPool) {
            unavailable(pool, $change).await;
        }
    };
}
refused!(tampered_file, |f| {
    fs::write(f.dir.path().join("client.apk"), b"XXXXXXXXXX").unwrap();
});
refused!(wrong_size, |f| {
    f.manifest["files"]["client.apk"]["size"] = json!(11);
    f.resign();
});
refused!(symlink_file, |f| {
    fs::remove_file(f.dir.path().join("client.apk")).unwrap();
    std::os::unix::fs::symlink("install.sh", f.dir.path().join("client.apk")).unwrap();
});
refused!(symlink_parent, |f| {
    fs::rename(f.dir.path().join("lib"), f.dir.path().join("actual-lib")).unwrap();
    std::os::unix::fs::symlink("actual-lib", f.dir.path().join("lib")).unwrap();
});
refused!(non_regular_file, |f| {
    fs::remove_file(f.dir.path().join("client.apk")).unwrap();
    fs::create_dir(f.dir.path().join("client.apk")).unwrap();
});
refused!(metadata_hash_mismatch, |f| {
    fs::write(f.dir.path().join("release.json"), b"{}").unwrap();
});
refused!(metadata_not_a_member, |f| {
    f.manifest["files"]
        .as_object_mut()
        .unwrap()
        .remove("release.json");
    f.resign();
});
refused!(metadata_version_mismatch, |f| {
    f.metadata["version"] = json!("99.0.0");
    f.write_metadata();
});
refused!(manifest_version_mismatch, |f| {
    f.manifest["version"] = json!("99.0.0");
    f.resign();
});
refused!(bad_signature, |f| {
    fs::write(f.dir.path().join("manifest.json.minisig"), "invalid").unwrap();
});
refused!(wrong_signature_key, |f| {
    let bytes = fs::read(f.dir.path().join("manifest.json")).unwrap();
    fs::write(
        f.dir.path().join("manifest.json.minisig"),
        SigningKey::new().sign(&bytes, false),
    )
    .unwrap();
});
refused!(legacy_signature, |f| {
    let bytes = fs::read(f.dir.path().join("manifest.json")).unwrap();
    fs::write(
        f.dir.path().join("manifest.json.minisig"),
        f.key.sign(&bytes, true),
    )
    .unwrap();
});
refused!(unlisted_advertised_file, |f| {
    f.metadata["desktop"]["files"]
        .as_array_mut()
        .unwrap()
        .push(json!("unlisted"));
    f.write_metadata();
});
refused!(traversal_manifest_member, |f| {
    f.manifest["files"]["../client.apk"] = digest(b"0123456789");
    f.resign();
});
refused!(absolute_manifest_member, |f| {
    f.manifest["files"]["/client.apk"] = digest(b"0123456789");
    f.resign();
});

#[sqlx::test(migrations = "../db/migrations")]
async fn role_and_surface_access(pool: PgPool) {
    let release = Fixture::new();
    let mut settings = release.settings();
    settings.upgrade_command = Some("/srv/data/porchlight-upgrade".into());
    let f = HouseholdFixture::build_with(
        &pool,
        TestOptions {
            settings,
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let s = &f.server;
    for user in [&f.admin, &f.users[0], &f.users[1]] {
        assert_eq!(get(s, &user.device, "/v1/client-release").await.status, 200);
        assert_eq!(
            get(s, &user.device, "/v1/client-release/files/client.apk")
                .await
                .status,
            200
        );
        assert_eq!(get(s, &user.device, "/v1/admin/release").await.status, 404);
    }
    assert_eq!(
        get(s, &f.users[0].device, "/v1/server-release")
            .await
            .status,
        403
    );
    let r = get(s, &f.admin.device, "/v1/server-release").await;
    assert_eq!(r.status, 200);
    let expected = json!({"running":VERSION,"latest":null,"update_available":false,"checked_at":null,"last_error":null,"upgrade_command":"/srv/data/porchlight-upgrade"});
    assert_eq!(r.json(), expected);
    for path in [
        "/v1/client-release",
        "/v1/client-release/files/client.apk",
        "/v1/server-release",
    ] {
        let r = agent_request(
            &s.agent_socket,
            Some(s.bootstrap_token()),
            f.admin.device.request("GET", path, json!({}), None),
        )
        .await;
        assert_eq!(r.status, 404);
    }
    let r = agent_request(
        &s.agent_socket,
        Some(s.bootstrap_token()),
        f.admin
            .device
            .request("GET", "/v1/admin/release", json!({}), None),
    )
    .await;
    assert_eq!(r.status, 200);
    let mut expected = expected;
    expected["client_release"] = json!({"version":VERSION,"android_version_code":code(VERSION)});
    assert_eq!(r.json(), expected);
    for scope in ["read", "write"] {
        let (_, token) = porchlight_api::testing::trust::seed_pat(&pool, s, &[scope]).await;
        let r = agent_request(
            &s.agent_socket,
            Some(&token),
            f.admin
                .device
                .request("GET", "/v1/admin/release", json!({}), None),
        )
        .await;
        assert_eq!(r.status, 403);
    }
    f.server.stop().await;
}

#[tokio::test]
async fn watch_verifies_before_adoption_and_keeps_previous_on_failure() {
    use axum::{
        Router,
        extract::{Path as AxumPath, State},
        http::StatusCode,
        routing::get,
    };
    let key = SigningKey::new();
    let assets = Arc::new(RwLock::new(BTreeMap::<String, String>::new()));
    let app = Router::new()
        .route(
            "/{file}",
            get(
                async |State(assets): State<Arc<RwLock<BTreeMap<String, String>>>>,
                       AxumPath(file): AxumPath<String>| {
                    match assets.read().unwrap().get(&file) {
                        Some(bytes) => (
                            StatusCode::OK,
                            [("content-type", "application/json")],
                            bytes.clone(),
                        ),
                        None => (
                            StatusCode::NOT_FOUND,
                            [("content-type", "text/plain")],
                            "missing".into(),
                        ),
                    }
                },
            ),
        )
        .with_state(assets.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let tls = rustls::ClientConfig::builder()
        .with_root_certificates(rustls::RootCertStore::empty())
        .with_no_client_auth();
    let watch = ReleaseWatch::local_test(Fetcher::local_test(tls), base, key.public());
    let publish = |version: &str| {
        let manifest =
            json!({"format_version":1, "version":version, "files":{"release.json":digest(b"{}")}})
                .to_string();
        let mut assets = assets.write().unwrap();
        assets.insert(
            "manifest.json.minisig".into(),
            key.sign(manifest.as_bytes(), false),
        );
        assets.insert("manifest.json".into(), manifest);
    };
    assert!(watch.status(None).await.latest.is_none());
    publish("99.0.0");
    watch.check_for_test().await;
    let good = watch.status(None).await;
    assert_eq!(good.latest.unwrap().version, "99.0.0");
    assert!(good.update_available && good.checked_at.is_some() && good.last_error.is_none());
    publish("100.0.0");
    assets
        .write()
        .unwrap()
        .insert("manifest.json.minisig".into(), "invalid".into());
    watch.check_for_test().await;
    let failed = watch.status(None).await;
    assert_eq!(failed.latest.unwrap().version, "99.0.0");
    assert_eq!(failed.last_error.as_deref(), Some("signature"));
    assets.write().unwrap().clear();
    watch.check_for_test().await;
    let failed = watch.status(None).await;
    assert_eq!(failed.latest.unwrap().version, "99.0.0");
    assert_eq!(failed.last_error.as_deref(), Some("http_status"));
    for version in ["0.0.0", VERSION] {
        publish(version);
        watch.check_for_test().await;
        let status = watch.status(None).await;
        assert_eq!(status.latest.unwrap().version, version);
        assert!(!status.update_available);
        assert!(status.last_error.is_none());
    }
    for invalid in [
        "01.0.0",
        "1.0.0-beta",
        "v1.0.0",
        "1.0",
        "18446744073709551616.0.0",
    ] {
        publish(invalid);
        watch.check_for_test().await;
        let status = watch.status(None).await;
        assert_eq!(status.latest.unwrap().version, VERSION);
        assert_eq!(status.last_error.as_deref(), Some("version"));
    }
    task.abort();
}
