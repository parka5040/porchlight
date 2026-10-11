//! Explicit data-only exports: never credentials, sessions, replay responses or PKI.
use crate::{
    error::{ApiError, Problem},
    extract::{ApiQuery, Caller},
    state::AppState,
};
use axum::{
    body::Body,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde_json::Value;
mod archive;
mod member;
mod spool;
use uuid::Uuid;

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct MemberParams {
    pub member_id: Option<Uuid>,
}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct MemberExport {
    pub format_version: u32,
    pub exported_at: DateTime<Utc>,
    pub member_id: Uuid,
    pub tables: Value,
    /// Save as appointments.ics. Materialized appointments, stable occurrence UID,
    /// all retained dates (no snapshot window); floating local times have no TZID.
    pub appointments_ics: String,
}
#[utoipa::path(get,path="/v1/export/member",params(MemberParams),responses((status=200,description="Data-only JSON envelope plus appointments_ics file content, authoritative member ACL even for admin PAT target",body=MemberExport),(status=403,description="Cannot export another member without admin PAT"),crate::openapi::RateLimitProblem,(status=507,description="storage-full: insufficient spool space",content_type="application/problem+json",body=crate::openapi::ProblemBody),crate::openapi::UniversalProblems))]
pub async fn member(
    State(s): State<AppState>,
    caller: Caller,
    ApiQuery(p): ApiQuery<MemberParams>,
) -> Result<Response, ApiError> {
    let target = p.member_id.unwrap_or(caller.member_id());
    if target != caller.member_id() && !caller.sees_all() {
        return Err(ApiError::new(Problem::Forbidden));
    }
    if caller.sees_all() {
        caller.require_admin()?;
    }
    let permit = s
        .export_limits
        .acquire(caller.household_id(), caller.household_id())?;
    let spool_permit = s
        .member_spool_limits
        .acquire(caller.household_id(), caller.household_id())?;
    let (mut send, receive) = tokio::sync::oneshot::channel();
    let runtime = tokio::runtime::Handle::current();
    // The worker owns its permit and snapshot even if the handler is cancelled.
    tokio::task::spawn_blocking(move || {
        let result = runtime.block_on(async {
            tokio::select! { biased;
                _ = send.closed() => Err(ApiError::internal("export_cancelled")),
                result = member::prepare(s, caller, target, spool_permit) => result,
            }
        });
        drop(permit);
        let _ = send.send(result);
    });
    let spool = receive
        .await
        .map_err(|_| ApiError::internal("export_task"))??;
    let len = spool
        .file
        .metadata()
        .map_err(|_| ApiError::internal("export_spool"))?
        .len();
    let body = spool::body(
        tokio::fs::File::from_std(spool.file),
        std::time::Duration::from_secs(30),
        (spool.reservation, spool.permit),
    );
    Ok((
        [
            ("content-type", "application/json".to_owned()),
            ("content-length", len.to_string()),
        ],
        body,
    )
        .into_response())
}
// Fail-closed allowlist: additions require an export decision, never introspect and
// accidentally expose a future credential table. Standard encrypted backups dump all.
const HOUSEHOLD_TABLES: &[&str] = &[
    "households",
    "users",
    "household_members",
    "devices",
    "device_certificates",
    "member_settings",
    "items",
    "item_assignees",
    "item_viewers",
    "item_reminders",
    "item_occurrences",
    "member_agenda",
    "scheduled_reminders",
    "item_derivation",
    "item_revisions",
    "task_events",
    "purge_log",
    "security_events",
    "notification_kinds",
    "notification_events",
    "notification_prefs",
    "notifications",
    "notification_sources",
    "notification_event_delivery",
    "notification_delivery_retry",
    "lists",
    "list_viewers",
    "list_entries",
    "list_entry_checks",
    "list_revisions",
    "list_entry_revisions",
    "comments",
    "attachments",
    "blobs",
    "collaboration_audit",
    "ics_sources",
    "ics_source_revisions",
    "ics_uid_map",
    "availability_windows",
    "availability_profiles",
    "schedule_suggestions",
    "planning_audit",
    "member_daily_load",
    "member_load_jobs",
    "scheduler_jobs",
    "outbox",
    "backup_runs",
];
async fn step_up(s: &AppState, c: &Caller, headers: &HeaderMap) -> Result<(), ApiError> {
    c.require_admin()?;
    if let Caller::Member {
        session_id,
        device_id,
        ..
    } = c
    {
        use porchlight_core::auth::token::{TokenKind, hash_token, looks_like};
        let token = headers
            .get("porchlight-step-up")
            .and_then(|v| v.to_str().ok())
            .filter(|t| looks_like(TokenKind::StepUp, t))
            .ok_or_else(|| ApiError::new(Problem::StepUpRequired))?;
        let mut tx = porchlight_db::tx::begin_as(&s.pool, &c.actor()).await?;
        let valid = porchlight_db::auth::consume_step_up(
            &mut tx,
            &hash_token(token),
            *session_id,
            *device_id,
            "household.export",
        )
        .await?;
        if !valid {
            return Err(ApiError::new(Problem::StepUpRequired));
        }
        tx.commit().await?;
    }
    Ok(())
}
#[utoipa::path(get,path="/v1/admin/export/household",params(crate::openapi::StepUpHeader),responses((status=200,description="Secret-free streamed tar.gz: v2 data.json manifest, tables/*.jsonl, live referenced attachment/thumbnail blobs and LAST complete.json inventory. Missing completion entry or invalid gzip trailer means incomplete. Admin PAT or super-admin app with household.export single-use step-up",body=String,content_type="application/gzip"),(status=403,description="Admin scope or step-up required"),crate::openapi::RateLimitProblem,crate::openapi::UniversalProblems))]
pub async fn household(
    State(s): State<AppState>,
    c: Caller,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    // This GET consumes a single-use credential and takes durable blob leases.
    if s.maintenance.active() {
        return Err(ApiError::new(Problem::Maintenance).retry(1));
    }
    c.require_admin()?;
    let permit = s
        .export_limits
        .acquire(c.household_id(), c.household_id())?;
    step_up(&s, &c, &headers).await?;
    let (send, receive) = tokio::sync::mpsc::channel(2);
    let household = c.household_id();
    let runtime = tokio::runtime::Handle::current();
    tokio::spawn(async move {
        let errors = send.clone();
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            archive::produce(s, household, send, runtime)
        });
        if !matches!(task.await, Ok(Ok(()))) {
            let _ = errors
                .send(Err(std::io::Error::other("export interrupted")))
                .await;
        }
    });
    // No table read or descriptor inventory before headers. Dropping the body
    // closes the channel, cancels preparation, and drops the worker's lease guard.
    Ok((
        [
            ("content-type", "application/gzip"),
            (
                "content-disposition",
                "attachment; filename=household.tar.gz",
            ),
        ],
        Body::from_stream(futures_util::stream::unfold(
            receive,
            |mut receive| async move { receive.recv().await.map(|chunk| (chunk, receive)) },
        )),
    )
        .into_response())
}
#[derive(utoipa::OpenApi)]
#[openapi(paths(member, household), components(schemas(MemberExport)))]
pub struct ExportsApi;

