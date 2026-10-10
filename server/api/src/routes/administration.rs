//! S5 agent-only domain administration. Every mutation uses its claim connection.
use crate::{
    error::{ApiError, FieldError, Problem},
    extract::{ApiQuery, Caller},
    idempotency::IdemTx,
    pagination::{Page, PageParams},
    state::AppState,
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Acquire, PgConnection};
use utoipa::ToSchema;
use uuid::Uuid;

fn invalid(field: &str) -> ApiError {
    ApiError::validation(vec![FieldError {
        field: field.into(),
        message: "Invalid value".into(),
        code: None,
    }])
}
fn target(tx: &IdemTx) -> Result<Uuid, ApiError> {
    tx.context
        .target_id
        .ok_or_else(|| ApiError::new(Problem::InvalidRequest))
}
#[derive(Serialize, sqlx::FromRow, ToSchema)]
pub struct UserResponse {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub username: Option<String>,
    pub display_name: String,
    pub role: String,
    pub color: String,
    pub tzid: String,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}
#[derive(ToSchema)]
pub struct UsersPage {
    pub items: Vec<UserResponse>,
    pub next_cursor: Option<String>,
}
#[derive(ToSchema)]
pub struct RevisionsPage {
    pub items: Vec<RevisionResponse>,
    pub next_cursor: Option<String>,
}
const USERS: &str = "select
m.id,m.user_id,u.username,m.display_name,m.role,m.color,m.tzid,m.version,m.created_at,m.deleted_at
from porchlight.household_members m left join
porchlight.users u on u.id=m.user_id";
async fn user(
    conn: &mut PgConnection,
    household: Uuid,
    id: Uuid,
) -> Result<UserResponse, ApiError> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{USERS} where m.household_id=$1 and m.id=$2"
    )))
    .bind(household)
    .bind(id)
    .fetch_optional(conn)
    .await?
    .ok_or_else(|| ApiError::new(Problem::NotFound))
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AddUser {
    display_name: String,
    #[serde(default = "user_role")]
    role: String,
    username: Option<String>,
    initial_password: Option<String>,
    tzid: Option<String>,
}
fn user_role() -> String {
    "user".into()
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EditUser {
    display_name: Option<String>,
    role: Option<String>,
    color: Option<String>,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RevertRequest {
    revision_id: Uuid,
}
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PurgeRequest {
    item_ids: Option<Vec<Uuid>>,
    deleted_before: Option<DateTime<Utc>>,
}
#[derive(Serialize, ToSchema)]
pub struct PurgeResponse {
    id: Uuid,
    item_ids: Vec<Uuid>,
    #[schema(value_type = PurgeCounts)]
    counts: Value,
}
/// Counts captured before deletion. O2 retention migrations must extend this schema
/// when adding another dependent table to pl_purge_items.
#[derive(ToSchema)]
pub struct PurgeCounts {
    pub ics_uid_map: i64,
    pub schedule_suggestions: i64,
    pub notification_events: Option<i64>,
    pub notification_sources: i64,
    pub notification_event_delivery: i64,
    /// Item references explicitly severed; recipient history survives.
    pub notifications: i64,
    pub blob_references: i64,
    pub comments: i64,
    pub attachments: i64,
    pub lists: i64,
    pub list_entries: i64,
    pub list_viewers: i64,
    pub list_entry_checks: i64,
    pub list_revisions: i64,
    pub list_entry_revisions: i64,
    pub member_agenda: i64,
    pub scheduled_reminders: i64,
    pub item_occurrences: i64,
    pub item_assignees: i64,
    pub item_viewers: i64,
    pub item_reminders: i64,
    pub item_derivation: i64,
    pub item_revisions: i64,
    pub task_events: i64,
    pub items: i64,
}
/// Append-only SQL actor envelope; null actor settings are omitted by SQL.
#[derive(ToSchema)]
pub struct RevisionActor {
    pub member_id: Option<Uuid>,
    pub device_id: Option<Uuid>,
    pub request_id: Option<Uuid>,
    pub application_name: Option<String>,
    pub token_id: Option<Uuid>,
    pub db_user: String,
}
/// Raw authoritative database snapshot, not an ItemResponse. In particular SQL
/// intervals are strings, reminders are named `reminders`, and no capabilities,
/// conflicts or pending flags are stored. Keep in step with pl_item_snapshot.
#[derive(ToSchema)]
pub struct RevisionSnapshot {
    pub id: Uuid,
    pub household_id: Uuid,
    pub title: String,
    pub notes: String,
    pub priority: i16,
    pub owner_id: Uuid,
    pub subject_member_id: Option<Uuid>,
    #[schema(value_type = super::items::ItemKind)]
    pub kind: String,
    #[schema(value_type = super::items::ItemTimeMode)]
    pub time_mode: String,
    pub tzid: Option<String>,
    pub all_day: bool,
    pub due_date: Option<chrono::NaiveDate>,
    pub due_at: Option<DateTime<Utc>>,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_due: Option<chrono::NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_start: Option<chrono::NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_end: Option<chrono::NaiveDateTime>,
    pub address: Option<String>,
    pub buffer_before_min: i32,
    pub buffer_after_min: i32,
    pub est_minutes: Option<i32>,
    pub rrule: Option<String>,
    #[schema(value_type = Option<super::items::ItemRecurrenceMode>)]
    pub recurrence_mode: Option<String>,
    pub completion_interval: Option<String>,
    #[schema(value_type = super::items::ItemMissedPolicy)]
    pub missed_policy: String,
    #[schema(value_type = super::items::ItemVisibility)]
    pub visibility: String,
    pub ics_source_id: Option<Uuid>,
    pub source: Option<String>,
    pub version: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub series_id: Option<Uuid>,
    pub split_from_item_id: Option<Uuid>,
    pub split_from_seq: Option<i32>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub nominal_local_anchor: Option<chrono::NaiveDateTime>,
    pub nominal_wall_duration: Option<i64>,
    pub assignees: Vec<Uuid>,
    pub viewers: Vec<Uuid>,
    pub reminders: Vec<i32>,
}
#[derive(Serialize, sqlx::FromRow, ToSchema)]
pub struct DerivationFailure {
    item_id: Uuid,
    attempts: i32,
    last_error: Option<String>,
    retry_at: Option<DateTime<Utc>>,
}
type FailureCursorRow = (
    Uuid,
    i32,
    Option<String>,
    Option<DateTime<Utc>>,
    DateTime<Utc>,
);
#[derive(Serialize, ToSchema)]
pub struct DerivationRetry {
    item_id: Uuid,
    /// True only when an existing job still has unapplied input generations.
    queued: bool,
}
#[derive(Serialize, ToSchema)]
pub struct DerivationStatus {
    pending_count: i64,
    items: Vec<DerivationFailure>,
    next_cursor: Option<String>,
}
#[derive(Serialize, sqlx::FromRow, ToSchema)]
pub struct RevisionResponse {
    id: Uuid,
    item_id: Uuid,
    rev: i32,
    #[schema(value_type = RevisionSnapshot)]
    snapshot: Value,
    #[schema(value_type = RevisionActor)]
    actor: Value,
    created_at: DateTime<Utc>,
}

#[utoipa::path(security(("pat"=[])),get,path="/v1/admin/users",params(("cursor"=Option<String>,Query),("limit"=Option<u16>,Query)),responses((status=200,body=UsersPage,description="Members including deleted members"),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem))]
pub async fn users(
    State(s): State<AppState>,
    c: Caller,
    ApiQuery(page): ApiQuery<PageParams>,
) -> Result<Json<Page<UserResponse>>, ApiError> {
    c.require_admin()?;
    let limit = page.limit()?;
    let context = format!("admin-users:{}", c.household_id());
    let after: Option<(DateTime<Utc>, Uuid)> = page
        .cursor
        .as_deref()
        .map(|v| s.idem.cursor.decode(&context, v))
        .transpose()?;
    let rows = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{USERS} where m.household_id=$1 and
($2::timestamptz is null or (m.created_at,m.id)>($2,$3))
order by m.created_at,m.id limit $4"
    )))
    .bind(c.household_id())
    .bind(after.map(|x| x.0))
    .bind(after.map(|x| x.1))
    .bind((limit + 1) as i64)
    .fetch_all(&s.pool)
    .await?;
    Ok(Json(s.idem.cursor.page(
        &context,
        rows,
        limit,
        |r: &UserResponse| (r.created_at, r.id),
    )?))
}
#[utoipa::path(security(("pat"=[])),post,path="/v1/admin/users",request_body=AddUser,responses((status=201,body=UserResponse,description="Created"),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem))]
pub fn add_user<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: AddUser,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        c.require_admin()?;
        if !(1..=60).contains(&b.display_name.trim().chars().count()) {
            return Err(invalid("display_name"));
        }
        if !["user", "super_admin", "dependent"].contains(&b.role.as_str()) {
            return Err(invalid("role"));
        }
        let household_zone: String =
            sqlx::query_scalar("select tzid from porchlight.households where id=$1")
                .bind(c.household_id())
                .fetch_one(&mut *tx.tx)
                .await?;
        let zone =
            porchlight_core::time::storage_zone(b.tzid.as_deref().unwrap_or(&household_zone))
                .map_err(|_| ApiError::new(Problem::InvalidTimeZone))?;
        let dependent = b.role == "dependent";
        if dependent && zone.1 != household_zone {
            return Err(invalid("tzid"));
        }
        let hash = if dependent {
            if b.username.is_some() || b.initial_password.is_some() {
                return Err(invalid("username"));
            }
            None
        } else {
            let username = b.username.as_deref().ok_or_else(|| invalid("username"))?;
            if !(2..=32).contains(&username.len())
                || !username.as_bytes()[0].is_ascii_alphanumeric()
                || !username
                    .bytes()
                    .all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || b"._-".contains(&x))
            {
                return Err(invalid("username"));
            }
            let password = b
                .initial_password
                .as_deref()
                .ok_or_else(|| invalid("initial_password"))?;
            super::auth::policy(password)?;
            Some(super::auth::hash(s, password).await?)
        };
        let mut d = tx.tx.begin().await?;
        let result = async {
            let uid = if let Some(hash) = hash {
                let exists: bool = sqlx::query_scalar(
                    "select exists(select 1 from porchlight.users where
household_id=$1 and username=$2)",
                )
                .bind(c.household_id())
                .bind(&b.username)
                .fetch_one(&mut *d)
                .await?;
                if exists {
                    return Err(invalid("username"));
                }
                let id: Uuid = sqlx::query_scalar(
                    "insert into porchlight.users(household_id,username)
values($1,$2) returning id",
                )
                .bind(c.household_id())
                .bind(&b.username)
                .fetch_one(&mut *d)
                .await
                .map_err(|e| {
                    if e.as_database_error().and_then(|e| e.code()).as_deref() == Some("23505") {
                        invalid("username")
                    } else {
                        e.into()
                    }
                })?;
                sqlx::query(
                    "insert into
porchlight.user_credentials(household_id,user_id,password_hash)
values($1,$2,$3)",
                )
                .bind(c.household_id())
                .bind(id)
                .bind(hash)
                .execute(&mut *d)
                .await?;
                Some(id)
            } else {
                None
            };
            let n: i64 = sqlx::query_scalar(
                "select count(*) from porchlight.household_members where
household_id=$1",
            )
            .bind(c.household_id())
            .fetch_one(&mut *d)
            .await?;
            let colors = [
                "#FABD2F", "#FE8019", "#8EC07C", "#83A598", "#D3869B", "#B8BB26", "#FB4934",
            ];
            let id: Uuid = sqlx::query_scalar(
                "insert into
porchlight.household_members(household_id,user_id,display_name,role,color,tzid)
values($1,$2,$3,$4,$5,$6) returning id",
            )
            .bind(c.household_id())
            .bind(uid)
            .bind(b.display_name.trim())
            .bind(&b.role)
            .bind(colors[n as usize % colors.len()])
            .bind(zone.1)
            .fetch_one(&mut *d)
            .await?;
            Ok((
                StatusCode::CREATED,
                Json(user(&mut d, c.household_id(), id).await?),
            )
                .into_response())
        }
        .await;
        match result {
            Ok(r) => {
                d.commit().await?;
                Ok(r)
            }
            Err(e) => {
                d.rollback().await?;
                Err(e)
            }
        }
    })
}
async fn last_admin(
    conn: &mut PgConnection,
    h: Uuid,
    id: Uuid,
    removing: bool,
) -> Result<(), ApiError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "select id from porchlight.household_members where
household_id=$1 and (role='super_admin' or id=$2) and
deleted_at is null order by id for no key update",
    )
    .bind(h)
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    if removing {
        let n: i64 = sqlx::query_scalar(
            "select count(*) from porchlight.household_members where
household_id=$1 and role='super_admin' and deleted_at is
null and id<>$2",
        )
        .bind(h)
        .bind(id)
        .fetch_one(conn)
        .await?;
        if n == 0 {
            return Err(invalid("role"));
        }
    }
    let _ = ids;
    Ok(())
}
#[utoipa::path(security(("pat"=[])),patch,path="/v1/admin/users/{id}",request_body=EditUser,params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),responses(super::items::IfMatchProblem,(status=200,body=UserResponse,description="Updated"),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem))]
pub fn edit_user<'a>(
    _s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: EditUser,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        c.require_admin()?;
        let id = target(tx)?;
        let expected = crate::protocol::if_match(&tx.context.if_match)?;
        let mut d = tx.tx.begin().await?;
        let result = async {
            let old = user(&mut d, c.household_id(), id).await?;
            if old.deleted_at.is_some() {
                return Err(ApiError::new(Problem::NotFound));
            }
            let role = b.role.as_deref().unwrap_or(&old.role);
            if !["user", "super_admin", "dependent"].contains(&role)
                || (role == "dependent") != old.user_id.is_none()
            {
                return Err(invalid("role"));
            }
            let name = b.display_name.as_deref().unwrap_or(&old.display_name);
            if !(1..=60).contains(&name.trim().chars().count()) {
                return Err(invalid("display_name"));
            }
            let color = b.color.as_deref().unwrap_or(&old.color);
            if color.len() != 7
                || !color.starts_with('#')
                || !color[1..]
                    .bytes()
                    .all(|x| x.is_ascii_hexdigit() && !x.is_ascii_lowercase())
            {
                return Err(invalid("color"));
            }
            last_admin(
                &mut d,
                c.household_id(),
                id,
                old.role == "super_admin" && role != "super_admin",
            )
            .await?;
            let current = user(&mut d, c.household_id(), id).await?;
            if current.version != expected {
                return Err(ApiError::new(Problem::VersionConflict)
                    .extension("current", serde_json::to_value(current).unwrap()));
            }
            if name != old.display_name || role != old.role || color != old.color {
                sqlx::query(
                    "update porchlight.household_members set
display_name=$2,role=$3,color=$4 where id=$1",
                )
                .bind(id)
                .bind(name.trim())
                .bind(role)
                .bind(color)
                .execute(&mut *d)
                .await?;
            }
            Ok(Json(user(&mut d, c.household_id(), id).await?).into_response())
        }
        .await;
        match result {
            Ok(r) => {
                d.commit().await?;
                Ok(r)
            }
            Err(e) => {
                d.rollback().await?;
                Err(e)
            }
        }
    })
}
#[utoipa::path(security(("pat"=[])),delete,path="/v1/admin/users/{id}",request_body=super::devices::Empty,params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),responses(super::items::IfMatchProblem,(status=200,body=UserResponse,description="Deleted; devices and PATs revoked atomically"),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem))]
pub fn delete_user<'a>(
    _s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    _b: super::devices::Empty,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        c.require_admin()?;
        let id = target(tx)?;
        let expected = crate::protocol::if_match(&tx.context.if_match)?;
        let mut d = tx.tx.begin().await?;
        let mut revoked = vec![];
        let result = async {
            let old = user(&mut d, c.household_id(), id).await?;
            // The claim already owns the calling PAT; cross-PAT locks never wait, to
            // prevent two agents deleting each other's owners from forming a cycle.
            let pats: Vec<Uuid> = sqlx::query_scalar(
                "select id from porchlight.api_tokens where household_id=$1
and owner_member_id=$2 order by id for no key update nowait",
            )
            .bind(c.household_id())
            .bind(id)
            .fetch_all(&mut *d)
            .await?;
            let devices: Vec<Uuid> = sqlx::query_scalar(
                "select id from porchlight.devices where household_id=$1 and
member_id=$2 order by id for update",
            )
            .bind(c.household_id())
            .bind(id)
            .fetch_all(&mut *d)
            .await?;
            if let Some(uid) = old.user_id {
                porchlight_db::auth::lock_credentials(&mut d, uid).await?;
            }
            for device in &devices {
                porchlight_db::auth::revoke_device_sessions(&mut *d, *device).await?;
            }
            last_admin(
                &mut d,
                c.household_id(),
                id,
                old.role == "super_admin" && old.deleted_at.is_none(),
            )
            .await?;
            let current = user(&mut d, c.household_id(), id).await?;
            if current.version != expected {
                return Err(ApiError::new(Problem::VersionConflict)
                    .extension("current", serde_json::to_value(current).unwrap()));
            }
            let now_devices: Vec<Uuid> = sqlx::query_scalar(
                "select id from porchlight.devices where household_id=$1 and
member_id=$2 order by id",
            )
            .bind(c.household_id())
            .bind(id)
            .fetch_all(&mut *d)
            .await?;
            if devices != now_devices {
                return Err(ApiError::transient(Problem::Conflict).retry(1));
            }
            let now_pats: Vec<Uuid> = sqlx::query_scalar(
                "select id from porchlight.api_tokens where household_id=$1
and owner_member_id=$2 order by id",
            )
            .bind(c.household_id())
            .bind(id)
            .fetch_all(&mut *d)
            .await?;
            if pats != now_pats {
                return Err(ApiError::transient(Problem::Conflict).retry(1));
            }
            for device in &devices {
                if let Some(r) =
                    porchlight_db::devices::revoke(&mut d, c.household_id(), Some(id), *device)
                        .await?
                {
                    revoked.push((*device, r.fingerprints));
                }
            }
            sqlx::query(
                "update porchlight.api_tokens set
revoked_at=coalesce(revoked_at,now()) where household_id=$1
and owner_member_id=$2",
            )
            .bind(c.household_id())
            .bind(id)
            .execute(&mut *d)
            .await?;
            sqlx::query(
                "update porchlight.enrollment_tokens set
used_at=coalesce(used_at,now()) where household_id=$1 and
member_id=$2",
            )
            .bind(c.household_id())
            .bind(id)
            .execute(&mut *d)
            .await?;
            if let Some(uid) = old.user_id {
                sqlx::query(
                    "update porchlight.users set
deleted_at=coalesce(deleted_at,now()) where id=$1",
                )
                .bind(uid)
                .execute(&mut *d)
                .await?;
            }
            if old.deleted_at.is_none() {
                sqlx::query(
                    "update porchlight.household_members set
deleted_at=now(),tz_authority_device_id=null where id=$1",
                )
                .bind(id)
                .execute(&mut *d)
                .await?;
            }
            Ok(Json(user(&mut d, c.household_id(), id).await?).into_response())
        }
        .await;
        match result {
            Ok(r) => {
                d.commit().await?;
                for (id, fps) in revoked {
                    tx.publish_revocation(id, fps);
                }
                Ok(r)
            }
            Err(e) => {
                d.rollback().await?;
                Err(e)
            }
        }
    })
}
#[utoipa::path(security(("pat"=[])),post,path="/v1/admin/items/{id}/revert",request_body=RevertRequest,params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),responses(super::items::IfMatchProblem,(status=200,body=super::items::ItemResponse,description="Restored through ordinary derivation"),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem))]
pub fn revert<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: RevertRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(crate::items::revert(c, tx, b.revision_id, (s.item_clock)()))
}
#[utoipa::path(security(("pat"=[])),get,path="/v1/admin/items/{id}/revisions",params(("id"=Uuid,Path),("cursor"=Option<String>,Query),("limit"=Option<u16>,Query)),responses((status=200,body=RevisionsPage,description="Revision snapshots"),crate::openapi::UniversalProblems,crate::openapi::NotFoundProblem))]
pub async fn revisions(
    State(s): State<AppState>,
    c: Caller,
    Path(id): Path<Uuid>,
    ApiQuery(page): ApiQuery<PageParams>,
) -> Result<Json<Page<RevisionResponse>>, ApiError> {
    let mut conn = s.pool.acquire().await?;
    crate::items::response(&mut conn, &c, id, true, vec![]).await?;
    let limit = page.limit()?;
    let context = format!("revisions:{}:{id}", c.household_id());
    let after: Option<i32> = page
        .cursor
        .as_deref()
        .map(|v| s.idem.cursor.decode(&context, v))
        .transpose()?;
    let rows = sqlx::query_as(
        "select id,item_id,rev,snapshot,actor,created_at from
porchlight.item_revisions where household_id=$1 and
item_id=$2 and ($3::int is null or rev>$3) order by rev
limit $4",
    )
    .bind(c.household_id())
    .bind(id)
    .bind(after)
    .bind((limit + 1) as i64)
    .fetch_all(&mut *conn)
    .await?;
    Ok(Json(s.idem.cursor.page(
        &context,
        rows,
        limit,
        |r: &RevisionResponse| r.rev,
    )?))
}
#[utoipa::path(security(("pat"=[])),post,path="/v1/admin/purge",request_body=PurgeRequest,responses((status=200,body=PurgeResponse,description="Audited physical deletion, maximum 200 items"),(status=409,body=crate::openapi::ProblemBody,description="purge-dependency-unhandled: an unhandled relation with selected dependent rows or undeclared privilege requires a retention migration; detail names the relation"),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem))]
pub fn purge<'a>(
    _s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: PurgeRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        c.require_admin()?;
        if b.item_ids.is_some() == b.deleted_before.is_some() {
            return Err(invalid("item_ids"));
        }
        let mut d = tx.tx.begin().await?;
        let result = async {
            let ids = if let Some(ids) = b.item_ids {
                ids
            } else {
                sqlx::query_scalar(
                    "select id from porchlight.items where household_id=$1 and
deleted_at<$2 order by id limit 201",
                )
                .bind(c.household_id())
                .bind(b.deleted_before)
                .fetch_all(&mut *d)
                .await?
            };
            if ids.is_empty() {
                return Err(invalid("item_ids"));
            }
            if ids.len() > 200 {
                return Err(ApiError::validation(vec![FieldError {
                    field: "item_ids".into(),
                    message:
                        "At most 200 items per purge; select a smaller batch with explicit item_ids"
                            .into(),
                    code: None,
                }]));
            }
            let value: Value = sqlx::query_scalar("select porchlight.pl_purge_items($1,$2)")
                .bind(c.household_id())
                .bind(ids)
                .fetch_one(&mut *d)
                .await
                .map_err(|e| {
                    if e.as_database_error().and_then(|e| e.code()).as_deref() == Some("PL422") {
                        invalid("item_ids")
                    } else if e.as_database_error().and_then(|e| e.code()).as_deref()
                        == Some("PL409")
                    {
                        ApiError::new(Problem::PurgeDependencyUnhandled).detail(
                            e.as_database_error()
                                .map(|d| d.message())
                                .unwrap_or("Purge dependency unhandled"),
                        )
                    } else {
                        e.into()
                    }
                })?;
            Ok(Json(value).into_response())
        }
        .await;
        match result {
            Ok(r) => {
                d.commit().await?;
                Ok(r)
            }
            Err(e) => {
                d.rollback().await?;
                Err(e)
            }
        }
    })
}
#[utoipa::path(security(("pat"=[])),get,path="/v1/admin/derive",params(("cursor"=Option<String>,Query),("limit"=Option<u16>,Query)),responses((status=200,body=DerivationStatus,description="Pending count and failing jobs"),crate::openapi::UniversalProblems))]
pub async fn derive_status(
    State(s): State<AppState>,
    c: Caller,
    ApiQuery(page): ApiQuery<PageParams>,
) -> Result<Json<DerivationStatus>, ApiError> {
    let limit = page.limit()?;
    let context = format!("derive:{}", c.household_id());
    let after: Option<(DateTime<Utc>, Uuid)> = page
        .cursor
        .as_deref()
        .map(|v| s.idem.cursor.decode(&context, v))
        .transpose()?;
    let pending_count = sqlx::query_scalar(
        "select count(*) from porchlight.item_derivation where
household_id=$1 and applied_gen<requested_gen",
    )
    .bind(c.household_id())
    .fetch_one(&s.pool)
    .await?;
    // Jobs have stable item creation cursors; no mutable retry timestamp cursors.
    let rows: Vec<FailureCursorRow> = sqlx::query_as(
        "select
d.item_id,d.attempts,d.last_error,d.retry_at,i.created_at
from porchlight.item_derivation d join porchlight.items i on
i.id=d.item_id where d.household_id=$1 and d.last_error is
not null and ($2::timestamptz is null or
(i.created_at,d.item_id)>($2,$3)) order by
i.created_at,d.item_id limit $4",
    )
    .bind(c.household_id())
    .bind(after.map(|x| x.0))
    .bind(after.map(|x| x.1))
    .bind((limit + 1) as i64)
    .fetch_all(&s.pool)
    .await?;
    let page = s.idem.cursor.page(&context, rows, limit, |r| (r.4, r.0))?;
    Ok(Json(DerivationStatus {
        pending_count,
        items: page
            .items
            .into_iter()
            .map(|r| DerivationFailure {
                item_id: r.0,
                attempts: r.1,
                last_error: r.2,
                retry_at: r.3,
            })
            .collect(),
        next_cursor: page.next_cursor,
    }))
}
#[utoipa::path(security(("pat"=[])),post,path="/v1/admin/derive/{id}/retry",request_body=super::devices::Empty,params(("id"=Uuid,Path)),responses((status=200,description="Retry reset; queued indicates a pending job",body=DerivationRetry),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::NotFoundProblem))]
pub fn derive_retry<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    _b: super::devices::Empty,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        let id = target(tx)?;
        let found: Option<Uuid> = sqlx::query_scalar(
            "select id from porchlight.items where household_id=$1 and
id=$2 for no key update",
        )
        .bind(c.household_id())
        .bind(id)
        .fetch_optional(&mut *tx.tx)
        .await?;
        if found.is_none() {
            return Err(ApiError::new(Problem::NotFound));
        }
        let queued: Option<bool> = sqlx::query_scalar(
            "update porchlight.item_derivation set
attempts=0,retry_at=$2,last_error=null where item_id=$1
returning applied_gen<requested_gen",
        )
        .bind(id)
        .bind((s.item_clock)())
        .fetch_optional(&mut *tx.tx)
        .await?;
        sqlx::query("select pg_notify('porchlight_derive','')")
            .execute(&mut *tx.tx)
            .await?;
        Ok(Json(DerivationRetry {
            item_id: id,
            queued: queued.unwrap_or(false),
        })
        .into_response())
    })
}
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        users,
        add_user,
        edit_user,
        delete_user,
        revert,
        revisions,
        purge,
        derive_status,
        derive_retry
    ),
    components(schemas(
        UserResponse,
        AddUser,
        EditUser,
        RevertRequest,
        PurgeRequest,
        PurgeResponse,
        PurgeCounts,
        RevisionActor,
        RevisionSnapshot,
        UsersPage,
        RevisionsPage,
        DerivationFailure,
        DerivationStatus,
        DerivationRetry,
        RevisionResponse
    ))
)]
pub struct AdministrationApi;

