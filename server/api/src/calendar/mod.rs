//! ICS persistence, feed administration and shared read/notification contributions.
pub mod fetch;
pub mod jobs;
pub mod permissions;
use crate::{
    error::{ApiError, Problem},
    extract::Caller,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, types::Json};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Source {
    pub id: Uuid,
    pub household_id: Uuid,
    pub owner_member_id: Uuid,
    pub target_member_id: Option<Uuid>,
    pub kind: String,
    #[schema(value_type = SourceVisibility)]
    pub visibility: String,
    pub import_key: String,
    pub name: String,
    pub url: Option<String>,
    pub allow_http: bool,
    pub refresh_seconds: i32,
    pub enabled: bool,
    pub next_fetch_at: DateTime<Utc>,
    pub last_fetch_at: Option<DateTime<Utc>>,
    /// queued, ok, partial, not_modified, failed, or retrying (first two consecutive deadline failures).
    pub last_status: Option<String>,
    /// Static failure category; calendar_too_large is admission; apply_retryable/apply_timeout are deadline categories.
    pub last_error: Option<String>,
    pub last_warnings: Vec<Warning>,
    pub failure_count: i32,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub version: i32,
    pub deleted_at: Option<DateTime<Utc>>,
}
#[derive(Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    pub content: String,
    pub member_id: Uuid,
    #[serde(default = "import_name")]
    pub name: String,
    /// Stable caller/target-local identity scope (1–200 characters, no controls). Reuse to update; use another for a distinct import.
    #[serde(default = "import_name")]
    pub scope: String,
    #[serde(default)]
    pub preview: bool,
    #[serde(default)]
    #[schema(value_type = Option<SourceVisibility>)]
    pub visibility: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportQuery {
    pub member_id: Uuid,
    #[serde(default = "import_name")]
    pub name: String,
    #[serde(default = "import_name")]
    pub scope: String,
    #[serde(default)]
    pub preview: bool,
    #[serde(default)]
    pub visibility: Option<String>,
}
fn import_name() -> String {
    "import".into()
}
#[derive(Default, Serialize, utoipa::ToSchema)]
pub struct ImportSummary {
    pub created: u32,
    pub updated: u32,
    pub unchanged: u32,
    pub removed: u32,
    pub deleted: u32,
    /// Total skipped, including invalid events and cancelled/EXDATE occurrences (legacy aggregate).
    pub skipped: u32,
    /// Invalid events skipped with InvalidEvent warnings.
    #[serde(default)]
    pub invalid_skipped: u32,
    /// Cancelled overrides and EXDATE occurrences, not invalid events.
    #[serde(default)]
    pub cancelled_skipped: u32,
    pub warnings: Vec<Warning>,
    pub source_id: Option<Uuid>,
    pub item_ids: Vec<Uuid>,
    pub conflicts: Vec<crate::routes::items::ConflictResponse>,
    pub preview: bool,
    pub removal_suppressed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Warning {
    pub uid: Option<String>,
    pub code: String,
    pub message: String,
}
#[derive(Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceRequest {
    pub name: String,
    pub url: String,
    pub target_member_id: Option<Uuid>,
    #[serde(default)]
    #[schema(value_type = Option<SourceVisibility>)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub allow_http: bool,
    #[serde(default = "refresh_default")]
    pub refresh_seconds: i32,
    #[serde(default = "enabled_default")]
    pub enabled: bool,
}
fn refresh_default() -> i32 {
    86400
}
fn enabled_default() -> bool {
    true
}
pub async fn load_source(
    conn: &mut PgConnection,
    household: Uuid,
    id: Uuid,
    lock: bool,
) -> Result<Source, ApiError> {
    let query = if lock {
        "select to_jsonb(s) from porchlight.ics_sources s where household_id=$1 and id=$2 and deleted_at is null for no key update"
    } else {
        "select to_jsonb(s) from porchlight.ics_sources s where household_id=$1 and id=$2 and deleted_at is null"
    };
    sqlx::query_scalar::<_, Json<Source>>(sqlx::AssertSqlSafe(query))
        .bind(household)
        .bind(id)
        .fetch_optional(conn)
        .await?
        .map(|v| v.0)
        .ok_or_else(|| ApiError::new(Problem::NotFound))
}
pub async fn target_zone(
    conn: &mut PgConnection,
    household: Uuid,
    target: Option<Uuid>,
) -> Result<chrono_tz::Tz, ApiError> {
    let zone: Option<String> = match target {
        Some(id) => sqlx::query_scalar("select case when m.role='dependent' then h.tzid else m.tzid end from porchlight.household_members m join porchlight.households h on h.id=m.household_id where m.household_id=$1 and m.id=$2 and m.deleted_at is null").bind(household).bind(id).fetch_optional(&mut *conn).await?,
        None => sqlx::query_scalar("select tzid from porchlight.households where id=$1 and deleted_at is null").bind(household).fetch_optional(&mut *conn).await?,
    };
    zone.ok_or_else(|| member_reference("target_member_id"))?
        .parse()
        .map_err(|_| ApiError::internal("calendar_zone"))
}
pub fn member_reference(field: &str) -> ApiError {
    crate::items::reference_error(field, "invalid-reference")
}
/// Source-controlled calendars have no selected-viewer list. Input stays a string
/// until validation so unknown values produce a 422 field error, not JSON's 400.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceVisibility {
    Private,
    Household,
}
pub fn validate_visibility(value: &str) -> Result<(), ApiError> {
    if matches!(value, "private" | "household") {
        return Ok(());
    }
    Err(ApiError::validation(vec![crate::error::FieldError {
        field: "visibility".into(),
        message: "Expected private or household".into(),
        code: None,
    }]))
}
pub async fn dependent_target(
    conn: &mut PgConnection,
    household: Uuid,
    target: Option<Uuid>,
) -> Result<bool, ApiError> {
    match target {
        None => Ok(false),
        Some(id) => sqlx::query_scalar("select role='dependent' from porchlight.household_members where household_id=$1 and id=$2 and deleted_at is null")
            .bind(household).bind(id).fetch_optional(conn).await?.ok_or_else(|| member_reference("target_member_id")),
    }
}
pub async fn source_owner(conn: &mut PgConnection, source: &Source) -> Result<Uuid, ApiError> {
    Ok(
        if dependent_target(conn, source.household_id, source.target_member_id).await? {
            source.owner_member_id
        } else {
            source.target_member_id.unwrap_or(source.owner_member_id)
        },
    )
}
pub async fn source_visibility(
    conn: &mut PgConnection,
    household: Uuid,
    target: Option<Uuid>,
    requested: Option<&str>,
) -> Result<String, ApiError> {
    if let Some(v) = requested {
        validate_visibility(v)?;
    }
    let dependent = dependent_target(conn, household, target).await?;
    Ok(requested
        .unwrap_or(if target.is_none() || dependent {
            "household"
        } else {
            "private"
        })
        .into())
}
impl SourceRequest {
    pub fn visibility(&self) -> String {
        self.visibility.clone().unwrap_or_else(|| {
            if self.target_member_id.is_some() {
                "private".into()
            } else {
                "household".into()
            }
        })
    }
}
pub fn validate_source(body: &SourceRequest) -> Result<(), ApiError> {
    validate_name(&body.name)?;
    validate_visibility(&body.visibility())?;
    if !(60..=2592000).contains(&body.refresh_seconds) {
        return Err(ApiError::new(Problem::ValidationFailed));
    }
    fetch::validate_url(&body.url, body.allow_http)
        .map_err(|_| ApiError::new(Problem::ValidationFailed))?;
    Ok(())
}
pub fn validate_name(name: &str) -> Result<(), ApiError> {
    // Item badges include the four-character `ics:` prefix.
    if name.is_empty() || name.chars().count() > 196 || name.chars().any(char::is_control) {
        return Err(ApiError::validation(vec![crate::error::FieldError {
            field: "name".into(),
            message: "Expected 1–196 characters without control characters".into(),
            code: None,
        }]));
    }
    Ok(())
}
/// Safe device-cache projection: only the URL host and categorical fetch status.
/// Full subscription URLs, validators and remote error/warning details are admin-route only.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SourceSnapshot {
    pub id: Uuid,
    pub name: String,
    pub host: Option<String>,
    pub enabled: bool,
    pub last_status: Option<String>,
    pub last_fetch_at: Option<DateTime<Utc>>,
    pub version: i32,
}
/// Admin-only contribution, called within the shared snapshot RR transaction.
pub async fn snapshot_sources(
    conn: &mut PgConnection,
    caller: &Caller,
) -> Result<Vec<SourceSnapshot>, ApiError> {
    if caller.require_admin().is_err() {
        return Ok(Vec::new());
    }
    #[derive(sqlx::FromRow)]
    struct SnapshotRow {
        id: Uuid,
        name: String,
        url: Option<String>,
        enabled: bool,
        last_status: Option<String>,
        last_fetch_at: Option<DateTime<Utc>>,
        version: i32,
    }
    let rows: Vec<SnapshotRow> =
        sqlx::query_as("select id,name,url,enabled,last_status,last_fetch_at,version from porchlight.ics_sources where household_id=$1 and kind='feed' and deleted_at is null order by id")
        .bind(caller.household_id()).fetch_all(conn).await?;
    Ok(rows
        .into_iter()
        .map(|s| SourceSnapshot {
            id: s.id,
            name: s.name,
            host: s
                .url
                .and_then(|u| url::Url::parse(&u).ok())
                .and_then(|u| u.host_str().map(str::to_owned)),
            enabled: s.enabled,
            last_status: s.last_status,
            last_fetch_at: s.last_fetch_at,
            version: s.version,
        })
        .collect())
}
/// Emit only changed imported/feed items, in one batch through the shared producer ABI.
/// Owners/assignees receive business changes only while still allowed to read.
/// Deleted items remain represented by audit/outbox eviction, not content notifications.
pub async fn emit_import_notifications(
    conn: &mut PgConnection,
    caller: &Caller,
    items: &[Uuid],
) -> Result<(), ApiError> {
    if items.is_empty() {
        return Ok(());
    }
    sqlx::query("select porchlight.pl_emit_notification_event(i.household_id,m.id,'calendar_changed','item',i.id,jsonb_build_object('item_id',i.id),i.id)
        from porchlight.items i join porchlight.household_members m on m.household_id=i.household_id
        where i.household_id=$1 and i.id=any($2) and i.deleted_at is null
        and m.deleted_at is null and m.role<>'dependent'
        and (m.id=i.owner_id or exists(select 1 from porchlight.item_assignees a where a.item_id=i.id and a.member_id=m.id))
        and porchlight.pl_item_can_read(i.id,m.id)")
        .bind(caller.household_id()).bind(items).execute(conn).await?;
    Ok(())
}

pub fn validate_scope(scope: &str) -> Result<(), ApiError> {
    if scope.is_empty() || scope.chars().count() > 200 || scope.chars().any(char::is_control) {
        return Err(ApiError::validation(vec![crate::error::FieldError {
            field: "scope".into(),
            message: "Expected 1–200 characters without control characters".into(),
            code: None,
        }]));
    }
    Ok(())
}
