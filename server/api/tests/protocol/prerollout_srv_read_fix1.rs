use axum::{
    Router,
    body::{Body, to_bytes},
    http::Request,
    middleware,
    response::IntoResponse,
    routing::get,
};
use tower::ServiceExt;

async fn request(
    path: &str,
    encoding: &str,
    method: &str,
    size: usize,
) -> axum::response::Response {
    let app = Router::new()
        .route(
            path,
            get(move || async move {
                (
                    [
                        ("content-type", "application/json"),
                        ("content-length", &size.to_string()),
                    ],
                    "x".repeat(size),
                )
                    .into_response()
            }),
        )
        .layer(middleware::from_fn(porchlight_api::compression::json));
    app.oneshot(
        Request::builder()
            .uri(path)
            .method(method)
            .header("accept-encoding", encoding)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}
#[test]
fn writer_budget_never_emits_a_partial_document() {
    let mut document = porchlight_api::json_document::Document::bounded(64);
    assert!(document.field("text", &"x".repeat(1000)).is_err());
    assert!(document.response().is_err());
}

#[tokio::test]
async fn incremental_document_negotiates_small_and_large_json() {
    use porchlight_api::json_document::Document;
    use std::io::Read;
    for size in [100, 9000] {
        let app = Router::new()
            .route(
                "/v1/snapshot",
                get(move || async move {
                    let mut doc = Document::default();
                    doc.field("text", &"x".repeat(size)).unwrap();
                    let mut rows = doc.array("rows").unwrap();
                    rows.sql_row(r#"{ "a": null, "b": "a b\\n\\\"" }"#);
                    rows.finish();
                    doc.response().unwrap()
                }),
            )
            .layer(middleware::from_fn(porchlight_api::compression::json));
        let mut values = Vec::new();
        for encoding in ["identity", "gzip", "gzip, identity;q=0"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/v1/snapshot")
                        .header("accept-encoding", encoding)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let gzip = response.headers().contains_key("content-encoding");
            assert_eq!(
                gzip,
                encoding != "identity" && (size >= 8192 || encoding.contains("identity;q=0"))
            );
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let mut decoded = Vec::new();
            if gzip {
                flate2::read::GzDecoder::new(bytes.as_ref())
                    .read_to_end(&mut decoded)
                    .unwrap();
            } else {
                decoded.extend_from_slice(&bytes);
            }
            values.push(serde_json::from_slice::<serde_json::Value>(&decoded).unwrap());
        }
        assert_eq!(values[0], values[1]);
        assert_eq!(values[0], values[2]);
    }
}

#[tokio::test]
async fn identity_rejection_is_respected() {
    for size in [100, 9000] {
        let r = request("/v1/items", "gzip, identity;q=0", "GET", size).await;
        assert_eq!(
            r.headers()
                .get("content-encoding")
                .and_then(|v| v.to_str().ok()),
            Some("gzip"),
            "identity forbidden at {size}"
        );
        for encoding in ["gzip;q=0, identity;q=0", "*;q=0"] {
            assert_eq!(
                request("/v1/items", encoding, "GET", size).await.status(),
                406
            );
        }
        let r = request("/v1/auth/login", "gzip, identity;q=0", "GET", size).await;
        assert_eq!(r.status(), 406);
        assert!(!r.headers().contains_key("content-encoding"));
    }
}
#[tokio::test]
async fn registered_head_uses_get_representation_metadata() {
    for size in [100, 9000] {
        for encoding in ["gzip", "gzip, identity;q=0"] {
            let get = request("/v1/items", encoding, "GET", size).await;
            let head = request("/v1/items", encoding, "HEAD", size).await;
            assert_eq!(
                head.headers().get("content-encoding"),
                get.headers().get("content-encoding"),
                "size={size}"
            );
            assert_eq!(head.headers().get("vary"), get.headers().get("vary"));
            if get.headers().contains_key("content-encoding") {
                assert!(!head.headers().contains_key("content-length"));
            }
            assert!(
                to_bytes(head.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }
}
