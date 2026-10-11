//! S8b planning routes, on both authenticated surfaces via the central registry.
use super::items::ConflictResponse;
use crate::{error::ApiError, extract::Caller, idempotency::IdemTx, state::AppState};
use axum::{
    Json,
    extract::{Path, Query, State},
    response::Response,
};
use chrono::{DateTime, NaiveTime, Utc};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, OpenApi, ToSchema};
use uuid::Uuid;
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Window {
    /// Monday=0 through Sunday=6. Equal endpoints are empty; end<start is overnight.
    #[schema(minimum = 0, maximum = 6)]
    pub weekday: i16,
    #[schema(value_type=String,format="local-time",example="09:00:00")]
    pub start: NaiveTime,
    #[schema(value_type=String,format="local-time",example="17:00:00")]
    pub end: NaiveTime,
}
#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AvailabilityRequest {
    pub windows: Vec<Window>,
}
#[derive(Serialize, ToSchema)]
pub struct AvailabilityResponse {
    pub member_id: Uuid,
    pub version: i32,
    pub windows: Vec<Window>,
    pub conflicts: Vec<ConflictResponse>,
}
#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SuggestRequest {
    pub member_id: Option<Uuid>,
}
#[derive(Serialize, ToSchema)]
pub struct SuggestionResponse {
    pub id: Uuid,
    pub member_id: Uuid,
    pub item_id: Uuid,
    pub occurrence_id: Uuid,
    pub proposed_start: DateTime<Utc>,
    pub proposed_end: DateTime<Utc>,
    #[schema(value_type=SuggestionStatus)]
    pub status: String,
    pub run_id: Uuid,
    pub version: i32,
    pub occurrence_version: Option<i32>,
    pub created_at: DateTime<Utc>,
    /// Acceptance-time warnings only; empty is not proof of no conflicts when unevaluated.
    pub conflicts: Vec<ConflictResponse>,
    /// True only for successful acceptance. Reads/snapshots do not evaluate proposal conflicts.
    /// Missing on older servers means false.
    #[schema(required = false, default = false)]
    pub conflicts_evaluated: bool,
}
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionStatus {
    Proposed,
    Accepted,
    Dismissed,
}
#[derive(Serialize, ToSchema)]
pub struct Unplaced {
    pub occurrence_id: Uuid,
    #[schema(value_type=UnplacedReason)]
    pub reason: String,
}
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnplacedReason {
    UnknownMember,
    InvalidEstimate,
    InvalidDeadline,
    NoSlotBeforeDeadline,
    DuplicateTask,
}
#[derive(Serialize, ToSchema)]
pub struct SuggestResponse {
    pub run_id: Uuid,
    pub suggestions: Vec<SuggestionResponse>,
    pub unplaced: Vec<Unplaced>,
}
#[derive(Deserialize, IntoParams)]
pub struct SuggestionsQuery {
    pub member_id: Option<Uuid>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}
#[derive(Serialize, ToSchema)]
pub struct SuggestionsPage {
    pub items: Vec<SuggestionResponse>,
    pub next_cursor: Option<String>,
}
#[derive(Serialize, ToSchema)]
pub struct Candidate {
    pub member_id: Uuid,
    #[schema(value_type=CandidateReason)]
    pub reason: porchlight_core::schedule::CandidateReason,
}
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CandidateReason {
    FreeInsideAvailability,
    FreeOutsideAvailability,
    Conflicting,
    InvalidAppointment,
}
#[derive(Serialize, ToSchema)]
pub struct CandidatesResponse {
    pub item_id: Uuid,
    pub occurrence_id: Option<Uuid>,
    pub candidates: Vec<Candidate>,
    pub pending_derivation: bool,
}
#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyRequest {}

