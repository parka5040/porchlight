//! Administrative security history is available only on the agent socket, never the device stream.
use crate::{
    error::{ApiError, Problem},
    extract::AdminAuth,
    state::AppState,
};
use axum::{
    Json,
    extract::{Query, State},
};
use chrono::{DateTime, Utc};
use porchlight_db::security_events;
use uuid::Uuid;

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct Params {
    pub since: Option<DateTime<Utc>>,
    pub kind: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct SecurityEvent {
    id: Uuid,
    household_id: Uuid,
    occurred_at: DateTime<Utc>,
    kind: String,
    /// Transaction member/device/token identifiers, agent label and request_id. No credential material.
    actor: serde_json::Value,
    target_type: String,
    target_id: Uuid,
    outcome: String,
}
impl From<security_events::Event> for SecurityEvent {
    fn from(e: security_events::Event) -> Self {
        Self {
            id: e.id,
            household_id: e.household_id,
            occurred_at: e.occurred_at,
            kind: e.kind,
            actor: e.actor,
            target_type: e.target_type,
            target_id: e.target_id,
            outcome: e.outcome,
        }
    }
}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct Events {
    items: Vec<SecurityEvent>,
    next_cursor: Option<String>,
}
#[utoipa::path(get, path="/v1/admin/security-events", params(Params),
    responses((status=200,description="Secret-free security history, oldest first; one-year retention",body=Events),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem),
    security(("pat"=[])))]
pub async fn list(
    State(state): State<AppState>,
    AdminAuth(p): AdminAuth,
    query: Result<Query<Params>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<Events>, ApiError> {
    let Query(q) = query.map_err(|_| ApiError::new(Problem::InvalidRequest))?;
    if q.kind
        .as_ref()
        .is_some_and(|k| k.len() > 64 || !k.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
    {
        return Err(ApiError::new(Problem::InvalidRequest));
    }
    let limit = crate::pagination::PageParams {
        cursor: None,
        limit: q.limit,
    }
    .limit()?;
    let context = format!(
        "security-events:{}:{:?}:{:?}",
        p.0.household_id, q.since, q.kind
    );
    let after = q
        .cursor
        .as_deref()
        .map(|c| state.idem.cursor.decode(&context, c))
        .transpose()?;
    let rows = security_events::list(
        &state.pool,
        p.0.household_id,
        q.since,
        q.kind.as_deref(),
        after,
        (limit + 1) as i64,
    )
    .await?;
    let page = state
        .idem
        .cursor
        .page(&context, rows, limit, |e| (e.occurred_at, e.id))?;
    Ok(Json(Events {
        items: page.items.into_iter().map(Into::into).collect(),
        next_cursor: page.next_cursor,
    }))
}
fn policy(c: &crate::permissions::PolicyContext) -> u16 {
    match c.pat_scope {
        Some(crate::permissions::PatScope::Admin) => 200,
        Some(_) => 403,
        None => 404,
    }
}
pub const ENDPOINTS: &[crate::permissions::Endpoint] = &[crate::permissions::Endpoint {
    method: "GET",
    path: "/v1/admin/security-events",
    handler: "list",
    policy: crate::permissions::Policy::Custom {
        label: "Admin PAT security history (agent socket only)",
        expected: policy,
    },
    case: "",
}];
#[derive(utoipa::OpenApi)]
#[openapi(paths(list))]
pub struct SecurityEventsApi;
