use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    middleware,
    response::IntoResponse,
};
use std::io::Read;
use tower::ServiceExt;

async fn response(
    path: &str,
    encoding: Option<&str>,
    content_type: &'static str,
    size: usize,
    method: &str,
    status: StatusCode,
) -> axum::response::Response {
    let app = Router::new()
        .fallback(move || async move {
            (
                status,
                [
                    ("content-type", content_type.to_owned()),
                    ("vary", "Origin".to_owned()),
                    ("content-length", size.to_string()),
                ],
                "x".repeat(size),
            )
                .into_response()
        })
        .layer(middleware::from_fn(porchlight_api::compression::json));
    let mut req = Request::builder().uri(path).method(method);
    if let Some(v) = encoding {
        req = req.header("accept-encoding", v);
    }
    app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap()
}
#[tokio::test]
async fn gzip_negotiation_and_boundary() {
    for (enc, yes) in [
        (None, false),
        (Some("br"), false),
        (Some("gzip"), true),
        (Some("GZip; q=0.5"), true),
        (Some("*;q=1"), true),
        (Some("gzip;q=0, *;q=1"), false),
        (Some("gzip;q=0.000"), false),
        (Some("gzip;q=0.001"), true),
        (Some("gzip;q=invalid"), false),
        (Some("gzip;q=0;q=1"), false),
        (Some("gzip;q=1.001"), false),
        (Some("gzip;q=0, gzip;q=1"), false),
    ] {
        for size in [8191, 8192] {
            let r = response(
                "/v1/snapshot",
                enc,
                "application/json",
                size,
                "GET",
                StatusCode::OK,
            )
            .await;
            let compressed = yes && size >= 8192;
            assert_eq!(
                r.headers().contains_key("content-encoding"),
                compressed,
                "encoding {enc:?} size {size}"
            );
            let vary = r
                .headers()
                .get_all("vary")
                .iter()
                .map(|v| v.to_str().unwrap())
                .collect::<Vec<_>>()
                .join(",");
            assert!(
                vary.contains("Origin") && vary.contains("Accept-Encoding"),
                "{vary}"
            );
            let bytes = to_bytes(r.into_body(), usize::MAX).await.unwrap();
            if compressed {
                let mut decoded = String::new();
                flate2::read::GzDecoder::new(bytes.as_ref())
                    .read_to_string(&mut decoded)
                    .unwrap();
                assert_eq!(decoded, "x".repeat(size));
            } else {
                assert_eq!(bytes.len(), size);
            }
        }
    }
}
#[tokio::test]
async fn gzip_excludes_secrets_downloads_streams_and_empty_statuses() {
    for path in [
        "/v1/feeds",
        "/v1/feeds/fixture",
        "/v1/admin/feeds",
        "/v1/admin/feeds/fixture",
        "/v1/auth/login",
        "/v1/auth/reauth",
        "/v1/enrollment/complete",
        "/v1/admin/enrollment-tokens",
        "/v1/admin/tokens",
        "/v1/admin/users/fixture/password-reset",
        "/v1/attachments/fixture",
        "/v1/attachments/fixture/thumbnail",
        "/v1/export/member",
        "/v1/admin/export/household",
        "/v1/backups/latest/download",
        "/v1/client-release/files/fixture",
    ] {
        let r = response(
            path,
            Some("gzip"),
            "application/json",
            9000,
            "GET",
            StatusCode::OK,
        )
        .await;
        assert!(!r.headers().contains_key("content-encoding"), "{path}");
    }
    for ct in [
        "text/event-stream",
        "application/octet-stream",
        "image/jpeg",
    ] {
        let r = response(
            "/v1/snapshot",
            Some("gzip"),
            ct,
            9000,
            "GET",
            StatusCode::OK,
        )
        .await;
        assert!(!r.headers().contains_key("content-encoding"));
    }
    for status in [StatusCode::NO_CONTENT, StatusCode::NOT_MODIFIED] {
        let r = response(
            "/v1/snapshot",
            Some("gzip"),
            "application/json",
            9000,
            "GET",
            status,
        )
        .await;
        assert!(!r.headers().contains_key("content-encoding"));
    }
    let r = response(
        "/v1/snapshot",
        Some("gzip"),
        "application/json",
        9000,
        "HEAD",
        StatusCode::OK,
    )
    .await;
    assert_eq!(r.headers()["content-encoding"], "gzip");
    assert!(!r.headers().contains_key("content-length"));
    let r = response(
        "/v1/items/fixture",
        Some("gzip"),
        "application/problem+json",
        9000,
        "GET",
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(r.headers()["content-encoding"], "gzip");
}

#[tokio::test]
async fn compressed_body_and_frames_retain_admission() {
    use porchlight_api::concurrency::{Limits, with_body};
    use std::sync::Arc;
    use uuid::Uuid;
    let limits = Arc::new(Limits::new(1, None));
    let device = Uuid::now_v7();
    let member = Uuid::now_v7();
    let l = limits.clone();
    let app = Router::new()
        .fallback(move || {
            let l = l.clone();
            async move {
                let permit = l.acquire(device, member).unwrap();
                with_body(
                    ([("content-type", "application/json")], "x".repeat(9000)).into_response(),
                    permit,
                )
            }
        })
        .layer(middleware::from_fn(porchlight_api::compression::json));
    let req = Request::builder()
        .uri("/v1/snapshot")
        .header("accept-encoding", "gzip")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.headers()["content-encoding"], "gzip");
    assert!(
        limits.acquire(device, member).is_err(),
        "compressed unread body must retain admission"
    );
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(
        limits.acquire(device, member).is_err(),
        "retained compressed frame must retain admission"
    );
    drop(bytes);
    assert!(limits.acquire(device, member).is_ok());
}

