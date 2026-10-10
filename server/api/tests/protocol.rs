use axum::{body::Body, http::Request};
use porchlight_api::{
    error::Problem,
    protocol::{ClientVersion, if_match, version_conflict},
    routes,
    testing::TestServer,
};
use sqlx::PgPool;
use tower::ServiceExt;

#[test]
fn versions_and_if_match_are_strict() {
    for valid in ["0.1.0", "1.20.3", "999.0.0"] {
        assert!(valid.parse::<ClientVersion>().is_ok());
    }
    for invalid in [
        "1",
        "1.2",
        "01.2.3",
        "1.2.3-beta",
        "1.2.3+build",
        "1.2.3.4",
        "-1.2.3",
        " 1.2.3",
    ] {
        assert!(invalid.parse::<ClientVersion>().is_err());
    }
    let mut h = axum::http::HeaderMap::new();
    assert_eq!(
        if_match(&h).unwrap_err().problem(),
        Problem::IfMatchRequired
    );
    for good in ["3", "\"3\""] {
        h.insert("if-match", good.parse().unwrap());
        assert_eq!(if_match(&h).unwrap(), 3);
    }
    for bad in ["W/\"3\"", "*", "3,4", "-1", "3.0", "\"3", ""] {
        h.insert("if-match", bad.parse().unwrap());
        assert_eq!(if_match(&h).unwrap_err().problem(), Problem::InvalidRequest);
    }
    assert_eq!(
        version_conflict(0, serde_json::json!({"version":4}))
            .unwrap_err()
            .problem(),
        Problem::VersionConflict
    );
    assert!(version_conflict(1, serde_json::json!({})).is_ok());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn headers_gate_every_router_including_fallbacks(pool: PgPool) {
    let s = TestServer::start(&pool).await;
    for router in [
        routes::mtls_router(s.state.clone()),
        routes::agent_router(s.state.clone()),
        routes::enroll_router(s.state.clone()),
    ] {
        for (version, tz, status, slug) in [
            (None, None, 400, "client-version-required"),
            (Some("bogus"), None, 400, "client-version-required"),
            (Some("0.0.9"), None, 426, "upgrade-required"),
            (Some("0.1.0"), Some("invalid-zone"), 404, "not-found"),
            (Some("0.1.0"), Some("America/New_York"), 404, "not-found"),
            (Some("0.1.0"), Some("US/Eastern"), 404, "not-found"),
            (Some("0.1.0"), Some("CET"), 404, "not-found"),
        ] {
            let mut req = Request::builder().uri("/missing");
            if let Some(v) = version {
                req = req.header("porchlight-client-version", v);
            }
            if let Some(tz) = tz {
                req = req.header("porchlight-tz", tz);
            }
            let r = router
                .clone()
                .oneshot(req.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(r.status().as_u16(), status);
            let body = axum::body::to_bytes(r.into_body(), 4096).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["type"], format!("urn:porchlight:problem:{slug}"));
            if status == 426 {
                assert_eq!(body["min_client_version"], "0.1.0");
            }
        }
    }
    for (router, path) in [
        (routes::mtls_router(s.state.clone()), "/v1/me"),
        (routes::agent_router(s.state.clone()), "/v1/admin/whoami"),
        (
            routes::enroll_router(s.state.clone()),
            "/v1/enrollment/complete",
        ),
    ] {
        for (version, status) in [
            (None, 400),
            (Some("0.0.9"), 426),
            (Some(porchlight_core::VERSION), 405),
        ] {
            let mut req = Request::builder().method("OPTIONS").uri(path);
            if let Some(v) = version {
                req = req.header("porchlight-client-version", v);
            }
            let response = router
                .clone()
                .oneshot(req.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), status);
        }
    }
    s.stop().await;
}

#[tokio::test]
async fn problem_extensions_carry_current_and_field_errors() {
    use axum::response::IntoResponse;
    use porchlight_api::error::{ApiError, FieldError};
    let response = version_conflict(0, serde_json::json!({"id":"resource","version":4}))
        .unwrap_err()
        .into_response();
    assert_eq!(response.status(), 409);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["current"]["version"], 4);
    let response = ApiError::validation(vec![FieldError {
        code: None,
        field: "title".into(),
        message: "required".into(),
    }])
    .into_response();
    assert_eq!(response.status(), 422);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["errors"],
        serde_json::json!([{"field":"title","message":"required"}])
    );
}

#[sqlx::test(migrations = "../db/migrations")]
async fn valid_time_zone_is_exposed_as_a_typed_extension(pool: PgPool) {
    use axum::{Extension, Json, Router, middleware, routing::get};
    use porchlight_api::{
        protocol::{self, ClientTz},
        state::AppState,
    };
    let s = TestServer::start(&pool).await;
    let router = Router::<AppState>::new()
        .route(
            "/tz",
            get(|Extension(ClientTz(tz)): Extension<ClientTz>| async move {
                Json(serde_json::json!({"tz":tz.map(|z|z.name())}))
            }),
        )
        .layer(middleware::from_fn_with_state(
            s.state.clone(),
            protocol::request_headers,
        ))
        .with_state(s.state.clone());
    for (tz, expected) in [
        (None, None),
        (Some("Asia/Kathmandu"), Some("Asia/Kathmandu")),
        (Some("US/Eastern"), Some("America/New_York")),
        (Some("CET"), Some("Europe/Brussels")),
        (Some("GMT"), Some("UTC")),
        (Some("Mars/Olympus_Mons"), None),
    ] {
        let mut req = Request::builder()
            .uri("/tz")
            .header("porchlight-client-version", porchlight_core::VERSION);
        if let Some(tz) = tz {
            req = req.header("porchlight-tz", tz);
        }
        let response = router
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["tz"], serde_json::json!(expected));
    }
    s.stop().await;
}

#[tokio::test]
async fn problem_extensions_cannot_forge_reserved_rfc_fields() {
    use axum::response::IntoResponse;
    let response = porchlight_api::error::ApiError::new(Problem::NotFound)
        .extension("type", serde_json::json!("forged-slug"))
        .extension("status", serde_json::json!(200))
        .into_response();
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["type"], "urn:porchlight:problem:not-found");
    assert_eq!(body["status"], 404);
}