fn agent_policy(c: &crate::permissions::PolicyContext) -> u16 {
    use crate::permissions::PatScope;
    if c.item.caller != porchlight_core::access::CallerKind::Agent {
        return 404;
    }
    if c.pat_scope != Some(PatScope::Admin) {
        return 403;
    }
    200
}
fn agent_create_policy(c: &crate::permissions::PolicyContext) -> u16 {
    match agent_policy(c) {
        200 => 201,
        status => status,
    }
}
fn agent_read_policy(c: &crate::permissions::PolicyContext) -> u16 {
    if c.item.caller == porchlight_core::access::CallerKind::Agent {
        200
    } else {
        404
    }
}
fn agent_write_policy(c: &crate::permissions::PolicyContext) -> u16 {
    if agent_read_policy(c) == 404 {
        return 404;
    }
    if c.pat_scope == Some(crate::permissions::PatScope::Read) {
        403
    } else {
        200
    }
}
pub const ENDPOINTS: &[crate::permissions::Endpoint] = &[
    crate::permissions::Endpoint {
        method: "GET",
        path: "/v1/admin/users",
        handler: "users",
        case: "",
        policy: crate::permissions::Policy::Custom {
            label: "admin PAT",
            expected: agent_policy,
        },
    },
    crate::permissions::Endpoint {
        method: "POST",
        path: "/v1/admin/users",
        handler: "add_user",
        case: "",
        policy: crate::permissions::Policy::Custom {
            label: "admin PAT",
            expected: agent_create_policy,
        },
    },
    crate::permissions::Endpoint {
        method: "PATCH",
        path: "/v1/admin/users/{id}",
        handler: "edit_user",
        case: "",
        policy: crate::permissions::Policy::Custom {
            label: "admin PAT",
            expected: agent_policy,
        },
    },
    crate::permissions::Endpoint {
        method: "DELETE",
        path: "/v1/admin/users/{id}",
        handler: "delete_user",
        case: "",
        policy: crate::permissions::Policy::Custom {
            label: "admin PAT",
            expected: agent_policy,
        },
    },
    crate::permissions::Endpoint {
        method: "POST",
        path: "/v1/admin/items/{id}/revert",
        handler: "revert",
        case: "",
        policy: crate::permissions::Policy::Custom {
            label: "write PAT",
            expected: agent_write_policy,
        },
    },
    crate::permissions::Endpoint {
        method: "GET",
        path: "/v1/admin/items/{id}/revisions",
        handler: "revisions",
        case: "",
        policy: crate::permissions::Policy::Custom {
            label: "read PAT",
            expected: agent_read_policy,
        },
    },
    crate::permissions::Endpoint {
        method: "POST",
        path: "/v1/admin/purge",
        handler: "purge",
        case: "",
        policy: crate::permissions::Policy::Custom {
            label: "admin PAT",
            expected: agent_policy,
        },
    },
    crate::permissions::Endpoint {
        method: "GET",
        path: "/v1/admin/derive",
        handler: "derive_status",
        case: "",
        policy: crate::permissions::Policy::Custom {
            label: "read PAT",
            expected: agent_read_policy,
        },
    },
    crate::permissions::Endpoint {
        method: "POST",
        path: "/v1/admin/derive/{id}/retry",
        handler: "derive_retry",
        case: "",
        policy: crate::permissions::Policy::Custom {
            label: "write PAT",
            expected: agent_write_policy,
        },
    },
];