#[sqlx::test(migrations = "../db/migrations")]
async fn gzip_idempotency_replays_negotiate_per_request(pool: sqlx::PgPool) {
    use porchlight_api::{
        routes,
        server::{ConnInfo, Transport},
        testing::{TestOptions, TestServer},
    };
    use serde_json::{Value, json};
    use uuid::Uuid;
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    let key = Uuid::now_v7().to_string();
    let payload =
        json!({"kind":"todo","title":"Fixture","time_mode":"fixed","notes":"x".repeat(9000)})
            .to_string();
    let mut values = Vec::new();
    for encoding in [None, Some("gzip"), Some("gzip;q=0")] {
        let mut req = Request::builder()
            .method("POST")
            .uri("/v1/items")
            .header("porchlight-client-version", porchlight_core::VERSION)
            .header("authorization", format!("Bearer {}", s.bootstrap_token()))
            .header("idempotency-key", &key)
            .header("content-type", "application/json");
        if let Some(encoding) = encoding {
            req = req.header("accept-encoding", encoding);
        }
        let response = routes::agent_router(s.state.clone())
            .layer(axum::Extension(ConnInfo {
                transport: Transport::Agent,
                client_addr: None,
                client_cert_fingerprint: None,
            }))
            .oneshot(req.body(Body::from(payload.clone())).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let compressed = response.headers().contains_key("content-encoding");
        assert_eq!(compressed, encoding == Some("gzip"));
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = if compressed {
            let mut decoded = Vec::new();
            flate2::read::GzDecoder::new(bytes.as_ref())
                .read_to_end(&mut decoded)
                .unwrap();
            serde_json::from_slice(&decoded).unwrap()
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        values.push(value);
    }
    assert_eq!(values[0], values[1]);
    assert_eq!(values[0], values[2]);
    let count: i64 =
        sqlx::query_scalar("select count(*) from porchlight.items where household_id=$1")
            .bind(s.init.household_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);
    s.stop().await;
}

#[tokio::test]
async fn gzip_preserves_ranges_existing_coding_and_validators() {
    for (request_range, response_range, existing_coding) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let app = Router::new()
            .fallback(move || async move {
                let mut response = (
                    [
                        ("content-type", "application/json"),
                        ("etag", "\"fixture\""),
                        ("vary", "*"),
                    ],
                    "x".repeat(9000),
                )
                    .into_response();
                if response_range {
                    response
                        .headers_mut()
                        .insert("content-range", "bytes 0-8999/9000".parse().unwrap());
                }
                if existing_coding {
                    response
                        .headers_mut()
                        .insert("content-encoding", "br".parse().unwrap());
                }
                response
            })
            .layer(middleware::from_fn(porchlight_api::compression::json));
        let mut req = Request::builder()
            .uri("/v1/items")
            .header("accept-encoding", "gzip");
        if request_range {
            req = req.header("range", "bytes=0-");
        }
        let response = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.headers()["vary"], "*");
        if existing_coding {
            assert_eq!(response.headers()["content-encoding"], "br");
        } else if request_range || response_range {
            assert!(!response.headers().contains_key("content-encoding"));
        } else {
            assert_eq!(response.headers()["etag"], "W/\"fixture\"");
            let length: usize = response.headers()["content-length"]
                .to_str()
                .unwrap()
                .parse()
                .unwrap();
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            assert_eq!(length, bytes.len());
        }
    }
}
