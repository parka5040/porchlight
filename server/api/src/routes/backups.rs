//! Read-only backup health and encrypted downloads. No decryption identity on server.
use crate::{
    error::{ApiError, Problem},
    extract::Caller,
    state::AppState,
};
use axum::{Json, extract::State, http::HeaderMap, response::Response};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;
mod verify;

#[derive(Serialize, Deserialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Bundle {
    pub id: Uuid,
    pub bundle_file: String,
    pub size: i64,
    pub sha256: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub next_due_at: DateTime<Utc>,
}
#[derive(Serialize, utoipa::ToSchema)]
pub struct BackupStatus {
    pub last_success: Option<DateTime<Utc>>,
    pub next_due: Option<DateTime<Utc>>,
    /// Controlled error category; raw external diagnostics are never returned.
    pub last_error: Option<String>,
    pub latest: Option<Bundle>,
}
async fn newest(c: &mut PgConnection) -> Result<Option<Bundle>, ApiError> {
    Ok(sqlx::query_as("select id,bundle_file,size,sha256,started_at,finished_at,next_due_at from porchlight.backup_runs where status='success' order by finished_at desc,id desc limit 1").fetch_optional(c).await?)
}
async fn status_inner(pool: &PgPool) -> Result<BackupStatus, ApiError> {
    let mut tx = porchlight_db::tx::begin(pool).await?;
    sqlx::query("set transaction isolation level repeatable read read only")
        .execute(&mut *tx)
        .await?;
    let latest = newest(&mut tx).await?;
    let failed: Option<DateTime<Utc>> = sqlx::query_scalar(
        "select max(finished_at) from porchlight.backup_runs where status='failed'",
    )
    .fetch_one(&mut *tx)
    .await?;
    let last_success = latest.as_ref().map(|b| b.finished_at);
    let last_error = failed
        .filter(|t| last_success.is_none_or(|success| *t > success))
        .map(|_| "backup failed; inspect private runtime diagnostics".to_string());
    let next_due = if last_error.is_some() {
        failed.map(|t| t + chrono::Duration::hours(1))
    } else {
        latest.as_ref().map(|b| b.next_due_at)
    };
    tx.commit().await?;
    Ok(BackupStatus {
        last_success,
        next_due,
        last_error,
        latest,
    })
}
#[utoipa::path(get,path="/v1/backups",responses((status=200,description="Super-admin app backup health",body=BackupStatus),(status=403,description="Super admin only"),crate::openapi::UniversalProblems))]
pub async fn status(State(s): State<AppState>, c: Caller) -> Result<Json<BackupStatus>, ApiError> {
    c.require_admin()?;
    Ok(Json(status_inner(&s.pool).await?))
}
#[utoipa::path(get,path="/v1/admin/backups",responses((status=200,description="Admin PAT backup health",body=BackupStatus),(status=403,description="Admin scope required"),crate::openapi::UniversalProblems))]
pub async fn admin_status(
    State(s): State<AppState>,
    c: Caller,
) -> Result<Json<BackupStatus>, ApiError> {
    status(State(s), c).await
}
#[utoipa::path(get,path="/v1/backups/latest",responses((status=200,description="Newest successful encrypted bundle metadata",body=Bundle),(status=404,description="No successful backup"),(status=403,description="Super admin only"),crate::openapi::UniversalProblems))]
pub async fn latest(State(s): State<AppState>, c: Caller) -> Result<Json<Bundle>, ApiError> {
    c.require_admin()?;
    let mut conn = s.pool.acquire().await?;
    Ok(Json(
        newest(&mut conn)
            .await?
            .ok_or_else(|| ApiError::new(Problem::NotFound))?,
    ))
}
#[utoipa::path(get,path="/v1/backups/latest/download",params(("Range"=Option<String>,Header,description="One byte range; suffix and open-ended supported"),("If-Range"=Option<String>,Header,description="Quoted sha256 ETag. Mismatch restarts as 200")),responses((status=200,description="Encrypted bytes, ETag sha256, Content-Length and Accept-Ranges",body=String,content_type="application/octet-stream"),(status=206,description="Byte range, Content-Range",body=String,content_type="application/octet-stream"),(status=416,description="Unsatisfiable range"),(status=409,description="File size/checksum mismatch"),(status=404,description="No bundle"),(status=403,description="Super-admin app over mTLS only"),crate::openapi::UniversalProblems))]
pub async fn download(
    State(s): State<AppState>,
    c: Caller,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    c.require_admin()?;
    let mut conn = s.pool.acquire().await?;
    let bundle = newest(&mut conn)
        .await?
        .ok_or_else(|| ApiError::new(Problem::NotFound))?;
    drop(conn);
    // The database constrains names, but validate again at the filesystem boundary.
    let name = &bundle.bundle_file;
    if name.contains('/')
        || name.contains('\\')
        || !name.starts_with("porchlight-")
        || !name.ends_with(".tar.age")
    {
        return Err(ApiError::internal("backup_name"));
    }
    let path = s.settings.backup_dir.join(name);
    if path.ancestors().any(|p| p.is_symlink()) {
        return Err(ApiError::internal("backup_path"));
    }
    let size = bundle.size as u64;
    let sha = bundle.sha256.clone();
    let publication = bundle.id;
    let file = tokio::task::spawn_blocking(move || verify::open(&path, publication, size, &sha))
        .await
        .map_err(|_| ApiError::internal("backup_worker"))??;
    crate::download::stream(
        file,
        size,
        &bundle.sha256,
        &headers,
        Some(&bundle.bundle_file),
    )
    .await
}

pub async fn emit_stale(c: &mut PgConnection, now: DateTime<Utc>) -> anyhow::Result<()> {
    sqlx::query("select set_config('porchlight.notification_now',$1,true)")
        .bind(now.to_rfc3339())
        .execute(&mut *c)
        .await?;
    sqlx::query("with last as (select id,finished_at from porchlight.backup_runs where status='success' order by finished_at desc,id desc limit 1) select porchlight.pl_emit_notification_event(h.id,m.id,'backup_stale','household',h.id,'{}',null,null,'backup-stale:'||m.id::text||':'||coalesce((select id::text from last),'initial')) from porchlight.households h join porchlight.household_members m on m.household_id=h.id where h.deleted_at is null and m.deleted_at is null and m.role='super_admin' and coalesce((select finished_at from last),h.created_at)<$1-interval '26 hours'").bind(now).execute(c).await?;
    Ok(())
}
fn admin_policy(c: &crate::permissions::PolicyContext) -> u16 {
    use porchlight_core::access::{CallerKind, Role};
    if c.item.caller == CallerKind::AppMember(Role::SuperAdmin)
        || c.pat_scope == Some(crate::permissions::PatScope::Admin)
    {
        200
    } else {
        403
    }
}
macro_rules! endpoint {
    ($path:literal,$handler:literal) => {
        crate::permissions::Endpoint {
            method: "GET",
            path: $path,
            handler: $handler,
            policy: crate::permissions::Policy::Custom {
                label: "Super-admin app (mTLS) or admin PAT only on agent status surface",
                expected: admin_policy,
            },
            case: "",
        }
    };
}
pub const ENDPOINTS: &[crate::permissions::Endpoint] = &[
    endpoint!("/v1/backups", "status"),
    endpoint!("/v1/admin/backups", "admin_status"),
    endpoint!("/v1/backups/latest", "latest"),
    endpoint!("/v1/backups/latest/download", "download"),
];
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(status, admin_status, latest, download),
    components(schemas(Bundle, BackupStatus))
)]
pub struct BackupsApi;
