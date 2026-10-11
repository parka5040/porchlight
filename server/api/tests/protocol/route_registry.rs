use axum::{body::Body, http::Request};
use porchlight_api::{
    routes::{self, ROUTES},
    testing::TestServer,
};
use sqlx::PgPool;
use tower::ServiceExt;
#[sqlx::test(migrations = "../db/migrations")]
async fn every_registered_mutation_requires_key(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    for route in ROUTES {
        if route.method == "GET" {
            continue;
        }
        let router = routes::registry_router(route.surface).with_state(s.state.clone());
        let path = route
            .path
            .replace("{id}", &uuid::Uuid::now_v7().to_string())
            .replace("{username}", "admin");
        let r = router
            .oneshot(
                Request::builder()
                    .method(route.method)
                    .uri(path)
                    .header("porchlight-client-version", porchlight_core::VERSION)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), 400, "{} {}", route.method, route.path);
        let body = axum::body::to_bytes(r.into_body(), 4096).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value["type"],
            "urn:porchlight:problem:idempotency-key-required"
        );
    }
    s.stop().await;
}

#[sqlx::test(migrations = "../db/migrations")]
async fn representative_replays_are_byte_identical_on_every_surface(pool: PgPool) {
    use porchlight_api::testing::{TestClient, trust};
    use porchlight_core::auth::session::Platform;
    use serde_json::json;
    let s = TestServer::start(&pool).await;
    let d = s
        .session_device(s.init.admin_member_id, Platform::Desktop)
        .await;
    let client = TestClient::new(&s.ca_pem, Some(&d));
    let body = json!({"password":s.admin_password,"action":"device.revoke"});
    let a = client
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/auth/reauth",
                body.clone(),
                Some("replay-member"),
            ),
        )
        .await
        .unwrap();
    let b = client
        .request(
            s.lan,
            d.request("POST", "/v1/auth/reauth", body, Some("replay-member")),
        )
        .await
        .unwrap();
    assert_eq!(a.status, 200);
    assert_eq!(a.status, b.status);
    assert_eq!(a.body, b.body);
    assert_eq!(a.headers["content-type"], b.headers["content-type"]);
    let body = json!({"username":"admin","device_label":"Replay"});
    let a = trust::agent(
        &s,
        Some(s.bootstrap_token()),
        "POST",
        "/v1/admin/enrollment-tokens",
        body.clone(),
        "replay-agent",
    )
    .await;
    let b = trust::agent(
        &s,
        Some(s.bootstrap_token()),
        "POST",
        "/v1/admin/enrollment-tokens",
        body,
        "replay-agent",
    )
    .await;
    assert_eq!(a.status, 201);
    assert_eq!(a.status, b.status);
    assert_eq!(a.body, b.body);
    let token = a.json()["token"].as_str().unwrap().to_owned();
    let (csr, _) = porchlight_pki::generate_csr("Replay").unwrap();
    let a = trust::complete(&s, &token, &s.admin_password, &csr, "replay-enrollment").await;
    let b = trust::complete(&s, &token, &s.admin_password, &csr, "replay-enrollment").await;
    assert_eq!(a.status, 201);
    assert_eq!(a.status, b.status);
    assert_eq!(a.body, b.body);
    // Device-only (session-less login/reset) is a distinct legacy authentication class.
    let body = json!({"password":s.admin_password});
    let a = client
        .request(
            s.lan,
            d.request(
                "POST",
                "/v1/auth/login",
                body.clone(),
                Some("replay-device-only"),
            ),
        )
        .await
        .unwrap();
    let b = client
        .request(
            s.lan,
            d.request("POST", "/v1/auth/login", body, Some("replay-device-only")),
        )
        .await
        .unwrap();
    assert_eq!(a.status, 200);
    assert_eq!(a.status, b.status);
    assert_eq!(a.body, b.body);
    s.stop().await;
}

