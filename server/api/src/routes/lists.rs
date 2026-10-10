//! Lists HTTP contracts. Mutations are registered only through S1's wrapper.
use crate::{error::ApiError, extract::Caller, idempotency::IdemTx, state::AppState};
use axum::{
    Json,
    extract::{Path, Query, State},
    response::Response,
};
use futures_util::future::BoxFuture;
use serde_json::Value;
use uuid::Uuid;

pub use crate::list_types::*;

#[utoipa::path(get,path="/v1/lists",params(ListsQuery),responses((status=200,description="Current resource",body=ListsPage),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::routes::items::ReadValidationProblem))]
pub async fn list(
    State(s): State<AppState>,
    c: Caller,
    Query(q): Query<ListsQuery>,
) -> Result<Json<ListsPage>, ApiError> {
    let mut tx = crate::items::read_snapshot(&s).await?;
    let page = crate::lists::collection(&s, &mut tx, &c, q).await?;
    tx.commit().await?;
    Ok(Json(page))
}
#[utoipa::path(get,path="/v1/lists/{id}",params(("id"=Uuid,Path),ListQuery),responses((status=200,description="Current resource",body=ListResponse),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::routes::items::ReadValidationProblem))]
pub async fn get(
    State(s): State<AppState>,
    c: Caller,
    Path(id): Path<Uuid>,
    Query(q): Query<ListQuery>,
) -> Result<Json<ListResponse>, ApiError> {
    let mut tx = crate::items::read_snapshot(&s).await?;
    let r = crate::lists::response(&mut tx, &c, id, q.occurrence_id, false).await?;
    tx.commit().await?;
    Ok(Json(r))
}

#[utoipa::path(post,path="/v1/lists",request_body=CreateList,responses((status=201,description="Created resource",body=ListResponse),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem))]
pub fn create<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    call(s, c, tx, b, crate::lists::Operation::Create)
}
#[utoipa::path(patch,path="/v1/lists/{id}",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=PatchList,responses((status=200,description="Current resource",body=ListResponse),(status=204,description="Writer lost access"),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::routes::items::IfMatchProblem))]
pub fn patch<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    call(s, c, tx, b, crate::lists::Operation::Patch)
}
#[utoipa::path(delete,path="/v1/lists/{id}",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),responses((status=200,description="Current resource",body=ListResponse),(status=204,description="Deleted parent: list removed by owner or super admin"),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::routes::items::IfMatchProblem))]
pub fn delete<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    call(s, c, tx, b, crate::lists::Operation::Delete)
}
#[utoipa::path(post,path="/v1/lists/{id}/entries",params(("id"=Uuid,Path)),request_body=EntryWrite,responses((status=201,description="Created resource",body=EntryResponse),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem))]
pub fn add_entry<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    call(s, c, tx, b, crate::lists::Operation::AddEntry)
}
#[utoipa::path(patch,path="/v1/list-entries/{id}",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=EntryWrite,responses((status=200,description="Current resource",body=EntryResponse),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::routes::items::IfMatchProblem))]
pub fn patch_entry<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    call(s, c, tx, b, crate::lists::Operation::PatchEntry)
}
#[utoipa::path(delete,path="/v1/list-entries/{id}",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),responses((status=200,description="Current resource",body=EntryResponse),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::routes::items::IfMatchProblem))]
pub fn delete_entry<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    call(s, c, tx, b, crate::lists::Operation::DeleteEntry)
}
#[utoipa::path(post,path="/v1/list-entries/{id}/check",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=CheckRequest,responses((status=200,description="Current resource",body=EntryResponse),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::routes::items::IfMatchProblem))]
pub fn check<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    call(s, c, tx, b, crate::lists::Operation::Check)
}
#[utoipa::path(post,path="/v1/list-entries/{id}/uncheck",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=CheckRequest,responses((status=200,description="Current resource",body=EntryResponse),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::routes::items::IfMatchProblem))]
pub fn uncheck<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    call(s, c, tx, b, crate::lists::Operation::Uncheck)
}
#[utoipa::path(post,path="/v1/lists/{id}/clear-checked",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=CheckRequest,responses((status=200,description="Current resource",body=ListResponse),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::routes::items::IfMatchProblem))]
pub fn clear<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    call(s, c, tx, b, crate::lists::Operation::Clear)
}
#[utoipa::path(put,path="/v1/lists/{id}/viewers",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=ViewersRequest,responses((status=200,description="Current resource",body=ListResponse),(status=204,description="Writer lost access"),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::openapi::MutationProblems,crate::openapi::RateLimitProblem,crate::routes::items::IfMatchProblem))]
pub fn viewers<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: Value,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    call(s, c, tx, b, crate::lists::Operation::Viewers)
}
fn call<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: Value,
    op: crate::lists::Operation,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move { crate::lists::mutate(c, tx, b, op, (s.item_clock)()).await })
}
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list,
        get,
        create,
        patch,
        delete,
        add_entry,
        patch_entry,
        delete_entry,
        check,
        uncheck,
        clear,
        viewers
    ),
    components(schemas(
        ListMode,
        ListVisibility,
        ListRow,
        EntryRow,
        EntryResponse,
        ListResponse,
        ListsPage,
        CreateList,
        PatchList,
        EntryWrite,
        CheckRequest,
        ViewersRequest
    ))
)]
pub struct ListsApi;
