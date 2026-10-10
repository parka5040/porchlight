//! Item HTTP wire types and registry callbacks. SQL and policy live in the service.
use crate::{error::ApiError, extract::Caller, idempotency::IdemTx, state::AppState};
use axum::{
    Json,
    extract::{Path, Query, State},
    response::Response,
};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Clone, Debug, Default, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct CompletionInterval {
    pub months: i32,
    pub days: i32,
    pub microseconds: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ItemFields {
    pub title: String,
    pub notes: String,
    pub priority: i16,
    pub owner_id: Uuid,
    pub subject_member_id: Option<Uuid>,
    #[schema(value_type=ItemVisibility)]
    pub visibility: String,
    pub address: Option<String>,
    pub source: Option<String>,
    pub buffer_before_min: i32,
    pub buffer_after_min: i32,
    pub est_minutes: Option<i32>,
    #[schema(value_type=ItemKind)]
    pub kind: String,
    #[schema(value_type=ItemTimeMode)]
    pub time_mode: String,
    pub tzid: Option<String>,
    pub due_date: Option<NaiveDate>,
    pub due_at: Option<DateTime<Utc>>,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_due: Option<NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_start: Option<NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_end: Option<NaiveDateTime>,
    pub all_day: bool,
    pub rrule: Option<String>,
    #[schema(value_type=Option<ItemRecurrenceMode>)]
    pub recurrence_mode: Option<String>,
    pub completion_interval: Option<CompletionInterval>,
    #[schema(value_type=ItemMissedPolicy)]
    pub missed_policy: String,
}
impl Default for ItemFields {
    fn default() -> Self {
        Self {
            title: String::new(),
            notes: String::new(),
            priority: 0,
            owner_id: Uuid::nil(),
            subject_member_id: None,
            visibility: "household".into(),
            address: None,
            source: None,
            buffer_before_min: 0,
            buffer_after_min: 0,
            est_minutes: None,
            kind: "todo".into(),
            time_mode: "fixed".into(),
            tzid: None,
            due_date: None,
            due_at: None,
            starts_at: None,
            ends_at: None,
            local_due: None,
            local_start: None,
            local_end: None,
            all_day: false,
            rrule: None,
            recurrence_mode: None,
            completion_interval: None,
            missed_policy: "carry_over".into(),
        }
    }
}
#[derive(Serialize, ToSchema)]
pub struct ItemResponse {
    pub id: Uuid,
    pub household_id: Uuid,
    pub version: i32,
    #[serde(flatten)]
    pub fields: ItemFields,
    pub assignees: Vec<Uuid>,
    pub viewers: Vec<Uuid>,
    pub reminder_offsets: Vec<i32>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub series_id: Option<Uuid>,
    pub split_from_item_id: Option<Uuid>,
    pub split_from_seq: Option<i32>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub nominal_local_anchor: Option<NaiveDateTime>,
    pub nominal_wall_duration: Option<i64>,
    #[schema(value_type=Vec<ItemCapability>)]
    pub capabilities: Vec<String>,
    pub pending_derivation: bool,
    pub conflicts: Vec<ConflictResponse>,
}
#[derive(Serialize, ToSchema)]
pub struct ConflictResponse {
    pub member_id: Uuid,
    pub occurrence_id: Uuid,
    pub other_occurrence_id: Option<Uuid>,
    pub other_item_id: Option<Uuid>,
    pub other_title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    #[schema(value_type=ConflictKind)]
    pub kind: String,
}
#[derive(Serialize, ToSchema)]
pub struct OccurrenceResponse {
    pub id: Uuid,
    pub item_id: Uuid,
    pub seq: u32,
    pub version: i32,
    #[schema(value_type=OccurrenceStatus)]
    pub status: String,
    pub disposition: String,
    pub board_position: String,
    pub completed_by: Option<Uuid>,
    pub completed_at: Option<DateTime<Utc>>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_start: Option<NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_end: Option<NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_due: Option<NaiveDateTime>,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    pub due_at: Option<DateTime<Utc>>,
    pub due_date: Option<NaiveDate>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub override_local_start: Option<NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub override_local_end: Option<NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub override_local_due: Option<NaiveDateTime>,
    pub override_due_date: Option<NaiveDate>,
    pub is_detached: bool,
    #[schema(value_type=Vec<ItemCapability>)]
    pub capabilities: Vec<String>,
    pub pending_derivation: bool,
    pub conflicts: Vec<ConflictResponse>,
}
#[derive(ToSchema)]
pub struct CreateRequest {
    #[serde(flatten)]
    pub fields: ItemFields,
    pub assignees: Option<Vec<Uuid>>,
    pub viewers: Option<Vec<Uuid>>,
    pub reminder_offsets: Option<Vec<i32>>,
}
#[derive(ToSchema)]
pub struct PatchRequest {
    pub title: Option<String>,
    pub notes: Option<String>,
    pub priority: Option<i16>,
    pub owner_id: Option<Uuid>,
    pub subject_member_id: Option<Uuid>,
    #[schema(value_type=Option<ItemVisibility>)]
    pub visibility: Option<String>,
    pub address: Option<String>,
    pub source: Option<String>,
    pub buffer_before_min: Option<i32>,
    pub buffer_after_min: Option<i32>,
    pub est_minutes: Option<i32>,
    #[schema(value_type=Option<ItemKind>)]
    pub kind: Option<String>,
    #[schema(value_type=Option<ItemTimeMode>)]
    pub time_mode: Option<String>,
    pub tzid: Option<String>,
    pub due_date: Option<NaiveDate>,
    pub due_at: Option<DateTime<Utc>>,
    pub starts_at: Option<DateTime<Utc>>,
    pub ends_at: Option<DateTime<Utc>>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_due: Option<NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_start: Option<NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub local_end: Option<NaiveDateTime>,
    pub all_day: Option<bool>,
    pub rrule: Option<String>,
    #[schema(value_type=Option<ItemRecurrenceMode>)]
    pub recurrence_mode: Option<String>,
    pub completion_interval: Option<CompletionInterval>,
    #[schema(value_type=Option<ItemMissedPolicy>)]
    pub missed_policy: Option<String>,
    /// all (default) edits only this series, including when this is a split tail.
    /// this_and_future re-anchors a new tail at occurrence_id; timing fields apply
    /// verbatim to the resulting tail, not to the original head.
    pub scope: Option<String>,
    pub occurrence_id: Option<Uuid>,
}
#[derive(ToSchema)]
pub struct AssigneesRequest {
    pub assignees: Vec<Uuid>,
}
#[derive(ToSchema)]
pub struct ViewersRequest {
    pub viewers: Vec<Uuid>,
}
#[derive(ToSchema)]
pub struct RemindersRequest {
    pub reminder_offsets: Vec<i32>,
}
#[derive(ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyRequest {}
#[derive(ToSchema)]
pub struct BoardPosition {
    /// Predecessor. Insert immediately after it in the household board projection.
    pub after_id: Option<Uuid>,
    /// Successor when no predecessor; otherwise only validates ordering of the bounds.
    pub before_id: Option<Uuid>,
}
#[derive(ToSchema)]
pub struct OccurrencePatch {
    #[schema(value_type=Option<OccurrenceStatus>)]
    pub status: Option<String>,
    pub after_id: Option<Uuid>,
    pub before_id: Option<Uuid>,
    /// Presence requests a move even with no bounds (end of column).
    pub board_move: Option<BoardPosition>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub override_local_start: Option<NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub override_local_end: Option<NaiveDateTime>,
    #[schema(value_type=Option<String>, format="local-date-time", pattern=r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,9})?$")]
    pub override_local_due: Option<NaiveDateTime>,
    pub override_due_date: Option<NaiveDate>,
}
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    pub cursor: Option<String>,
    pub limit: Option<u16>,
    pub updated_since: Option<DateTime<Utc>>,
    pub kind: Option<String>,
    pub include_deleted: Option<bool>,
}
#[derive(Serialize, ToSchema)]
pub struct ItemPage {
    pub items: Vec<ItemResponse>,
    pub next_cursor: Option<String>,
}
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OccurrencesQuery {
    /// Include undated rows alongside the bounded time window (same item ACL).
    pub undated: Option<bool>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
}
#[derive(Serialize, ToSchema)]
pub struct OccurrencePage {
    pub items: Vec<OccurrenceResponse>,
    pub next_cursor: Option<String>,
}

