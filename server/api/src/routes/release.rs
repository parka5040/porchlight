//! Device update relay and privileged server-release inspection. No upgrade execution.
use crate::{
    error::{ApiError, Problem},
    extract::{Caller, DeviceAuth},
    release::{ClientRelease, Store},
    release_watch::ServerRelease,
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
};
use serde::Serialize;
use std::sync::Arc;

fn store(s: &AppState) -> Result<Arc<Store>, ApiError> {
    s.client_release
        .get()
        .and_then(Option::as_ref)
        .cloned()
        .ok_or_else(|| ApiError::new(Problem::ClientReleaseUnavailable))
}

#[utoipa::path(get,path="/v1/client-release",description="Any enrolled device, without a session. Valid client-version header required; exempt from the minimum client version.",responses((status=200,description="Verified release matching this server",body=ClientRelease),(status=404,description="client-release-unavailable",body=crate::openapi::ProblemBody,content_type="application/problem+json"),crate::openapi::UniversalProblems))]
pub async fn client(
    State(s): State<AppState>,
    _: DeviceAuth,
) -> Result<Json<ClientRelease>, ApiError> {
    Ok(Json(store(&s)?.advertised.clone()))
}
#[utoipa::path(get,path="/v1/client-release/files/{path}",description="Enrolled device only, no session required. Exact allowlisted path, including nested files. Exempt from the minimum client version.",params(("path"=String,Path,description="Allowlisted relative path, possibly containing slashes"),("Range"=Option<String>,Header,description="Single byte range, suffix or open-ended"),("If-Range"=Option<String>,Header,description="Quoted sha256 ETag; mismatch restarts as 200")),responses((status=200,description="Verified bytes; ETag, Content-Length, Accept-Ranges; private, no-store",body=String,content_type="application/octet-stream"),(status=206,description="Byte range with Content-Range",body=String,content_type="application/octet-stream"),(status=416,description="range-not-satisfiable; Content-Range bytes */size",body=crate::openapi::ProblemBody,content_type="application/problem+json"),(status=404,description="client-release-unavailable or path not allowlisted",body=crate::openapi::ProblemBody,content_type="application/problem+json"),crate::openapi::UniversalProblems))]
pub async fn download(
    State(s): State<AppState>,
    _: DeviceAuth,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let store = store(&s)?;
    let (file, size, sha) = tokio::task::spawn_blocking(move || store.open(&path))
        .await
        .map_err(|_| ApiError::internal("release_worker"))?
        .map_err(|_| ApiError::new(Problem::NotFound))?;
    crate::download::stream(file, size, &sha, &headers, None).await
}
#[utoipa::path(get,path="/v1/server-release",security(("session"=[])),responses((status=200,description="Last signed release check and owner-run upgrade command",body=ServerRelease),crate::openapi::ForbiddenProblem,crate::openapi::UniversalProblems))]
pub async fn server(State(s): State<AppState>, c: Caller) -> Result<Json<ServerRelease>, ApiError> {
    c.require_admin()?;
    Ok(Json(
        s.release_watch
            .status(s.settings.upgrade_command.clone())
            .await,
    ))
}
#[derive(Serialize, utoipa::ToSchema)]
pub struct AdvertisedClient {
    pub version: String,
    pub android_version_code: u64,
}
#[derive(Serialize, utoipa::ToSchema)]
pub struct ReleaseStatus {
    #[serde(flatten)]
    pub server: ServerRelease,
    pub client_release: Option<AdvertisedClient>,
}
#[utoipa::path(get,path="/v1/admin/release",security(("pat"=[])),responses((status=200,description="Admin PAT release status for porch",body=ReleaseStatus),crate::openapi::ForbiddenProblem,crate::openapi::UniversalProblems))]
pub async fn admin_status(
    State(s): State<AppState>,
    c: Caller,
) -> Result<Json<ReleaseStatus>, ApiError> {
    c.require_admin()?;
    let client_release = s
        .client_release
        .get()
        .and_then(Option::as_ref)
        .map(|store| AdvertisedClient {
            version: store.advertised.version.clone(),
            android_version_code: store.advertised.android.version_code,
        });
    Ok(Json(ReleaseStatus {
        server: s
            .release_watch
            .status(s.settings.upgrade_command.clone())
            .await,
        client_release,
    }))
}
fn device_policy(c: &crate::permissions::PolicyContext) -> u16 {
    if c.pat_scope.is_some() { 404 } else { 200 }
}
fn server_policy(c: &crate::permissions::PolicyContext) -> u16 {
    use porchlight_core::access::{CallerKind, Role};
    if c.pat_scope.is_some() {
        404
    } else if c.item.caller == CallerKind::AppMember(Role::SuperAdmin) {
        200
    } else {
        403
    }
}
fn agent_policy(c: &crate::permissions::PolicyContext) -> u16 {
    match c.pat_scope {
        Some(crate::permissions::PatScope::Admin) => 200,
        Some(_) => 403,
        None => 404,
    }
}
macro_rules! endpoint {
    ($path:literal,$handler:literal,$label:literal,$policy:ident) => {
        crate::permissions::Endpoint {
            method: "GET",
            path: $path,
            handler: $handler,
            policy: crate::permissions::Policy::Custom {
                label: $label,
                expected: $policy,
            },
            case: "",
        }
    };
}
pub const ENDPOINTS: &[crate::permissions::Endpoint] = &[
    endpoint!(
        "/v1/client-release",
        "client",
        "Enrolled device (mTLS), no session; version minimum exempt",
        device_policy
    ),
    endpoint!(
        "/v1/client-release/files/{path}",
        "download",
        "Enrolled device (mTLS), verified allowlist; version minimum exempt",
        device_policy
    ),
    endpoint!(
        "/v1/server-release",
        "server",
        "Super-admin app (mTLS) only",
        server_policy
    ),
    endpoint!(
        "/v1/admin/release",
        "admin_status",
        "Admin PAT only; agent status surface",
        agent_policy
    ),
];
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(client, download, server, admin_status),
    components(schemas(ClientRelease, ServerRelease, ReleaseStatus))
)]
pub struct ReleaseApi;
