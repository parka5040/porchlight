//! S8a routes registered only through the shared mutation registry.
use crate::{
    calendar::{self, ImportRequest, ImportSummary, Source, SourceRequest},
    error::{ApiError, Problem},
    extract::Caller,
    idempotency::IdemTx,
    pagination::PageParams,
    state::AppState,
};
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures_util::future::BoxFuture;
use sqlx::types::Json as SqlJson;
use uuid::Uuid;

#[utoipa::path(post,path="/v1/imports/ics",params(("member_id"=Option<Uuid>,Query,description="Required for text uploads; JSON uses the body target"),("name"=Option<String>,Query),("scope"=Option<String>,Query),("preview"=Option<bool>,Query),("visibility"=Option<calendar::SourceVisibility>,Query)),request_body(content((ImportRequest="application/json"),(String="text/calendar"),(String="text/plain")),description="JSON envelope or UTF-8 ICS text (5 MiB limit). Text uploads use query target/name/scope/preview. The 4000-unit actual-work admission limit returns urn:porchlight:problem:calendar-too-large (422). Apply deadlines return retryable, unsealed 409 conflict."),responses((status=200,description="Success",body=ImportSummary),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::openapi::RateLimitProblem))]
pub fn import<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    body: ImportRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        Ok(
            Json(crate::items::ics_import::import(c, tx, body, (s.item_clock)()).await?)
                .into_response(),
        )
    })
}
#[derive(serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyRequest {}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct SourcePage {
    pub items: Vec<Source>,
    pub next_cursor: Option<String>,
}
#[utoipa::path(get,path="/v1/feeds",params(("cursor"=Option<String>,Query),("limit"=Option<u16>,Query)),responses((status=200,description="Success",body=SourcePage),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem))]
pub async fn list(
    State(s): State<AppState>,
    c: Caller,
    Query(q): Query<PageParams>,
) -> Result<Json<SourcePage>, ApiError> {
    c.require_admin()?;
    let limit = q.limit()?;
    let context = format!("ics_sources:{}:{}", c.household_id(), c.member_id());
    let cursor = q
        .cursor
        .as_ref()
        .map(|v| s.idem.cursor.decode::<Uuid>(&context, v))
        .transpose()?;
    let rows=sqlx::query_scalar::<_,SqlJson<Source>>("select to_jsonb(s) from porchlight.ics_sources s where household_id=$1 and kind='feed' and deleted_at is null and ($2::uuid is null or id>$2) order by id limit $3").bind(c.household_id()).bind(cursor).bind((limit+1)as i64).fetch_all(&s.pool).await?.into_iter().map(|s|s.0).collect();
    let page = s
        .idem
        .cursor
        .page(&context, rows, limit, |s: &Source| s.id)?;
    Ok(Json(SourcePage {
        items: page.items,
        next_cursor: page.next_cursor,
    }))
}
#[utoipa::path(get,path="/v1/admin/feeds",params(("cursor"=Option<String>,Query),("limit"=Option<u16>,Query)),responses((status=200,description="Success",body=SourcePage),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem))]
pub async fn admin_list(
    s: State<AppState>,
    c: Caller,
    q: Query<PageParams>,
) -> Result<Json<SourcePage>, ApiError> {
    list(s, c, q).await
}
#[utoipa::path(post,path="/v1/feeds",request_body=SourceRequest,responses((status=201,description="Subscription created",body=Source),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem))]
pub fn add<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: SourceRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        c.require_admin()?;
        let mut b = b;
        calendar::validate_source(&b)?;
        calendar::target_zone(&mut tx.tx, c.household_id(), b.target_member_id).await?;
        b.visibility = Some(
            calendar::source_visibility(
                &mut tx.tx,
                c.household_id(),
                b.target_member_id,
                b.visibility.as_deref(),
            )
            .await?,
        );
        let id:Uuid=sqlx::query_scalar("insert into porchlight.ics_sources(household_id,owner_member_id,target_member_id,kind,import_key,name,url,allow_http,refresh_seconds,enabled,next_fetch_at,visibility) values($1,$2,$3,'feed',$4,$5,$6,$7,$8,$9,$10,$11) returning id")
        .bind(c.household_id()).bind(c.member_id()).bind(b.target_member_id).bind(Uuid::now_v7().to_string()).bind(&b.name).bind(&b.url).bind(b.allow_http).bind(b.refresh_seconds).bind(b.enabled).bind((s.item_clock)()).bind(b.visibility()).fetch_one(&mut *tx.tx).await?;
        Ok((
            StatusCode::CREATED,
            Json(calendar::load_source(&mut tx.tx, c.household_id(), id, false).await?),
        )
            .into_response())
    })
}
#[utoipa::path(post,path="/v1/admin/feeds",request_body=SourceRequest,responses((status=201,description="Subscription created",body=Source),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem))]
pub fn admin_add<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: SourceRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    add(s, c, tx, b)
}
fn target(tx: &IdemTx) -> Result<Uuid, ApiError> {
    tx.context
        .target_id
        .ok_or_else(|| ApiError::new(Problem::InvalidRequest))
}
async fn checked(c: &Caller, tx: &mut IdemTx) -> Result<Source, ApiError> {
    c.require_admin()?;
    let expected = crate::protocol::if_match(&tx.context.if_match)?;
    let id = target(tx)?;
    let source = calendar::load_source(&mut tx.tx, c.household_id(), id, true).await?;
    if source.kind != "feed" {
        return Err(ApiError::new(Problem::NotFound));
    }
    if source.version != expected {
        return Err(ApiError::new(Problem::VersionConflict).extension(
            "current",
            serde_json::to_value(source).map_err(|_| ApiError::internal("source_encode"))?,
        ));
    }
    Ok(source)
}
#[utoipa::path(put,path="/v1/feeds/{id}",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=SourceRequest,responses((status=200,description="Success",body=Source),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::routes::items::IfMatchProblem,crate::openapi::RateLimitProblem))]
pub fn update<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: SourceRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        let old = checked(&c, tx).await?;
        let mut b = b;
        calendar::validate_source(&b)?;
        calendar::target_zone(&mut tx.tx, c.household_id(), b.target_member_id).await?;
        b.visibility = Some(
            calendar::source_visibility(
                &mut tx.tx,
                c.household_id(),
                b.target_member_id,
                b.visibility.as_deref(),
            )
            .await?,
        );
        crate::items::ics_import::configure_feed(&mut tx.tx, &old, &b, (s.item_clock)()).await?;
        sqlx::query("update porchlight.ics_sources set name=$2,url=$3,target_member_id=$4,allow_http=$5,refresh_seconds=$6,enabled=$7,next_fetch_at=$8,visibility=$9,etag=null,last_modified=null,failure_count=0,last_warnings='[]',last_status='queued',last_error=null,apply_timeout_count=0 where id=$1").bind(old.id).bind(&b.name).bind(&b.url).bind(b.target_member_id).bind(b.allow_http).bind(b.refresh_seconds).bind(b.enabled).bind((s.item_clock)()).bind(b.visibility()).execute(&mut *tx.tx).await?;
        Ok(
            Json(calendar::load_source(&mut tx.tx, c.household_id(), old.id, false).await?)
                .into_response(),
        )
    })
}
#[utoipa::path(put,path="/v1/admin/feeds/{id}",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=SourceRequest,responses((status=200,description="Success",body=Source),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::routes::items::IfMatchProblem,crate::openapi::RateLimitProblem))]
pub fn admin_update<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: SourceRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    update(s, c, tx, b)
}
#[utoipa::path(delete,path="/v1/feeds/{id}",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=EmptyRequest,responses((status=200,description="Success",body=Source),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::routes::items::IfMatchProblem,crate::openapi::RateLimitProblem))]
pub fn remove<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    _: EmptyRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        let old = checked(&c, tx).await?;
        crate::items::ics_import::remove_feed(&mut tx.tx, &old, (s.item_clock)()).await?;
        // R50: soft-delete all source items and retire mappings in the same transaction.
        sqlx::query("update porchlight.ics_sources set deleted_at=$2,enabled=false where id=$1")
            .bind(old.id)
            .bind((s.item_clock)())
            .execute(&mut *tx.tx)
            .await?;
        let SqlJson(source) = sqlx::query_scalar::<_, SqlJson<Source>>(
            "select to_jsonb(s) from porchlight.ics_sources s where id=$1",
        )
        .bind(old.id)
        .fetch_one(&mut *tx.tx)
        .await?;
        Ok(Json(source).into_response())
    })
}
#[utoipa::path(delete,path="/v1/admin/feeds/{id}",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=EmptyRequest,responses((status=200,description="Success",body=Source),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::routes::items::IfMatchProblem,crate::openapi::RateLimitProblem))]
pub fn admin_remove<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: EmptyRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    remove(s, c, tx, b)
}
#[utoipa::path(post,path="/v1/feeds/{id}/refresh",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=EmptyRequest,responses((status=202,description="Refresh queued",body=Source),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::routes::items::IfMatchProblem,crate::openapi::RateLimitProblem))]
pub fn refresh<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    _: EmptyRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        let old = checked(&c, tx).await?;
        if !old.enabled {
            return Err(ApiError::new(Problem::ValidationFailed));
        }
        sqlx::query(
            "update porchlight.ics_sources set next_fetch_at=$2,last_status='queued',apply_timeout_count=0 where id=$1",
        )
        .bind(old.id)
        .bind((s.item_clock)())
        .execute(&mut *tx.tx)
        .await?;
        Ok((
            StatusCode::ACCEPTED,
            Json(calendar::load_source(&mut tx.tx, c.household_id(), old.id, false).await?),
        )
            .into_response())
    })
}
#[utoipa::path(post,path="/v1/admin/feeds/{id}/refresh",params(("id"=Uuid,Path),crate::openapi::IfMatchHeader),request_body=EmptyRequest,responses((status=202,description="Refresh queued",body=Source),crate::openapi::UniversalProblems,crate::openapi::MutationProblems,crate::openapi::ForbiddenProblem,crate::openapi::NotFoundProblem,crate::routes::items::IfMatchProblem,crate::openapi::RateLimitProblem))]
pub fn admin_refresh<'a>(
    s: &'a AppState,
    c: Caller,
    tx: &'a mut IdemTx,
    b: EmptyRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    refresh(s, c, tx, b)
}
#[derive(serde::Deserialize, utoipa::IntoParams)]
pub struct TargetQuery {
    pub username: String,
}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct TargetResponse {
    pub member_id: Uuid,
}
/// Household-scoped calendar name resolver: username/display name, ambiguity is a field error.
#[utoipa::path(get,path="/v1/admin/feeds/target",params(TargetQuery),responses((status=200,description="Resolved calendar target",body=TargetResponse),crate::openapi::UniversalProblems,crate::openapi::ForbiddenProblem))]
pub async fn resolve_target(
    State(s): State<AppState>,
    c: Caller,
    Query(q): Query<TargetQuery>,
) -> Result<Json<TargetResponse>, ApiError> {
    // Own-name resolution also works for write PATs; resolving another target
    // remains admin-only. Collect all candidates before picking (never first-match).
    let mut rows: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
        "select m.id,m.display_name,u.username from porchlight.household_members m left join porchlight.users u on u.id=m.user_id where m.household_id=$1 and m.deleted_at is null and (lower(m.display_name)=lower($2) or lower(u.username)=lower($2)) order by m.id"
    ).bind(c.household_id()).bind(&q.username).fetch_all(&s.pool).await?;
    // A literal username is a stable identity and wins over display-name aliases.
    // Keep the original admin/self authorization for every selected result.
    if let Some(exact) = rows
        .iter()
        .find(|(_, _, user)| user.as_deref() == Some(q.username.as_str()))
        .cloned()
    {
        rows = vec![exact];
    }
    if rows.len() == 1 {
        let member_id = rows[0].0;
        if member_id != c.member_id() {
            c.require_admin()?;
        }
        return Ok(Json(TargetResponse { member_id }));
    }
    c.require_admin()?;
    if rows.is_empty() {
        return Err(calendar::member_reference("username"));
    }
    Err(ApiError::validation(vec![crate::error::FieldError {
        field: "username".into(),
        message: format!(
            "Ambiguous calendar target; candidates: {}",
            rows.iter()
                .map(|(id, name, user)| format!(
                    "{name} ({id}{})",
                    user.as_ref().map(|u| format!(", {u}")).unwrap_or_default()
                ))
                .collect::<Vec<_>>()
                .join("; ")
        ),
        code: Some("ambiguous-reference".into()),
    }]))
}
#[derive(utoipa::OpenApi)]
#[openapi(paths(
    import,
    list,
    admin_list,
    add,
    admin_add,
    update,
    admin_update,
    remove,
    admin_remove,
    refresh,
    admin_refresh,
    resolve_target
))]
pub struct CalendarApi;