#[utoipa::path(get,path="/v1/members/{id}/availability",params(("id"=Uuid,Path)),responses((status=200,description="Versioned windows; empty means load not tracked: no load conflicts, notifications or EDF suggestions; with any configured windows, omitted weekdays have capacity 0",body=AvailabilityResponse),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem))]
pub async fn availability(
    State(s): State<AppState>,
    c: Caller,
    Path(id): Path<Uuid>,
) -> Result<Json<AvailabilityResponse>, ApiError> {
    let mut tx = crate::items::read_snapshot(&s).await?;
    crate::planning::check_member(&mut tx, &c, id).await?;
    Ok(Json(crate::planning::availability(&mut tx, id).await?))
}
#[utoipa::path(put,path="/v1/members/{id}/availability",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=AvailabilityRequest,responses((status=200,description="Windows replaced and load warnings recomputed",body=AvailabilityResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,super::items::IfMatchProblem))]
pub fn put_availability<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    body: AvailabilityRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(crate::planning::mutate(
        s,
        c,
        tx,
        crate::planning::Operation::Availability,
        serde_json::to_value(body).unwrap(),
    ))
}
#[utoipa::path(post,path="/v1/schedule/suggest",request_body=SuggestRequest,responses((status=200,description="EDF over target member agenda (assignees union subject, else owner), independent of caller; self or super admin for dependent only; invalid member_id body references return 422 validation-failed with member_id / invalid-reference; retires considered-item stale proposals and deleted-item proposals, preserves excluded live items and accepted/dismissed history; starts at the next member-local 15-minute boundary; empty windows yield no suggestions; no work moves automatically",body=SuggestResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::openapi::ForbiddenProblem))]
pub fn suggest<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    body: SuggestRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(crate::planning::mutate(
        s,
        c,
        tx,
        crate::planning::Operation::Suggest,
        serde_json::to_value(body).unwrap(),
    ))
}
#[utoipa::path(get,path="/v1/suggestions",params(SuggestionsQuery),responses((status=200,description="ACL-filtered private suggestions",body=SuggestionsPage),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,super::items::ReadValidationProblem))]
pub async fn suggestions(
    State(s): State<AppState>,
    c: Caller,
    Query(q): Query<SuggestionsQuery>,
) -> Result<Json<SuggestionsPage>, ApiError> {
    Ok(Json(crate::planning::list(&s, &c, q).await?))
}
#[utoipa::path(post,path="/v1/suggestions/{id}/accept",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=EmptyRequest,responses((status=200,description="Accepted by item Edit caller OR suggestion member still on agenda; writes only occurrence span. Member must still be on agenda and proposed_start must be at least now minus 5 minutes (inclusive grace), else sealed 422 validation-failed with field code. If-Match is the OCCURRENCE version",body=SuggestionResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,super::items::IfMatchProblem))]
pub fn accept<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    body: EmptyRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(crate::planning::mutate(
        s,
        c,
        tx,
        crate::planning::Operation::Accept,
        serde_json::to_value(body).unwrap(),
    ))
}
#[utoipa::path(post,path="/v1/suggestions/{id}/dismiss",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=EmptyRequest,responses((status=200,description="Dismissed; If-Match is the suggestion version",body=SuggestionResponse),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,super::items::IfMatchProblem))]
pub fn dismiss<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    body: EmptyRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(crate::planning::mutate(
        s,
        c,
        tx,
        crate::planning::Operation::Dismiss,
        serde_json::to_value(body).unwrap(),
    ))
}
#[utoipa::path(get,path="/v1/items/{id}/candidates",params(("id"=Uuid,Path)),responses((status=200,description="Dependent assignees ranked by room for the next future occurrence; pending peers mark response pending, poisoned peers do not and poisoned parents are omitted; only free_inside_availability candidates notify; no assignment is made",body=CandidatesResponse),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,super::items::ReadValidationProblem))]
pub async fn candidates(
    State(s): State<AppState>,
    c: Caller,
    Path(id): Path<Uuid>,
) -> Result<Json<CandidatesResponse>, ApiError> {
    let mut tx = crate::items::read_snapshot(&s).await?;
    Ok(Json(
        crate::planning::candidates(&mut tx, &c, id, (s.item_clock)()).await?,
    ))
}
#[derive(OpenApi)]
#[openapi(
    paths(
        availability,
        put_availability,
        suggest,
        suggestions,
        accept,
        dismiss,
        candidates
    ),
    components(schemas(
        Window,
        AvailabilityRequest,
        AvailabilityResponse,
        SuggestRequest,
        SuggestionResponse,
        SuggestionStatus,
        SuggestResponse,
        Unplaced,
        UnplacedReason,
        SuggestionsPage,
        Candidate,
        CandidateReason,
        CandidatesResponse,
        EmptyRequest
    ))
)]
pub struct PlanningApi;
