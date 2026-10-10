use crate::server::{ConnInfo, Transport};
use axum::{Extension, Json};
use serde::Serialize;

#[derive(Serialize, utoipa::ToSchema)]
pub struct Connection {
    pub(crate) transport: Transport,
    pub(crate) client_addr: Option<String>,
}
#[utoipa::path(get,
    path="/v1/connection",
    responses((status=200,description="Success",body=crate::routes::connection::Connection),crate::openapi::UniversalProblems),
    security(("session" = [])))]
pub async fn get(
    _session: crate::extract::SessionAuth,
    Extension(info): Extension<ConnInfo>,
) -> Result<Json<Connection>, crate::error::ApiError> {
    Ok(Json(Connection {
        transport: info.transport,
        client_addr: info.client_addr.map(|a| a.to_string()),
    }))
}

#[derive(utoipa::OpenApi)]
#[openapi(paths(get))]
pub struct ConnectionApi;
