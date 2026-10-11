//! Shared single-range streaming for verified local files.
use crate::error::{ApiError, Problem};
use axum::{
    body::Body,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
fn range(value: &str, size: u64) -> Option<(u64, u64)> {
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().ok()?;
        if suffix == 0 {
            return None;
        }
        return Some((size.saturating_sub(suffix), size.checked_sub(1)?));
    }
    let start = start.parse::<u64>().ok()?;
    let end = if end.is_empty() {
        size.checked_sub(1)?
    } else {
        end.parse::<u64>().ok()?.min(size.checked_sub(1)?)
    };
    (start < size && start <= end).then_some((start, end))
}
pub(crate) fn valid_range(size: u64, sha256: &str, headers: &HeaderMap) -> bool {
    selection(size, sha256, headers).is_some()
}
pub(crate) fn unsatisfiable(size: u64) -> Response {
    let mut response = ApiError::new(Problem::RangeNotSatisfiable).into_response();
    response
        .headers_mut()
        .insert("content-range", format!("bytes */{size}").parse().unwrap());
    response
}
fn selection(size: u64, sha256: &str, headers: &HeaderMap) -> Option<(StatusCode, u64, u64)> {
    let etag = format!("\"{}\"", sha256);
    let requested = headers.get("range").filter(|_| {
        headers
            .get("if-range")
            .is_none_or(|v| v.to_str().ok() == Some(etag.as_str()))
    });
    if let Some(value) = requested {
        range(value.to_str().ok()?, size).map(|(a, b)| (StatusCode::PARTIAL_CONTENT, a, b - a + 1))
    } else {
        Some((StatusCode::OK, 0, size))
    }
}
pub async fn stream(
    file: std::fs::File,
    size: u64,
    sha256: &str,
    headers: &HeaderMap,
    filename: Option<&str>,
) -> Result<Response, ApiError> {
    let Some((code, start, length)) = selection(size, sha256, headers) else {
        return Ok(unsatisfiable(size));
    };
    let etag = format!("\"{}\"", sha256);
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut file = tokio::fs::File::from_std(file);
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(|_| ApiError::internal("download_seek"))?;
    let mut response = (
        code,
        Body::from_stream(tokio_util::io::ReaderStream::new(file.take(length))),
    )
        .into_response();
    let h = response.headers_mut();
    for (key, value) in [
        ("content-type", "application/octet-stream".into()),
        ("content-length", (length).to_string()),
        ("accept-ranges", "bytes".into()),
        ("etag", etag),
        ("cache-control", "private, no-store".into()),
    ] {
        h.insert(
            axum::http::HeaderName::from_bytes(key.as_bytes()).unwrap(),
            value
                .parse()
                .map_err(|_| ApiError::internal("download_header"))?,
        );
    }
    if let Some(name) = filename {
        h.insert(
            "content-disposition",
            format!("attachment; filename={name}")
                .parse()
                .map_err(|_| ApiError::internal("download_header"))?,
        );
    }
    if code == StatusCode::PARTIAL_CONTENT {
        let end = start + length - 1;
        h.insert(
            "content-range",
            format!("bytes {start}-{end}/{size}").parse().unwrap(),
        );
    }
    Ok(response)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audit_trust_invalid_range_bytes_are_not_a_full_download() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "range",
            axum::http::HeaderValue::from_bytes(b"bytes=\xff").unwrap(),
        );
        assert!(
            !valid_range(10, "digest", &headers),
            "invalid Range must be rejected before verification"
        );
        headers.insert("if-range", "\"old-digest\"".parse().unwrap());
        assert!(
            valid_range(10, "digest", &headers),
            "If-Range mismatch intentionally restarts the download"
        );
    }
    #[test]
    fn ranges() {
        assert_eq!(range("bytes=2-4", 10), Some((2, 4)));
        assert_eq!(range("bytes=3-", 10), Some((3, 9)));
        assert_eq!(range("bytes=-3", 10), Some((7, 9)));
        for bad in ["bytes=10-", "bytes=3-2", "bytes=1-2,4-5", "bytes=-0"] {
            assert_eq!(range(bad, 10), None)
        }
    }
}