// Axum 0.8 exposes no public iterator. Inspect the actual final router's path
// and method maps via Debug, failing closed if the pinned version's format changes.
fn final_routes(router: &axum::Router) -> std::collections::BTreeSet<(String, String)> {
    let debug = format!("{router:?}");
    let path_router = debug.split(", fallback_router:").next().unwrap();
    let paths = path_router
        .split("node: Node { paths: {")
        .nth(1)
        .expect("axum Debug path map changed")
        .split("} }")
        .next()
        .unwrap()
        .trim();
    let paths: std::collections::BTreeMap<_, String> = paths
        .split(", ")
        .map(|entry| {
            let (id, path) = entry
                .split_once(": ")
                .expect("axum Debug path entry changed");
            (
                id,
                serde_json::from_str(path).expect("axum Debug path string changed"),
            )
        })
        .collect();
    assert!(!paths.is_empty(), "final path map must not be empty");
    let routes = path_router
        .split("routes: {")
        .nth(1)
        .unwrap()
        .split("}, node:")
        .next()
        .unwrap();
    let mut actual = std::collections::BTreeSet::new();
    for entry in routes.split("RouteId(").skip(1) {
        let (id, methods) = entry.split_once("): ").unwrap();
        let id = format!("RouteId({id})");
        let path = paths
            .get(id.as_str())
            .expect("method route absent from path map");
        assert!(
            methods.starts_with("MethodRouter(MethodRouter {"),
            "unregistered route service: {path}"
        );
        for method in [
            "get", "head", "post", "put", "patch", "delete", "options", "trace", "connect",
        ] {
            let endpoint = methods
                .split(&format!("{method}: "))
                .nth(1)
                .expect("axum Debug method map changed");
            if !endpoint.starts_with("None") {
                actual.insert((path.clone(), method.to_ascii_uppercase()));
            }
        }
    }
    actual
}
#[sqlx::test(migrations = "../db/migrations")]
async fn final_routers_have_exactly_the_registered_method_routes(pool: PgPool) {
    use routes::Surface;
    let s = TestServer::start(&pool).await;
    for (surface, router) in [
        (Surface::Mtls, routes::mtls_router(s.state.clone())),
        (Surface::Agent, routes::agent_router(s.state.clone())),
        (Surface::Enrollment, routes::enroll_router(s.state.clone())),
    ] {
        let expected: std::collections::BTreeSet<_> = ROUTES
            .iter()
            .filter(|r| r.surface == surface)
            .map(|r| (routes::mount_path(r.path).to_owned(), r.method.to_owned()))
            .collect();
        assert_eq!(final_routes(&router), expected);
        // Negative controls: a new path, an extra method, and a merged router
        // must all be visible, regardless of the registry-generated metadata.
        for extra in [
            router.clone().route(
                "/unregistered",
                axum::routing::get(|| async { "unregistered" }),
            ),
            router.clone().route(
                ROUTES
                    .iter()
                    .find(|r| {
                        r.surface == surface
                            && !ROUTES.iter().any(|other| {
                                other.surface == surface
                                    && other.path == r.path
                                    && other.method == "PUT"
                            })
                    })
                    .expect("negative control requires a path without PUT")
                    .path,
                axum::routing::put(|| async { "unregistered" }),
            ),
            router.merge(
                axum::Router::new()
                    .route("/merged", axum::routing::post(|| async { "unregistered" })),
            ),
        ] {
            assert_ne!(final_routes(&extra), expected);
        }
    }
    s.stop().await;
}

#[test]
fn legacy_adapters_are_explicitly_pinned() {
    let actual: Vec<_> = ROUTES.iter().filter(|r| r.legacy).map(|r| r.path).collect();
    assert_eq!(
        actual,
        vec![
            "/v1/auth/login",
            "/v1/auth/logout",
            "/v1/auth/password",
            "/v1/auth/reauth",
            "/v1/auth/reset-password",
            "/v1/devices/self/certificate",
            "/v1/devices/{id}/revoke",
            "/v1/enrollment/complete",
            "/v1/admin/enrollment-tokens",
            "/v1/admin/users/{username}/password-reset",
            "/v1/admin/devices/{id}/revoke",
            "/v1/admin/tokens",
            "/v1/admin/tokens/{id}/revoke"
        ]
    );
}