#[utoipa::path(get,path="/v1/items",params(ListQuery),responses((status=200,description="Visible items in UUID order",body=ItemPage),crate::openapi::UniversalProblems,ReadValidationProblem,crate::openapi::ForbiddenProblem))]
pub async fn list(
    State(s): State<AppState>,
    c: Caller,
    query: Result<Query<ListQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<ItemPage>, ApiError> {
    let Query(q) = query.map_err(|_| ApiError::new(crate::error::Problem::InvalidRequest))?;
    crate::items::list(&s, &c, q).await.map(Json)
}
#[utoipa::path(get,path="/v1/items/{id}",params(("id"=Uuid,Path)),responses((status=200,description="Item",body=ItemResponse),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem))]
pub async fn get(
    State(s): State<AppState>,
    c: Caller,
    path: Result<Path<Uuid>, axum::extract::rejection::PathRejection>,
) -> Result<Json<ItemResponse>, ApiError> {
    let Path(id) = path.map_err(|_| ApiError::new(crate::error::Problem::InvalidRequest))?;
    let mut conn = crate::items::read_snapshot(&s).await?;
    crate::items::response(&mut conn, &c, id, false, vec![])
        .await
        .map(Json)
}
#[utoipa::path(get,path="/v1/items/{id}/occurrences",params(("id"=Uuid,Path),OccurrencesQuery),responses((status=200,description="Occurrences overlapping optional UTC window; undated=true also includes undated rows",body=OccurrencePage),crate::openapi::UniversalProblems,ReadValidationProblem,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem))]
pub async fn occurrences(
    State(s): State<AppState>,
    c: Caller,
    path: Result<Path<Uuid>, axum::extract::rejection::PathRejection>,
    query: Result<Query<OccurrencesQuery>, axum::extract::rejection::QueryRejection>,
    axum::Extension(tz): axum::Extension<crate::protocol::ClientTz>,
) -> Result<Json<OccurrencePage>, ApiError> {
    let Path(id) = path.map_err(|_| ApiError::new(crate::error::Problem::InvalidRequest))?;
    let Query(q) = query.map_err(|_| ApiError::new(crate::error::Problem::InvalidRequest))?;
    crate::items::occurrences_with_zone(&s, &c, id, q, tz.0)
        .await
        .map(Json)
}

#[utoipa::path(post,path="/v1/items",request_body=CreateRequest,responses((status=201,description="Success with current capabilities and masked conflicts",body=ItemResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem))]
pub fn create<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    body: serde_json::Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(crate::items::mutate(
        c,
        tx,
        body,
        crate::items::Operation::Create,
        (s.item_clock)(),
    ))
}
#[utoipa::path(patch,path="/v1/items/{id}",params(("id"=Uuid,Path),EditQuery,crate::openapi::IfMatchHeader),request_body=PatchRequest,responses((status=204,description="Mutation succeeded; resource no longer visible or present"),(status=200,description="Success with current capabilities and masked conflicts",body=ItemResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,IfMatchProblem,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem))]
pub fn patch<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    body: serde_json::Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(crate::items::mutate(
        c,
        tx,
        body,
        crate::items::Operation::Patch,
        (s.item_clock)(),
    ))
}
#[utoipa::path(delete,path="/v1/items/{id}",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=EmptyRequest,responses((status=204,description="Mutation succeeded; resource no longer visible or present"),(status=200,description="Success with current capabilities and masked conflicts",body=ItemResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,IfMatchProblem,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem))]
pub fn delete<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    body: serde_json::Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(crate::items::mutate(
        c,
        tx,
        body,
        crate::items::Operation::Delete,
        (s.item_clock)(),
    ))
}
#[utoipa::path(put,path="/v1/items/{id}/assignees",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=AssigneesRequest,responses((status=204,description="Mutation succeeded; resource no longer visible or present"),(status=200,description="Success with current capabilities and masked conflicts",body=ItemResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,IfMatchProblem,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem))]
pub fn assignees<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    body: serde_json::Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(crate::items::mutate(
        c,
        tx,
        body,
        crate::items::Operation::Assignees,
        (s.item_clock)(),
    ))
}
#[utoipa::path(put,path="/v1/items/{id}/viewers",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=ViewersRequest,responses((status=204,description="Mutation succeeded; resource no longer visible or present"),(status=200,description="Success with current capabilities and masked conflicts",body=ItemResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,IfMatchProblem,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem))]
pub fn viewers<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    body: serde_json::Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(crate::items::mutate(
        c,
        tx,
        body,
        crate::items::Operation::Viewers,
        (s.item_clock)(),
    ))
}
#[utoipa::path(put,path="/v1/items/{id}/reminders",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=RemindersRequest,responses((status=204,description="Mutation succeeded; resource no longer visible or present"),(status=200,description="Success with current capabilities and masked conflicts",body=ItemResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,IfMatchProblem,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem))]
pub fn reminders<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    body: serde_json::Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(crate::items::mutate(
        c,
        tx,
        body,
        crate::items::Operation::Reminders,
        (s.item_clock)(),
    ))
}
#[utoipa::path(post,path="/v1/items/{id}/take",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=EmptyRequest,responses((status=204,description="Mutation succeeded; resource no longer visible or present"),(status=200,description="Success with current capabilities and masked conflicts",body=ItemResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,IfMatchProblem,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem))]
pub fn take<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    body: serde_json::Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(crate::items::mutate(
        c,
        tx,
        body,
        crate::items::Operation::Take,
        (s.item_clock)(),
    ))
}
#[utoipa::path(patch,path="/v1/occurrences/{id}",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=OccurrencePatch,responses((status=204,description="Mutation succeeded; resource no longer visible or present"),(status=200,description="Success with current capabilities and masked conflicts",body=OccurrenceResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,IfMatchProblem,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem))]
pub fn patch_occurrence<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    body: serde_json::Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(crate::items::mutate(
        c,
        tx,
        body,
        crate::items::Operation::PatchOccurrence,
        (s.item_clock)(),
    ))
}
#[utoipa::path(post,path="/v1/occurrences/{id}/complete",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=EmptyRequest,responses((status=204,description="Mutation succeeded; resource no longer visible or present"),(status=200,description="Success with current capabilities and masked conflicts",body=OccurrenceResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,IfMatchProblem,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem))]
pub fn complete<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    body: serde_json::Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(crate::items::mutate(
        c,
        tx,
        body,
        crate::items::Operation::Complete,
        (s.item_clock)(),
    ))
}
#[utoipa::path(post,path="/v1/occurrences/{id}/reopen",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=EmptyRequest,responses((status=204,description="Mutation succeeded; resource no longer visible or present"),(status=200,description="Success with current capabilities and masked conflicts",body=OccurrenceResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,IfMatchProblem,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem))]
pub fn reopen<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    body: serde_json::Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(crate::items::mutate(
        c,
        tx,
        body,
        crate::items::Operation::Reopen,
        (s.item_clock)(),
    ))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(
    list,
    get,
    occurrences,
    create,
    patch,
    delete,
    assignees,
    viewers,
    reminders,
    take,
    patch_occurrence,
    complete,
    reopen
))]
pub struct ItemsApi;

