//! Private inbox and canonical notification settings. Monotonic read actions need no If-Match.
use crate::{
    error::{ApiError, Problem},
    extract::{ApiQuery, Caller},
    idempotency::IdemTx,
    notifier::{self, NotificationPrefs, NotificationSummary},
    pagination::{Page, PageParams},
    state::AppState,
};
use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, NaiveTime, Timelike, Utc};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct InboxParams {
    pub unread: Option<bool>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct AdminInboxParams {
    pub member: Uuid,
    pub unread: Option<bool>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}
#[derive(Serialize, utoipa::ToSchema)]
pub struct Notification {
    pub id: Uuid,
    pub kind: String,
    pub category: String,
    pub item_id: Option<Uuid>,
    pub occurrence_id: Option<Uuid>,
    pub entity_type: String,
    pub entity_id: Uuid,
    pub actor_member_id: Option<Uuid>,
    pub actor_label: Option<String>,
    pub count: i32,
    pub summary: NotificationSummary,
    pub created_at: DateTime<Utc>,
    pub release_at: DateTime<Utc>,
    pub released_at: Option<DateTime<Utc>>,
    pub read_at: Option<DateTime<Utc>>,
    pub version: i32,
}
#[derive(Serialize, utoipa::ToSchema)]
pub struct NotificationPage {
    pub items: Vec<Notification>,
    pub next_cursor: Option<String>,
}
#[derive(Serialize, utoipa::ToSchema, sqlx::FromRow)]
pub struct PrefsRead {
    pub member_id: Uuid,
    #[sqlx(json)]
    pub prefs: NotificationPrefs,
    pub version: i32,
}
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadRequest {}
#[derive(Serialize, utoipa::ToSchema)]
pub struct ReadAllResult {
    pub count: u64,
}
#[derive(sqlx::FromRow)]
pub(super) struct Row {
    pub(super) id: Uuid,
    category: String,
    created_at: DateTime<Utc>,
    release_at: DateTime<Utc>,
    released_at: Option<DateTime<Utc>>,
    read_at: Option<DateTime<Utc>>,
    version: i32,
}
pub(super) const ROW: &str = "select id,category,created_at,release_at,released_at,read_at,version from porchlight.notifications where household_id=$1 and recipient_member_id=$2 and released_at is not null";
pub(super) async fn detail(
    c: &mut PgConnection,
    member: Uuid,
    h: Uuid,
    row: Row,
) -> Result<Option<Notification>, ApiError> {
    Ok(details(c, member, h, vec![row]).await?.pop())
}
async fn details(
    c: &mut PgConnection,
    member: Uuid,
    h: Uuid,
    rows: Vec<Row>,
) -> Result<Vec<Notification>, ApiError> {
    let mut values = Vec::with_capacity(rows.len());
    for batch in rows.chunks(notifier::SOURCE_DETAIL_BATCH) {
        let ids: Vec<_> = batch.iter().map(|r| r.id).collect();
        let mut summaries: std::collections::HashMap<_, _> =
            notifier::visible_source_summaries(c, &ids, member, h)
                .await?
                .into_iter()
                .map(|s| (s.notification_id, s))
                .collect();
        for row in batch {
            if let Some(sources) = summaries.remove(&row.id) {
                values.push(render_detail(row, sources));
            }
        }
    }
    Ok(values)
}
fn render_detail(row: &Row, sources: notifier::SourceSummary) -> Notification {
    let first = &sources.first;
    let category = if row.category == "digest" {
        "digest"
    } else if sources.same_category {
        notifier::category(&first.kind)
    } else {
        "activity"
    }
    .to_string();
    Notification {
        id: row.id,
        kind: if sources.count == 1 {
            first.kind.clone()
        } else {
            "summary".into()
        },
        category,
        item_id: if sources.count == 1 {
            first.item_id
        } else {
            None
        },
        occurrence_id: if sources.count == 1 {
            first.occurrence_id
        } else {
            None
        },
        entity_type: first.entity_type.clone(),
        entity_id: first.entity_id,
        actor_member_id: if sources.same_actor {
            first.actor_member_id
        } else {
            None
        },
        actor_label: if sources.same_actor {
            first.actor_label.clone()
        } else {
            None
        },
        count: sources.count,
        summary: sources.summary(row.category == "digest"),
        created_at: row.created_at,
        release_at: row.release_at,
        released_at: row.released_at,
        read_at: row.read_at,
        version: row.version,
    }
}
async fn inbox(
    state: &AppState,
    caller: &Caller,
    member: Uuid,
    p: InboxParams,
) -> Result<Json<Page<Notification>>, ApiError> {
    let limit = PageParams {
        cursor: None,
        limit: p.limit,
    }
    .limit()?;
    let unread = p.unread.unwrap_or(false);
    let context = format!(
        "notifications:{}:{}:{}:{unread}",
        caller.household_id(),
        caller.member_id(),
        member
    );
    let after: Option<Uuid> = p
        .cursor
        .as_deref()
        .map(|s| state.idem.cursor.decode(&context, s))
        .transpose()?;
    let mut tx = porchlight_db::tx::begin(&state.pool).await?;
    sqlx::query("set transaction isolation level repeatable read read only")
        .execute(&mut *tx)
        .await?;
    let live:bool=sqlx::query_scalar("select exists(select 1 from porchlight.household_members where household_id=$1 and id=$2 and deleted_at is null)").bind(caller.household_id()).bind(member).fetch_one(&mut *tx).await?;
    if !live {
        return Err(ApiError::new(Problem::NotFound));
    }
    // Filter BEFORE pagination: inaccessible coalesced sources cannot create empty/short pages.
    let sql = format!(
        "{ROW} and (not $3 or read_at is null) and ($4::uuid is null or id<$4) and exists(select 1 from porchlight.notification_sources s join porchlight.notification_events e on e.id=s.event_id where s.notification_id=notifications.id and (e.item_id is null or porchlight.pl_item_can_read(e.item_id,$2))) order by id desc limit $5"
    );
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(caller.household_id())
        .bind(member)
        .bind(unread)
        .bind(after)
        .bind((limit + 1) as i64)
        .fetch_all(&mut *tx)
        .await?;
    let values = details(&mut tx, member, caller.household_id(), rows).await?;
    let page = state.idem.cursor.page(&context, values, limit, |n| n.id)?;
    tx.commit().await?;
    Ok(Json(page))
}
#[utoipa::path(get,path="/v1/notifications",params(InboxParams),responses((status=200,description="Private released inbox; each coalesced source rechecks recipient item ACL",body=NotificationPage),crate::openapi::UniversalProblems))]
pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    ApiQuery(p): ApiQuery<InboxParams>,
) -> Result<Json<Page<Notification>>, ApiError> {
    inbox(&state, &caller, caller.member_id(), p).await
}
#[utoipa::path(get,path="/v1/admin/notifications",params(AdminInboxParams),responses((status=200,description="Admin PAT inspection of the target recipient's authorized inbox",body=NotificationPage),crate::openapi::UniversalProblems))]
pub async fn admin_list(
    State(state): State<AppState>,
    caller: Caller,
    ApiQuery(p): ApiQuery<AdminInboxParams>,
) -> Result<Json<Page<Notification>>, ApiError> {
    caller.require_admin()?;
    inbox(
        &state,
        &caller,
        p.member,
        InboxParams {
            unread: p.unread,
            cursor: p.cursor,
            limit: p.limit,
        },
    )
    .await
}
async fn one(
    c: &mut PgConnection,
    caller: &Caller,
    id: Uuid,
    lock: bool,
) -> Result<Notification, ApiError> {
    let sql = format!("{ROW} and id=$3{}", if lock { " for update" } else { "" });
    let row: Row = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(caller.household_id())
        .bind(caller.member_id())
        .bind(id)
        .fetch_optional(&mut *c)
        .await?
        .ok_or_else(|| ApiError::new(Problem::NotFound))?;
    detail(c, caller.member_id(), caller.household_id(), row)
        .await?
        .ok_or_else(|| ApiError::new(Problem::NotFound))
}
#[utoipa::path(post,path="/v1/notifications/{id}/read",params(("id"=Uuid,Path)),request_body=ReadRequest,responses((status=200,description="Monotonic read; repeated action is a no-op, dismissed tickle reaches this member's other devices",body=Notification),crate::openapi::UniversalProblems,crate::openapi::MutationProblems))]
pub fn read<'a>(
    state: &'a AppState,
    caller: Caller,
    tx: &'a mut IdemTx,
    _body: ReadRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        let id = tx
            .context
            .target_id
            .ok_or_else(|| ApiError::new(Problem::InvalidRequest))?;
        let n = one(&mut tx.tx, &caller, id, true).await?;
        if n.read_at.is_none() {
            sqlx::query(
                "update porchlight.notifications set read_at=$2 where id=$1 and read_at is null",
            )
            .bind(id)
            .bind((state.item_clock)())
            .execute(&mut *tx.tx)
            .await?;
        }
        Ok(Json(one(&mut tx.tx, &caller, id, false).await?).into_response())
    })
}
#[utoipa::path(post,path="/v1/notifications/read-all",request_body=ReadRequest,responses((status=200,description="Mark all currently visible released notifications read",body=ReadAllResult),crate::openapi::UniversalProblems,crate::openapi::MutationProblems))]
pub fn read_all<'a>(
    state: &'a AppState,
    caller: Caller,
    tx: &'a mut IdemTx,
    _body: ReadRequest,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        let count=sqlx::query("with locked as materialized (select n.id from porchlight.notifications n where household_id=$1 and recipient_member_id=$2 and released_at is not null and read_at is null and exists(select 1 from porchlight.notification_sources s join porchlight.notification_events e on e.id=s.event_id where s.notification_id=n.id and (e.item_id is null or porchlight.pl_item_can_read(e.item_id,$2))) order by n.id for update) update porchlight.notifications n set read_at=$3 from locked where n.id=locked.id and n.read_at is null")
        .bind(caller.household_id()).bind(caller.member_id()).bind((state.item_clock)()).execute(&mut *tx.tx).await?.rows_affected();
        Ok(Json(ReadAllResult { count }).into_response())
    })
}
pub(super) async fn prefs_row(
    c: &mut PgConnection,
    caller: &Caller,
    lock: bool,
) -> Result<PrefsRead, ApiError> {
    if lock {
        sqlx::query("select id from porchlight.household_members where id=$1 for key share")
            .bind(caller.member_id())
            .fetch_one(&mut *c)
            .await?;
    }
    let sql = format!(
        "select member_id,{} as prefs,version from porchlight.notification_prefs p where household_id=$1 and member_id=$2{}",
        notifier::PREFS_JSON,
        if lock { " for update" } else { "" }
    );
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(caller.household_id())
        .bind(caller.member_id())
        .fetch_optional(c)
        .await?
        .ok_or_else(|| ApiError::new(Problem::NotFound))
}
#[utoipa::path(get,path="/v1/me/notification-prefs",responses((status=200,description="Canonical self notification preferences and version",body=PrefsRead),crate::openapi::UniversalProblems))]
pub async fn prefs_get(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<PrefsRead>, ApiError> {
    let mut c = state.pool.acquire().await?;
    Ok(Json(prefs_row(&mut c, &caller, false).await?))
}
fn validate(mut p: NotificationPrefs) -> Result<NotificationPrefs, ApiError> {
    let invalid = || ApiError::new(Problem::ValidationFailed);
    if p.quiet_start.is_some() != p.quiet_end.is_some() {
        return Err(invalid());
    }
    for value in [&mut p.quiet_start, &mut p.quiet_end].into_iter().flatten() {
        let parsed: NaiveTime = value.parse().map_err(|_| invalid())?;
        if parsed.nanosecond() != 0 {
            return Err(invalid());
        }
        *value = parsed.to_string();
    }
    if p.quiet_start.is_some() && p.quiet_start == p.quiet_end {
        return Err(invalid());
    }
    for offsets in [
        &mut p.default_todo_offsets,
        &mut p.default_deadline_offsets,
        &mut p.default_appointment_offsets,
    ] {
        if offsets.len() > 16 || offsets.iter().any(|n| !(0..=525600).contains(n)) {
            return Err(invalid());
        }
        offsets.sort_unstable();
        if offsets.windows(2).any(|w| w[0] == w[1]) {
            return Err(invalid());
        }
    }
    Ok(p)
}
#[utoipa::path(put,path="/v1/me/notification-prefs",params(crate::openapi::IfMatchHeader),request_body=NotificationPrefs,responses((status=200,description="Replace canonical preferences; unchanged values write nothing",body=PrefsRead),(status=428,description="If-Match required",body=crate::openapi::ProblemBody,content_type="application/problem+json"),crate::openapi::UniversalProblems,crate::openapi::MutationProblems))]
pub fn prefs_put<'a>(
    state: &'a AppState,
    caller: Caller,
    tx: &'a mut IdemTx,
    body: NotificationPrefs,
) -> BoxFuture<'a, Result<Response, ApiError>> {
    Box::pin(async move {
        let expected = crate::protocol::if_match(&tx.context.if_match)?;
        let p = validate(body)?;
        let current = prefs_row(&mut tx.tx, &caller, false).await?;
        // Acquire all item/job locks before prefs, never lock an item after taking prefs.
        let offsets_changed = (
            current.prefs.default_todo_offsets.clone(),
            current.prefs.default_deadline_offsets.clone(),
            current.prefs.default_appointment_offsets.clone(),
        ) != (
            p.default_todo_offsets.clone(),
            p.default_deadline_offsets.clone(),
            p.default_appointment_offsets.clone(),
        );
        if offsets_changed {
            let scope = porchlight_db::items::WriteScope::begin_at(
                &mut tx.tx,
                caller.household_id(),
                porchlight_db::items::ScopeRequest {
                    member_zones: vec![caller.member_id()],
                    enqueue_only: true,
                    ..Default::default()
                },
                (state.item_clock)(),
            )
            .await
            .map_err(crate::items::domain_error)?;
            scope
                .finish((state.item_clock)())
                .await
                .map_err(crate::items::domain_error)?;
        }
        let current = prefs_row(&mut tx.tx, &caller, true).await?;
        if current.version != expected {
            return Err(ApiError::new(Problem::VersionConflict).extension(
                "current",
                serde_json::to_value(current).map_err(|_| ApiError::internal("prefs"))?,
            ));
        }
        if current.prefs == p {
            return Ok(Json(current).into_response());
        }
        sqlx::query("update porchlight.notification_prefs set reminders=$3,assignments=$4,dependents=$5,activity=$6,digest=$7,connection=$8,quiet_start=$9,quiet_end=$10,default_todo_offsets=$11,default_deadline_offsets=$12,default_appointment_offsets=$13 where household_id=$1 and member_id=$2")
        .bind(caller.household_id()).bind(caller.member_id()).bind(p.reminders).bind(p.assignments).bind(p.dependents).bind(p.activity).bind(p.digest).bind(p.connection).bind(p.quiet_start.as_deref().map(|s|s.parse::<NaiveTime>().unwrap())).bind(p.quiet_end.as_deref().map(|s|s.parse::<NaiveTime>().unwrap())).bind(p.default_todo_offsets).bind(p.default_deadline_offsets).bind(p.default_appointment_offsets).execute(&mut *tx.tx).await?;
        Ok(Json(prefs_row(&mut tx.tx, &caller, false).await?).into_response())
    })
}
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(list, admin_list, read, read_all, prefs_get, prefs_put),
    components(schemas(
        Notification,
        NotificationPage,
        NotificationSummary,
        NotificationPrefs,
        PrefsRead,
        ReadRequest,
        ReadAllResult
    ))
)]
pub struct NotificationsApi;

/// Snapshot extension: caller-owned notifications + prefs assembled in its existing RR snapshot.
pub async fn snapshot_fields(
    c: &mut PgConnection,
    caller: &Caller,
) -> Result<(Vec<Notification>, PrefsRead), ApiError> {
    let sql = format!("{ROW} order by id desc");
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(caller.household_id())
        .bind(caller.member_id())
        .fetch_all(&mut *c)
        .await?;
    let values = details(c, caller.member_id(), caller.household_id(), rows).await?;
    Ok((values, prefs_row(c, caller, false).await?))
}
