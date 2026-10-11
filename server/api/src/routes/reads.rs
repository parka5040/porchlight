//! Authoritative ACL read models. Collection cursors are context-bound to caller and filter.
use super::read_models::{ItemReadPage, SearchReadPage, SnapshotRead};
use crate::extract::ApiQuery as Query;
use crate::{
    error::{ApiError, Problem},
    extract::Caller,
    pagination::{Page, PageParams},
    state::AppState,
};
use axum::{Json, extract::State};
use chrono::{DateTime, Duration, Months, Utc};
use porchlight_core::access::{self, AccessContext, CallerKind, Role, Visibility};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{PgConnection, types::Json as SqlJson};
use uuid::Uuid;

/// S6/S7 may reuse this predicate with aliases i and parameters $1 household,
/// $2 member, $3 authenticated agent bypass. No derived row grants visibility.
pub const VISIBLE: &str =
    "i.household_id=$1 AND i.deleted_at IS NULL AND ($3 OR porchlight.pl_item_can_read(i.id,$2))";
const ITEM: &str = "(to_jsonb(i)-'search_document'-'reminder_origin_id') || jsonb_build_object(
    'completion_interval',porchlight.pl_interval_json(i.completion_interval),
    'pending_derivation',coalesce(d.applied_gen<d.requested_gen,false),
    'assignees',coalesce((select jsonb_agg(a.member_id
        order by a.member_id)
        from porchlight.item_assignees a where a.item_id=i.id),'[]'::jsonb),
    'viewers',coalesce((select jsonb_agg(v.member_id
        order by v.member_id)
        from porchlight.item_viewers v where v.item_id=i.id),'[]'::jsonb),
    'reminders',coalesce((select jsonb_agg(to_jsonb(r)-'delivery_id'
        order by r.offset_minutes,r.id)
        from porchlight.item_reminders r where r.item_id=i.id),'[]'::jsonb),
    'subject_role',(select role
        from porchlight.household_members where id=i.subject_member_id))";
const JOIN: &str = "from porchlight.items i left
        join porchlight.item_derivation d on d.item_id=i.id";
fn uid(value: &Value) -> Option<Uuid> {
    value.as_str().and_then(|s| s.parse().ok())
}
pub fn decorate(mut item: Value, caller: &Caller) -> Result<Value, ApiError> {
    let member = caller.member_id();
    let has = |key: &str| {
        item[key]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| uid(v) == Some(member)))
    };
    let context = AccessContext {
        caller: match caller {
            Caller::Agent { .. } => CallerKind::Agent,
            Caller::Member { role, .. } => CallerKind::AppMember(
                role.parse::<Role>()
                    .map_err(|_| ApiError::internal("role"))?,
            ),
        },
        visibility: item["visibility"]
            .as_str()
            .unwrap_or("")
            .parse::<Visibility>()
            .map_err(|_| ApiError::internal("visibility"))?,
        is_owner: uid(&item["owner_id"]) == Some(member),
        is_assignee: has("assignees"),
        is_viewer: has("viewers"),
        is_subject: uid(&item["subject_member_id"]) == Some(member),
        unassigned_dependent_item: item["subject_role"] == "dependent"
            && item["assignees"].as_array().is_some_and(|a| a.is_empty()),
    };
    item.as_object_mut().unwrap().remove("subject_role");
    let capabilities = crate::items::restrict_feed(
        !item["ics_source_id"].is_null(),
        caller,
        access::capabilities(&context).iter().collect(),
    );
    item["capabilities"] =
        serde_json::to_value(capabilities).map_err(|_| ApiError::internal("capabilities"))?;
    Ok(item)
}
async fn rows(conn: &mut PgConnection, caller: &Caller, sql: &str) -> Result<Vec<Value>, ApiError> {
    let rows: Vec<SqlJson<Value>> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(caller.household_id())
        .bind(caller.member_id())
        .bind(caller.sees_all())
        .fetch_all(conn)
        .await?;
    rows.into_iter().map(|v| decorate(v.0, caller)).collect()
}
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct BoardParams {
    pub scope: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}