#[derive(utoipa::IntoResponses)]
#[response(
    status = 428,
    description = "If-Match is required",
    content_type = "application/problem+json"
)]
#[allow(dead_code)]
pub struct IfMatchProblem(crate::openapi::ProblemBody);

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct EditQuery {
    /// all (default) or this_and_future; may also be specified in the body.
    pub scope: Option<String>,
    pub occurrence_id: Option<Uuid>,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Todo,
    Deadline,
    Appointment,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemTimeMode {
    Fixed,
    Floating,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemRecurrenceMode {
    Calendar,
    Completion,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemMissedPolicy {
    CarryOver,
    Skip,
    Stack,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OccurrenceStatus {
    Backlog,
    Todo,
    Doing,
    Done,
    Skipped,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemVisibility {
    Household,
    Private,
    Selected,
}
#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ItemCapability {
    View,
    Comment,
    ChangeStatus,
    Complete,
    Snooze,
    Edit,
    EditReminders,
    Reassign,
    ChangeVisibility,
    Take,
    Delete,
}

#[derive(ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    Hard,
    Load,
}

#[derive(utoipa::IntoResponses)]
#[response(
    status = 422,
    description = "Validation failed; errors carry wire field names",
    content_type = "application/problem+json"
)]
#[allow(dead_code)]
pub struct ReadValidationProblem(crate::openapi::ProblemBody);
