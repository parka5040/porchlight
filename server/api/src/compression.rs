//! Negotiated JSON content coding, outside stored-response idempotency replay.
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::io::Write;

const MIN_BYTES: usize = 8 * 1024;

tokio::task_local! { static DOCUMENT_CODING: (bool, bool); }
/// The outer negotiation layer supplies this only while building the response.
pub(crate) fn document_coding() -> (bool, bool) {
    DOCUMENT_CODING
        .try_with(|coding| *coding)
        .unwrap_or((false, false))
}

fn quality(parameters: impl Iterator<Item = impl AsRef<str>>) -> u16 {
    let mut result = 1000;
    let mut seen = false;
    for parameter in parameters {
        if seen {
            return 0;
        }
        seen = true;
        let Some((name, value)) = parameter
            .as_ref()
            .trim()
            .split_once('=')
            .map(|(a, b)| (a.trim(), b.trim()))
        else {
            return 0;
        };
        if !name.eq_ignore_ascii_case("q") {
            return 0;
        }
        let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
        if fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return 0;
        }
        result = match whole {
            "0" => fraction.parse::<u16>().unwrap_or(0) * 10u16.pow(3 - fraction.len() as u32),
            "1" if fraction.bytes().all(|b| b == b'0') => 1000,
            _ => 0,
        };
    }
    result
}
fn accepted_codings(headers: &HeaderMap) -> (bool, bool) {
    let (mut gzip, mut wildcard, mut identity): (Option<u16>, Option<u16>, Option<u16>) =
        (None, None, None);
    for value in headers.get_all(header::ACCEPT_ENCODING) {
        let Ok(value) = value.to_str() else {
            return (false, true);
        };
        for coding in value.split(',') {
            let mut parts = coding.trim().split(';');
            let name = parts.next().unwrap_or("").trim();
            let slot = if name.eq_ignore_ascii_case("gzip") {
                &mut gzip
            } else if name.eq_ignore_ascii_case("identity") {
                &mut identity
            } else if name == "*" {
                &mut wildcard
            } else {
                continue;
            };
            let q = quality(parts);
            *slot = Some(slot.map_or(q, |old| old.min(q)));
        }
    }
    (
        gzip.or(wildcard).is_some_and(|q| q > 0),
        identity.unwrap_or(if wildcard == Some(0) { 0 } else { 1000 }) > 0,
    )
}
fn not_acceptable() -> Response {
    (
        StatusCode::NOT_ACCEPTABLE,
        [(header::VARY, "Accept-Encoding")],
        Body::empty(),
    )
        .into_response()
}
fn encoded_headers(headers: &mut HeaderMap) {
    headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    if let Some(etag) = headers.get(header::ETAG).and_then(|v| v.to_str().ok())
        && !etag.starts_with("W/")
        && let Ok(weak) = HeaderValue::from_str(&format!("W/{etag}"))
    {
        headers.insert(header::ETAG, weak);
    }
}
/// Secret-bearing classes and byte downloads stay identity-coded, including their
/// errors. Feed responses can reflect a label alongside a credential-bearing URL.
fn excluded(path: &str) -> bool {
    path.starts_with("/v1/auth/")
        || path.starts_with("/v1/enrollment/")
        || path == "/v1/admin/enrollment-tokens"
        || path == "/v1/admin/tokens"
        || path.starts_with("/v1/admin/tokens/")
        || (path.starts_with("/v1/admin/users/") && path.ends_with("/password-reset"))
        || path == "/v1/feeds"
        || path.starts_with("/v1/feeds/")
        || path == "/v1/admin/feeds"
        || path.starts_with("/v1/admin/feeds/")
        || path.starts_with("/v1/attachments/")
        || path.starts_with("/v1/export/")
        || path.starts_with("/v1/admin/export/")
        || path == "/v1/backups/latest/download"
        || path.starts_with("/v1/client-release/files/")
}
fn vary(headers: &mut HeaderMap) {
    let has = headers
        .get_all(header::VARY)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|v| v.trim() == "*" || v.trim().eq_ignore_ascii_case("accept-encoding"));
    if !has {
        headers.append(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    }
}
pub async fn json(req: Request, next: Next) -> Response {
    let head = req.method() == Method::HEAD;
    let skip = excluded(req.uri().path()) || req.headers().contains_key(header::RANGE);
    let (accepted, identity) = accepted_codings(req.headers());
    let mut response = DOCUMENT_CODING
        .scope((accepted && !skip, !identity), next.run(req))
        .await;
    if skip && !identity && !response.headers().contains_key(header::CONTENT_ENCODING) {
        return not_acceptable();
    }
    if skip
        || response.headers().contains_key(header::CONTENT_ENCODING)
        || response.headers().contains_key(header::CONTENT_RANGE)
    {
        return response;
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .unwrap_or("")
        .trim();
    if !matches!(
        content_type,
        "application/json" | "application/problem+json"
    ) {
        return response;
    }
    vary(response.headers_mut());
    if matches!(
        response.status(),
        StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED
    ) {
        return response;
    }
    if !accepted {
        return if identity { response } else { not_acceptable() };
    }
    if head {
        let length = response
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<usize>().ok());
        if !identity || length.is_none_or(|n| n >= MIN_BYTES) {
            encoded_headers(response.headers_mut());
            response.headers_mut().remove(header::CONTENT_LENGTH);
        }
        // Unknown size prevents Axum from inventing Content-Length: 0 after
        // HEAD has discarded the selected GET representation.
        if response.headers().contains_key(header::CONTENT_ENCODING) {
            *response.body_mut() = Body::from_stream(futures_util::stream::empty::<
                Result<axum::body::Bytes, std::convert::Infallible>,
            >());
        } else {
            *response.body_mut() = Body::empty();
        }
        return response;
    }
    // Upgrade before draining: compression must not release snapshot admission.
    let guard = response
        .extensions_mut()
        .remove::<crate::concurrency::BodyGuard>()
        .and_then(|guard| guard.0.upgrade());
    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => return crate::error::ApiError::internal("response_body").into_response(),
    };
    let response = if identity && bytes.len() < MIN_BYTES {
        Response::from_parts(parts, Body::from(bytes))
    } else {
        // Level 1 bounds CPU. Blocking work never monopolizes an async executor;
        // the admitted request retains its guard while waiting and compressing.
        let work_guard = guard.clone();
        let compressed = tokio::task::spawn_blocking(move || {
            // spawn_blocking outlives a cancelled HTTP future. Keep admission
            // until its CPU work and input buffer have actually been released.
            let _work_guard = work_guard;
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(1));
            encoder.write_all(&bytes)?;
            encoder.finish()
        })
        .await;
        let bytes = match compressed {
            Ok(Ok(bytes)) => bytes,
            _ => return crate::error::ApiError::internal("response_compression").into_response(),
        };
        parts
            .headers
            .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        parts
            .headers
            .insert(header::CONTENT_LENGTH, HeaderValue::from(bytes.len()));
        // Strong validators describe identity bytes, not this representation.
        if let Some(etag) = parts
            .headers
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok())
            && !etag.starts_with("W/")
            && let Ok(weak) = HeaderValue::from_str(&format!("W/{etag}"))
        {
            parts.headers.insert(header::ETAG, weak);
        }
        Response::from_parts(parts, Body::from(bytes))
    };
    if let Some(guard) = guard {
        crate::concurrency::with_body(response, guard)
    } else {
        response
    }
}