#[utoipa::path(get,path="/v1/board",params(BoardParams),responses((status=200,description="Next open card per series and recent Done, status/bytewise position/id order",body=ItemReadPage),crate::openapi::UniversalProblems))]
pub async fn board(
    State(state): State<AppState>,
    caller: Caller,
    Query(p): Query<BoardParams>,
) -> Result<Json<Page<Value>>, ApiError> {
    let now = (state.item_clock)();
    let scope = p.scope.as_deref().unwrap_or("mine");
    if !matches!(scope, "mine" | "household") {
        return Err(ApiError::new(Problem::InvalidRequest));
    }
    let limit = PageParams {
        cursor: None,
        limit: p.limit,
    }
    .limit()?;
    let context = format!(
        "board:{}:{}:{}:{scope}",
        caller.household_id(),
        caller.member_id(),
        caller.sees_all()
    );
    let key: Option<(String, String, Uuid)> = p
        .cursor
        .as_deref()
        .map(|c| state.idem.cursor.decode(&context, c))
        .transpose()?;
    let mut conn = state.pool.acquire().await?;
    let values = board_rows(&mut conn, &caller, scope, now, key, limit).await?;
    Ok(Json(state.idem.cursor.page(
        &context,
        values,
        limit,
        |v| {
            (
                v["occurrence"]["status"].as_str().unwrap().to_owned(),
                v["occurrence"]["board_position"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                uid(&v["occurrence"]["id"]).unwrap(),
            )
        },
    )?))
}
/// Injected clock used by R30 projection tests; same domain as S3's board writer.
pub async fn board_rows(
    conn: &mut PgConnection,
    caller: &Caller,
    scope: &str,
    now: DateTime<Utc>,
    after: Option<(String, String, Uuid)>,
    limit: usize,
) -> Result<Vec<Value>, ApiError> {
    let sql = format!(
        "with cards as (select *
        from porchlight.pl_board_cards($1,$4)) select {ITEM} || jsonb_build_object('occurrence',to_jsonb(o)-'during')
        {JOIN}
        join cards o on o.item_id=i.id
        where {VISIBLE}
        and ($5='household' or i.owner_id=$2 or i.subject_member_id=$2 or exists(select 1
        from porchlight.item_assignees a where a.item_id=i.id and a.member_id=$2))
        and ($6::text is null or (o.status,o.board_position collate \"C\",o.id)>($6,$7 collate \"C\",$8))
        order by o.status,o.board_position collate \"C\",o.id limit $9"
    );
    let (status, position, id) = after
        .map(|(a, b, c)| (Some(a), Some(b), Some(c)))
        .unwrap_or_default();
    let rows: Vec<SqlJson<Value>> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(caller.household_id())
        .bind(caller.member_id())
        .bind(caller.sees_all())
        .bind(now)
        .bind(scope)
        .bind(status)
        .bind(position)
        .bind(id)
        .bind((limit + 1) as i64)
        .fetch_all(conn)
        .await?;
    rows.into_iter().map(|v| decorate(v.0, caller)).collect()
}
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct AgendaParams {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub member_id: Option<Uuid>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}
#[utoipa::path(get,path="/v1/agenda",params(AgendaParams),responses((status=200,description="Member agenda, authoritative ACL and masked hard conflicts",body=ItemReadPage),crate::openapi::UniversalProblems))]
pub async fn agenda(
    State(state): State<AppState>,
    caller: Caller,
    Query(p): Query<AgendaParams>,
) -> Result<Json<Page<Value>>, ApiError> {
    if p.from >= p.to || p.to - p.from > Duration::days(730) {
        return Err(ApiError::new(Problem::InvalidRequest));
    }
    let member = p.member_id.unwrap_or(caller.member_id());
    let limit = PageParams {
        cursor: None,
        limit: p.limit,
    }
    .limit()?;
    let context = format!(
        "agenda:{}:{}:{}:{member}:{}:{}",
        caller.household_id(),
        caller.member_id(),
        caller.sees_all(),
        p.from,
        p.to
    );
    let after: Option<(DateTime<Utc>, Uuid)> = p
        .cursor
        .as_deref()
        .map(|c| state.idem.cursor.decode(&context, c))
        .transpose()?;
    let anchor = "coalesce(a.starts_at,a.due_at,a.due_date::timestamp at time zone
        (porchlight.pl_effective_member_zone(am.role,am.tzid,ah.tzid)))";
    let sql = format!(
        "select {ITEM} || jsonb_build_object(
        '_agenda_key',{anchor},'agenda',to_jsonb(a)-'busy','occurrence',to_jsonb(o)-'during',
        'conflicts',coalesce((select jsonb_agg(jsonb_build_object(
            'member_id',b.member_id,'occurrence_id',a.occurrence_id,
            'other_occurrence_id',case when $3 or porchlight.pl_item_can_read(j.id,$2) then b.occurrence_id end,
            'other_item_id',case when $3 or porchlight.pl_item_can_read(j.id,$2) then j.id end,
            'other_title',case when $3 or porchlight.pl_item_can_read(j.id,$2) then j.title else 'Busy' end,
            'starts_at',lower(a.busy*b.busy),'ends_at',upper(a.busy*b.busy),'kind','hard')
        order by b.occurrence_id)
        from porchlight.member_agenda b
        join porchlight.items j on j.id=b.item_id and j.deleted_at is null
        where b.household_id=$1 and b.member_id=a.member_id
          and b.occurrence_id<>a.occurrence_id and b.busy && a.busy),'[]'::jsonb))
        {JOIN}
        join porchlight.member_agenda a on a.item_id=i.id
        join porchlight.item_occurrences o on o.id=a.occurrence_id
        join porchlight.household_members am on am.id=a.member_id
        join porchlight.households ah on ah.id=am.household_id
        where {VISIBLE} and a.household_id=$1 and a.member_id=$4
        and ($7::timestamptz is null or ({anchor},a.occurrence_id)>($7,$8)) and (
        (a.starts_at<$6 and a.ends_at>$5) or (a.due_at>=$5 and a.due_at<$6)
        or (a.due_date is not null and {anchor}<$6 and
          (a.due_date+1)::timestamp at time zone (porchlight.pl_effective_member_zone(am.role,am.tzid,ah.tzid))>$5))
        order by {anchor},a.occurrence_id limit $9"
    );
    let (after_time, after_id) = after
        .map(|(time, id)| (Some(time), Some(id)))
        .unwrap_or_default();
    let mut conn = state.pool.acquire().await?;
    let mut data: Vec<SqlJson<Value>> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(caller.household_id())
        .bind(caller.member_id())
        .bind(caller.sees_all())
        .bind(member)
        .bind(p.from)
        .bind(p.to)
        .bind(after_time)
        .bind(after_id)
        .bind((limit + 1) as i64)
        .fetch_all(&mut *conn)
        .await?;
    let ids: Vec<_> = data.iter().filter_map(|r| uid(&r.0["id"])).collect();
    let occurrences: Vec<_> = data
        .iter()
        .filter_map(|r| uid(&r.0["occurrence"]["id"]))
        .collect();
    let loads = porchlight_db::planning::load_conflicts_for_occurrences(
        &mut conn,
        &ids,
        member,
        &occurrences,
    )
    .await
    .map_err(crate::items::domain_error)?;
    let mut by_occurrence: std::collections::HashMap<Uuid, Vec<Value>> =
        std::collections::HashMap::new();
    for (_, conflict) in loads {
        let mut warning =
            serde_json::to_value(&conflict).map_err(|_| ApiError::internal("conflict"))?;
        warning["other_occurrence_id"] = Value::Null;
        by_occurrence
            .entry(conflict.occurrence_id)
            .or_default()
            .push(warning);
    }
    for row in &mut data {
        if let Some(warnings) =
            uid(&row.0["occurrence"]["id"]).and_then(|id| by_occurrence.remove(&id))
        {
            row.0["conflicts"].as_array_mut().unwrap().extend(warnings);
        }
    }
    drop(conn);
    let values = data
        .into_iter()
        .map(|v| {
            let key: DateTime<Utc> = serde_json::from_value(v.0["_agenda_key"].clone())
                .map_err(|_| ApiError::internal("agenda_key"))?;
            let id = uid(&v.0["agenda"]["occurrence_id"]).unwrap();
            let mut value = v.0;
            value.as_object_mut().unwrap().remove("_agenda_key");
            Ok(((key, id), decorate(value, &caller)?))
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    let page = state.idem.cursor.page(&context, values, limit, |v| v.0)?;
    Ok(Json(Page {
        items: page.items.into_iter().map(|v| v.1).collect(),
        next_cursor: page.next_cursor,
    }))
}
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct SearchParams {
    pub q: String,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}
#[utoipa::path(get,path="/v1/search",params(SearchParams),responses((status=200,description="Simple FTS over items, lists and comments; source/UUID keyset pagination bound to query and caller",body=SearchReadPage),crate::openapi::UniversalProblems))]
pub async fn search(
    State(state): State<AppState>,
    caller: Caller,
    Query(p): Query<SearchParams>,
) -> Result<Json<Page<Value>>, ApiError> {
    if p.q.trim().is_empty() || p.q.len() > 500 {
        return Err(ApiError::new(Problem::InvalidRequest));
    }
    let limit = PageParams {
        cursor: None,
        limit: p.limit,
    }
    .limit()?;
    let context = format!(
        "search-v2:{}:{}:{}:{}",
        caller.household_id(),
        caller.member_id(),
        caller.sees_all(),
        p.q
    );
    let after: Option<(String, Uuid)> = p
        .cursor
        .as_deref()
        .map(|c| state.idem.cursor.decode(&context, c))
        .transpose()?;
    // Every source shares $1..$4, one RR view, and one source/id keyset.
    // S8b availability/proposals have no independent text document. Their item
    // title is already indexed here with VISIBLE; do not duplicate or leak private proposals.
    let sql = format!(
        "SELECT source,id,value FROM (
        SELECT 'item'::text AS source,i.id,{ITEM} AS value {JOIN}
        WHERE {VISIBLE} AND i.search_document @@ plainto_tsquery('simple',$4)
        UNION ALL {lists}
        UNION ALL {comments}
        ) sources WHERE ($5::text IS NULL OR (source COLLATE \"C\",id)>($5 COLLATE \"C\",$6::uuid))
        ORDER BY source COLLATE \"C\",id LIMIT $7",
        lists = crate::lists::SEARCH_SOURCE,
        comments = crate::collaboration::comment_search_source()
    );
    let mut tx = porchlight_db::tx::begin(&state.pool).await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await?;
    let found: Vec<(String, Uuid, Option<SqlJson<Value>>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(caller.household_id())
            .bind(caller.member_id())
            .bind(caller.sees_all())
            .bind(&p.q)
            .bind(after.as_ref().map(|key| key.0.as_str()))
            .bind(after.as_ref().map(|key| key.1))
            .bind((limit + 1) as i64)
            .fetch_all(&mut *tx)
            .await?;
    let page = state
        .idem
        .cursor
        .page(&context, found, limit, |v| (v.0.clone(), v.1))?;
    let mut values = Vec::with_capacity(page.items.len());
    for (source, id, value) in page.items {
        let mut value = match source.as_str() {
            "item" => decorate(
                value.ok_or_else(|| ApiError::internal("search_item"))?.0,
                &caller,
            )?,
            "list" => match crate::lists::response(&mut tx, &caller, id, None, false).await {
                Ok(list) => {
                    serde_json::to_value(list).map_err(|_| ApiError::internal("search_list"))?
                }
                Err(e) if e.problem() == Problem::NotFound => continue,
                Err(e) => return Err(e),
            },
            "comment" => value.ok_or_else(|| ApiError::internal("search_comment"))?.0,
            _ => return Err(ApiError::internal("search_source")),
        };
        value["entity_type"] = json!(source);
        values.push(value);
    }
    tx.commit().await?;
    Ok(Json(Page {
        items: values,
        next_cursor: page.next_cursor,
    }))
}

async fn snapshot_candidates(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    household: Uuid,
    member: Uuid,
    sees_all: bool,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<SqlJson<Value>>, ApiError> {
    // SQL SUPERSET, not membership: separate generated and effective wall boxes
    // avoid bridging the window between an old identity and a far-future override.
    // Override shifts are unbounded, so include them directly. Local endpoints
    // differ from resolved UTC by <3 days: resolve_local walks <48h and offsets
    // are <24h. R10's corrected end has the same shift as its resolved start.
    // Date-only rows include the following midnight; undated rows always survive.
    let sql = format!("select to_jsonb(o)-'during' from porchlight.item_occurrences o
        join porchlight.items i on i.id=o.item_id
        cross join lateral (select
            least(o.starts_at at time zone 'UTC',o.due_at at time zone 'UTC',
                o.local_start,o.local_due,o.due_date::timestamp) as lo,
            greatest(o.starts_at at time zone 'UTC',o.ends_at at time zone 'UTC',
                o.due_at at time zone 'UTC',o.local_start,o.local_end,o.local_due,
                o.due_date::timestamp + interval '1 day') as hi) generated
        cross join lateral (select
            coalesce(o.override_local_start,o.local_start,o.starts_at at time zone 'UTC') as s,
            coalesce(o.override_local_end,o.local_end,o.ends_at at time zone 'UTC') as e,
            coalesce(o.override_local_due,o.local_due,o.due_at at time zone 'UTC') as d,
            coalesce(o.override_due_date,o.due_date)::timestamp as day) effective
        cross join lateral (select least(effective.s,effective.d,effective.day) as lo,
            greatest(effective.s,effective.e,effective.d,effective.day + interval '1 day') as hi) overridden
        where {VISIBLE} and (generated.lo is null or
            (generated.lo < ($5::timestamptz at time zone 'UTC') + interval '3 days'
             and generated.hi > ($4::timestamptz at time zone 'UTC') - interval '3 days') or
            (o.is_detached and overridden.lo < ($5::timestamptz at time zone 'UTC') + interval '3 days'
             and overridden.hi > ($4::timestamptz at time zone 'UTC') - interval '3 days'))
        order by o.item_id,o.seq");
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(household)
        .bind(member)
        .bind(sees_all)
        .bind(from)
        .bind(to)
        .fetch_all(&mut **tx)
        .await?)
}

fn snapshot_in_window(
    anchor: &porchlight_core::items::OccurrenceAnchor,
    zone: chrono_tz::Tz,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> bool {
    use porchlight_core::time::{resolve_local, resolve_span, start_of_day};
    let span = anchor
        .local_start
        .zip(anchor.local_end)
        .and_then(|(s, e)| resolve_span(s, e, zone).ok());
    let start = anchor
        .starts_at
        .or(anchor.due_at)
        .or_else(|| anchor.local_start.map(|s| resolve_local(s, zone)))
        .or_else(|| anchor.local_due.map(|d| resolve_local(d, zone)))
        .or_else(|| anchor.due_date.map(|d| start_of_day(d, zone)));
    let end = anchor
        .ends_at
        .or_else(|| span.map(|(_, e)| e))
        .or_else(|| anchor.local_end.map(|e| resolve_local(e, zone)))
        .or_else(|| {
            anchor
                .due_date
                .and_then(|d| d.succ_opt())
                .map(|d| start_of_day(d, zone))
        });
    start.is_none_or(|s| s < to && end.map_or(s >= from, |e| e > from))
}

#[utoipa::path(get,path="/v1/snapshot",responses((status=200,description="One REPEATABLE READ view; occurrences now-180d to 18 month horizon, plus undated occurrences; global published scan cursor",body=SnapshotRead),crate::openapi::UniversalProblems))]
pub async fn snapshot(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Value>, ApiError> {
    let now = (state.item_clock)();
    let mut tx = porchlight_db::tx::begin(&state.pool).await?;
    sqlx::query("set transaction isolation level repeatable read read only")
        .execute(&mut *tx)
        .await?;
    let cursor: i64 = sqlx::query_scalar(
        "select coalesce(max(stream_seq),0)
        from porchlight.outbox",
    )
    .fetch_one(&mut *tx)
    .await?;
    let items = rows(
        &mut tx,
        &caller,
        &format!(
            "select {ITEM}
        {JOIN}
        where {VISIBLE}
        order by i.id"
        ),
    )
    .await?;
    // Membership uses the effective anchors, not the recurrence identity's generated
    // date. Resolve in Rust with derivation's earlier-fold/next-valid-gap semantics;
    // PostgreSQL AT TIME ZONE chooses different fold/gap instants.
    let household_zone: String =
        sqlx::query_scalar("select tzid from porchlight.households where id=$1")
            .bind(caller.household_id())
            .fetch_one(&mut *tx)
            .await?;
    let household_zone = porchlight_db::derive::parse_zone(&household_zone)
        .map_err(|_| ApiError::internal("snapshot_zone"))?;
    let timing: std::collections::HashMap<Uuid, porchlight_db::derive::ItemRow> = items
        .iter()
        .filter_map(|v| match porchlight_db::derive::ItemRow::deserialize(v) {
            Ok(row) => Some((row.id, row)),
            Err(_) => {
                // Raw-SQL poison must not take down the whole household cache.
                // Keep JSON unchanged; log only identity/category, never input values.
                tracing::warn!(item_id=?v.get("id"), "snapshot item timing unavailable");
                None
            }
        })
        .collect();
    let from = now - Duration::days(180);
    let to = now
        .checked_add_months(Months::new(18))
        .ok_or_else(|| ApiError::internal("horizon"))?;
    let candidates = snapshot_candidates(
        &mut tx,
        caller.household_id(),
        caller.member_id(),
        caller.sees_all(),
        from,
        to,
    )
    .await?;
    let mut occurrences = Vec::new();
    for value in candidates {
        let exact = (|| -> std::result::Result<bool, &'static str> {
            let row = porchlight_db::derive::OccurrenceRow::deserialize(&value.0)
                .map_err(|_| "occurrence")?;
            let item = timing.get(&row.item_id).ok_or("item")?;
            let fixed_zone = item
                .timing
                .tzid
                .as_deref()
                .map(porchlight_db::derive::parse_zone)
                .transpose()
                .map_err(|_| "zone")?;
            let anchor = porchlight_db::derive::effective_anchor_with_zone(
                &row,
                item.timing.time_mode,
                fixed_zone,
            )
            .map_err(|_| "override")?;
            Ok(snapshot_in_window(&anchor, household_zone, from, to))
        })();
        let include = match exact {
            Ok(include) => include,
            Err(category) => {
                tracing::warn!(occurrence_id=?value.0.get("id"), category, "snapshot timing refinement unavailable");
                true // SQL-bounded fail-open for this row; retain authoritative JSON.
            }
        };
        if include {
            occurrences.push(value);
        }
    }
    let occurrence_ids: Vec<Uuid> = occurrences.iter().filter_map(|v| uid(&v.0["id"])).collect();
    let reminders_sql = format!(
        "select to_jsonb(r)
        from porchlight.scheduled_reminders r
        join porchlight.items i on i.id=r.item_id
        where {VISIBLE}
        and ($3 or r.member_id=$2) and r.fire_at >= $4 and r.fire_at < $5
        and r.occurrence_id=any($6)
        order by r.fire_at,r.id"
    );
    let scheduled_reminders: Vec<SqlJson<Value>> =
        sqlx::query_scalar(sqlx::AssertSqlSafe(reminders_sql.as_str()))
            .bind(caller.household_id())
            .bind(caller.member_id())
            .bind(caller.sees_all())
            .bind(now - Duration::days(180))
            .bind(
                now.checked_add_months(Months::new(18))
                    .ok_or_else(|| ApiError::internal("horizon"))?,
            )
            .bind(&occurrence_ids)
            .fetch_all(&mut *tx)
            .await?;
    let members = super::members::all(&mut tx, caller.household_id(), caller.member_id()).await?;
    let settings = super::members::settings_row(&mut tx, &caller).await?;
    let (notifications, notification_prefs) =
        super::notifications::snapshot_fields(&mut tx, &caller).await?;
    let ids: Vec<Uuid> = items.iter().filter_map(|v| uid(&v["id"])).collect();
    let comments = crate::collaboration::snapshot_comments_s4b(&mut tx, &caller, &ids).await?;
    let attachments = crate::blobs::snapshot_attachments_s4b(&mut tx, &caller, &ids).await?;
    let ics_sources = crate::calendar::snapshot_sources(&mut tx, &caller).await?;
    let planning = crate::planning::snapshot_contribution(&mut tx, &caller).await?;
    let lists = crate::lists::snapshot_contribution(&mut tx, &caller).await?;
    tx.commit().await?;
    Ok(Json(
        json!({"comments":comments,"attachments":attachments,"ics_sources":ics_sources,"lists":lists,"notifications":notifications,"notification_prefs":notification_prefs,"availability":planning.availability,"suggestions":planning.suggestions,"members":members,"items":items,"occurrences":occurrences.into_iter().map(|v|v.0).collect::<Vec<_>>(),"member_settings":settings,"scheduled_reminders":scheduled_reminders.into_iter().map(|v|v.0).collect::<Vec<_>>(),"cursor":cursor}),
    ))
}
#[derive(utoipa::OpenApi)]
#[openapi(paths(board, agenda, search, snapshot))]
pub struct ReadsApi;

#[cfg(test)]
mod snapshot_window_tests {
    use chrono::{DateTime, Duration, Months, Utc};
    #[sqlx::test(migrations = "../db/migrations")]
    async fn snapshot_sql_never_loads_far_history_but_keeps_moved_in(pool: sqlx::PgPool) {
        use porchlight_core::items as core;
        use porchlight_db::{
            items,
            testing::seed_household,
            tx::{Actor, begin_as},
        };
        let h = seed_household(&pool).await.unwrap();
        let mut tx = begin_as(&pool, &Actor::system()).await.unwrap();
        let old: DateTime<Utc> = "2000-01-01T12:00:00Z".parse().unwrap();
        let now: DateTime<Utc> = "2026-10-06T12:00:00Z".parse().unwrap();
        let fields = items::ItemWrite {
            owner_id: h.user,
            title: "Far history".into(),
            timing: core::SeriesInput {
                kind: core::Kind::Deadline,
                tzid: Some("UTC".into()),
                due_at: Some(old),
                ..Default::default()
            },
            ..Default::default()
        };
        let out = items::create(&mut tx, h.id, &fields, &Default::default(), now)
            .await
            .unwrap();
        let from = now - Duration::days(180);
        let to = now.checked_add_months(Months::new(18)).unwrap();
        let loaded = super::snapshot_candidates(&mut tx, h.id, h.user, true, from, to)
            .await
            .unwrap();
        assert!(
            loaded.is_empty(),
            "far history must not be transferred to Rust"
        );
        sqlx::query("update porchlight.item_occurrences set override_local_due='2026-10-07T12:00:00' where item_id=$1").bind(out.item_id).execute(&mut *tx).await.unwrap();
        let loaded = super::snapshot_candidates(&mut tx, h.id, h.user, true, from, to)
            .await
            .unwrap();
        assert_eq!(
            loaded.len(),
            1,
            "far generated time with moved-in override must survive prefilter"
        );
        sqlx::query("update porchlight.item_occurrences set override_local_due='2029-10-07T12:00:00' where item_id=$1").bind(out.item_id).execute(&mut *tx).await.unwrap();
        let loaded = super::snapshot_candidates(&mut tx, h.id, h.user, true, from, to)
            .await
            .unwrap();
        assert!(
            loaded.is_empty(),
            "generated history and far-future override must not bridge the window"
        );
        tx.rollback().await.unwrap();
    }

    use super::snapshot_in_window;
    use porchlight_core::items::OccurrenceAnchor;
    fn instant(s: &str) -> chrono::DateTime<chrono::Utc> {
        s.parse().unwrap()
    }
    fn local(s: &str) -> chrono::NaiveDateTime {
        s.parse().unwrap()
    }
    #[test]
    fn floating_fold_and_gap_use_core_resolution_and_preserved_duration() {
        let zone = chrono_tz::America::New_York;
        let fold = OccurrenceAnchor {
            local_due: Some(local("2026-11-01T01:30:00")),
            ..Default::default()
        };
        assert!(snapshot_in_window(
            &fold,
            zone,
            instant("2026-11-01T05:00:00Z"),
            instant("2026-11-01T06:00:00Z")
        ));
        assert!(!snapshot_in_window(
            &fold,
            zone,
            instant("2026-11-01T06:00:00Z"),
            instant("2026-11-01T07:00:00Z")
        ));
        let gap = OccurrenceAnchor {
            local_start: Some(local("2026-03-08T02:10:00")),
            local_end: Some(local("2026-03-08T02:50:00")),
            ..Default::default()
        };
        assert!(snapshot_in_window(
            &gap,
            zone,
            instant("2026-03-08T07:20:00Z"),
            instant("2026-03-08T07:30:00Z")
        ));
        assert!(!snapshot_in_window(
            &gap,
            zone,
            instant("2026-03-08T07:40:00Z"),
            instant("2026-03-08T08:00:00Z")
        ));
    }
    #[test]
    fn undated_and_overlapping_date_spans_preserve_half_open_membership() {
        let zone = chrono_tz::UTC;
        let from = instant("2026-10-06T12:00:00Z");
        let to = instant("2026-10-07T12:00:00Z");
        assert!(snapshot_in_window(
            &OccurrenceAnchor::default(),
            zone,
            from,
            to
        ));
        let day = OccurrenceAnchor {
            due_date: Some("2026-10-06".parse().unwrap()),
            ..Default::default()
        };
        assert!(snapshot_in_window(&day, zone, from, to));
        let point = OccurrenceAnchor {
            due_at: Some(to),
            ..Default::default()
        };
        assert!(!snapshot_in_window(&point, zone, from, to));
    }
}