fn member_policy(c: &crate::permissions::PolicyContext) -> u16 {
    use porchlight_core::access::CallerKind;
    if matches!(c.item.caller, CallerKind::Agent) {
        if c.pat_scope == Some(crate::permissions::PatScope::Admin) {
            200
        } else {
            403
        }
    } else if c.is_self {
        200
    } else {
        403
    }
}
fn household_policy(c: &crate::permissions::PolicyContext) -> u16 {
    use porchlight_core::access::{CallerKind, Role};
    if c.item.caller == CallerKind::AppMember(Role::SuperAdmin)
        || c.pat_scope == Some(crate::permissions::PatScope::Admin)
    {
        200
    } else {
        403
    }
}
pub const ENDPOINTS: &[crate::permissions::Endpoint] = &[
    crate::permissions::Endpoint {
        method: "GET",
        path: "/v1/export/member",
        handler: "member",
        policy: crate::permissions::Policy::Custom {
            label: "Own member ACL; admin PAT can select a member (no bypass)",
            expected: member_policy,
        },
        case: "",
    },
    crate::permissions::Endpoint {
        method: "GET",
        path: "/v1/admin/export/household",
        handler: "household",
        policy: crate::permissions::Policy::Custom {
            label: "Admin PAT or super-admin app plus household.export step-up",
            expected: household_policy,
        },
        case: "",
    },
];
