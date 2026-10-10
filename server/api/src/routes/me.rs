//! The current live identity and authenticated connection.
use crate::{error::ApiError, extract::SessionAuth, server::ConnInfo, state::AppState};
use axum::{Extension, Json, extract::State};
use chrono::{DateTime, Utc};
use uuid::Uuid;
#[utoipa::path(get,
    path="/v1/me",
    responses((status=200,description="Success",body=MeResponse),crate::openapi::UniversalProblems),
    security(("session" = [])))]
pub async fn get(
    State(state): State<AppState>,
    s: SessionAuth,
    Extension(info): Extension<ConnInfo>,
) -> Result<Json<MeResponse>, ApiError> {
    let d = s.device;
    let display_name = porchlight_db::auth::display_name(&state.pool, d.member_id).await?;
    Ok(Json(MeResponse {
        member_id: d.member_id,
        user_id: d.user_id,
        username: d.username,
        display_name,
        role: d.role,
        household_id: d.household_id,
        device: MeDevice {
            id: d.device_id,
            label: d.label,
            platform: d.platform,
        },
        session_expires_at: s.session_expires_at,
        connection: super::connection::Connection {
            transport: info.transport,
            client_addr: info.client_addr.map(|a| a.to_string()),
        },
    }))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(get))]
pub struct MeApi;

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct MeDevice {
    pub id: Uuid,
    pub label: String,
    pub platform: String,
}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct MeResponse {
    pub member_id: Uuid,
    pub user_id: Uuid,
    pub username: String,
    pub display_name: String,
    pub role: String,
    pub household_id: Uuid,
    pub device: MeDevice,
    pub session_expires_at: DateTime<Utc>,
    pub connection: crate::routes::connection::Connection,
}
