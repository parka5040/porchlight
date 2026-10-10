//! Common request headers and optimistic concurrency. No implicit time-zone authority changes.
use crate::{
    error::{ApiError, Problem},
    state::AppState,
};
use axum::{
    extract::{Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use chrono_tz::Tz;
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClientVersion(pub [u64; 3]);
impl FromStr for ClientVersion {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parts: Vec<_> = value.split('.').collect();
        if parts.len() != 3 {
            return Err("expected MAJOR.MINOR.PATCH");
        }
        let mut result = [0; 3];
        for (i, part) in parts.iter().enumerate() {
            if part.is_empty()
                || !part.bytes().all(|b| b.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
            {
                return Err("invalid numeric component");
            }
            result[i] = part.parse().map_err(|_| "numeric component overflow")?;
        }
        Ok(Self(result))
    }
}
impl fmt::Display for ClientVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0[0], self.0[1], self.0[2])
    }
}
#[derive(Clone, Copy, Debug)]
pub struct ClientTz(pub Option<Tz>);
fn single_header<'a>(
    headers: &'a HeaderMap,
    name: &str,
    problem: Problem,
) -> Result<Option<&'a str>, ApiError> {
    let mut values = headers.get_all(name).iter();
    let first = values.next();
    if values.next().is_some() {
        return Err(ApiError::new(problem));
    }
    first
        .map(|v| v.to_str().map_err(|_| ApiError::new(problem)))
        .transpose()
}
pub async fn request_headers(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let version = single_header(
        req.headers(),
        "porchlight-client-version",
        Problem::ClientVersionRequired,
    )?
    .and_then(|v| v.parse::<ClientVersion>().ok())
    .ok_or_else(|| ApiError::new(Problem::ClientVersionRequired))?;
    let update_path = req.uri().path() == "/v1/client-release"
        || req.uri().path().starts_with("/v1/client-release/files/");
    if !update_path && version < state.settings.min_client_version {
        return Err(ApiError::new(Problem::UpgradeRequired).extension(
            "min_client_version",
            serde_json::json!(state.settings.min_client_version.to_string()),
        ));
    }
    // A zone report is a hint, never a reason to reject an otherwise valid request.
    let tz = single_header(req.headers(), "porchlight-tz", Problem::InvalidTimeZone)
        .ok()
        .flatten()
        .and_then(|name| {
            porchlight_core::time::storage_zone(name)
                .ok()
                .map(|(zone, _)| zone)
        });
    req.extensions_mut().insert(ClientTz(tz));
    Ok(next.run(req).await)
}
/// Strict strong version: unquoted decimal or quoted decimal, not lists, wildcards or weak ETags.
pub fn if_match(headers: &HeaderMap) -> Result<i32, ApiError> {
    let value = single_header(headers, "if-match", Problem::InvalidRequest)?
        .ok_or_else(|| ApiError::new(Problem::IfMatchRequired))?;
    let value = if value.starts_with('"') {
        value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .ok_or_else(|| ApiError::new(Problem::InvalidRequest))?
    } else {
        value
    };
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ApiError::new(Problem::InvalidRequest));
    }
    value
        .parse()
        .map_err(|_| ApiError::new(Problem::InvalidRequest))
}
pub fn version_conflict(
    rows_affected: u64,
    current: impl serde::Serialize,
) -> Result<(), ApiError> {
    if rows_affected == 0 {
        Err(ApiError::new(Problem::VersionConflict).extension(
            "current",
            serde_json::to_value(current)
                .map_err(|_| ApiError::internal("version_current_serialization"))?,
        ))
    } else {
        Ok(())
    }
}
