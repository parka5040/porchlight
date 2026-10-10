//! S8b transaction orchestration: one scope, one savepoint, shared item ACLs.
use crate::{
    error::{ApiError, FieldError, Problem},
    extract::Caller,
    idempotency::IdemTx,
    items,
    routes::planning::*,
    state::AppState,
};
use axum::{
    Json,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use porchlight_core::{access::Capability, items::TimeMode, schedule as core, time};
use porchlight_db::{
    derive,
    items::{ScopeRequest, WriteScope},
    planning as db,
    tx::{Actor, begin_as},
};
use serde_json::Value;
use sqlx::{Acquire, PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;
fn invalid(field: &str, reason: &str) -> ApiError {
    ApiError::validation(vec![FieldError {
        field: field.into(),
        message: reason.into(),
        code: None,
    }])
}
fn target(tx: &IdemTx) -> Result<Uuid, ApiError> {
    tx.context
        .target_id
        .ok_or_else(|| ApiError::new(Problem::InvalidRequest))
}
fn self_or_admin(c: &Caller, member: Uuid) -> Result<(), ApiError> {
    if c.member_id() == member || c.sees_all() {
        Ok(())
    } else {
        c.require_admin()
    }
}
pub async fn check_member(
    conn: &mut PgConnection,
    c: &Caller,
    member: Uuid,
) -> Result<(), ApiError> {
    self_or_admin(c, member)?;
    let exists:bool=sqlx::query_scalar("select exists(select 1 from porchlight.household_members where id=$1 and household_id=$2 and deleted_at is null)").bind(member).bind(c.household_id()).fetch_one(conn).await?;
    if !exists {
        return Err(ApiError::new(Problem::NotFound));
    }
    Ok(())
}
pub async fn availability(
    conn: &mut PgConnection,
    member: Uuid,
) -> Result<AvailabilityResponse, ApiError> {
    let version: Option<i32> = sqlx::query_scalar(
        "select version from porchlight.availability_profiles where member_id=$1",
    )
    .bind(member)
    .fetch_optional(&mut *conn)
    .await?;
    let windows = db::windows(conn, member)
        .await
        .map_err(items::domain_error)?
        .into_iter()
        .map(|w| Window {
            weekday: w.weekday,
            start: w.starts_local,
            end: w.ends_local,
        })
        .collect();
    Ok(AvailabilityResponse {
        member_id: member,
        version: version.unwrap_or(0),
        windows,
        conflicts: vec![],
    })
}
/// R2 agenda membership is authoritative; never derive EDF inputs from the caller's Edit rights.
async fn agenda_tasks(
    conn: &mut PgConnection,
    household: Uuid,
    member: Uuid,
) -> Result<Vec<Uuid>, ApiError> {
    let sql = format!(
        "select i.id from porchlight.items i where i.household_id=$1 and i.deleted_at is null and i.kind in ('todo','deadline') and {} order by i.id",
        db::agenda_membership_sql("$2")
    );
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(household)
        .bind(member)
        .fetch_all(conn)
        .await?)
}
/// The shared SQL visibility predicate; capability checks remain in items.
async fn visible_ids(
    conn: &mut PgConnection,
    c: &Caller,
    ids: &[Uuid],
) -> Result<Vec<Uuid>, ApiError> {
    let sql = format!(
        "select i.id from porchlight.items i where {} and i.id=any($4) order by i.id",
        crate::routes::reads::VISIBLE
    );
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(c.household_id())
        .bind(c.member_id())
        .bind(c.sees_all())
        .bind(ids)
        .fetch_all(conn)
        .await?)
}
async fn visible_tasks(
    conn: &mut PgConnection,
    c: &Caller,
    member: Uuid,
) -> Result<Vec<Uuid>, ApiError> {
    let ids = agenda_tasks(conn, c.household_id(), member).await?;
    visible_ids(conn, c, &ids).await
}
fn rejected(field: &str, code: &str) -> ApiError {
    ApiError::validation(vec![FieldError {
        field: field.into(),
        message: "Suggestion is no longer eligible".into(),
        code: Some(code.into()),
    }])
}
async fn check_run_member(
    conn: &mut PgConnection,
    c: &Caller,
    member: Uuid,
) -> Result<(), ApiError> {
    self_or_admin(c, member)?;
    let exists: bool = sqlx::query_scalar("select exists(select 1 from porchlight.household_members where id=$1 and household_id=$2 and deleted_at is null)")
        .bind(member).bind(c.household_id()).fetch_one(&mut *conn).await?;
    if !exists {
        return Err(items::reference_error("member_id", "invalid-reference"));
    }
    if member != c.member_id() {
        c.require_admin()?;
        let dependent: bool = sqlx::query_scalar(
            "select role='dependent' from porchlight.household_members where id=$1",
        )
        .bind(member)
        .fetch_one(conn)
        .await?;
        if !dependent {
            return Err(ApiError::new(Problem::Forbidden));
        }
    }
    Ok(())
}
async fn accept_preconditions(
    conn: &mut PgConnection,
    row: &db::Suggestion,
    now: DateTime<Utc>,
) -> Result<(), ApiError> {
    let sql = format!(
        "select exists(select 1 from porchlight.items i where i.id=$1 and i.deleted_at is null and {})",
        db::agenda_membership_sql("$2")
    );
    let member: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(row.item_id)
        .bind(row.member_id)
        .fetch_one(conn)
        .await?;
    if !member {
        return Err(rejected("member_id", "not-agenda-member"));
    }
    if row.proposed_start < now - chrono::Duration::minutes(5) {
        return Err(rejected("proposed_start", "not-future"));
    }
    Ok(())
}
async fn suggestion_wire(
    conn: &mut PgConnection,
    row: db::Suggestion,
    conflicts: Vec<crate::routes::items::ConflictResponse>,
) -> Result<SuggestionResponse, ApiError> {
    let occurrence_version = sqlx::query_scalar(
        "select version from porchlight.item_occurrences where id=$1 and item_id=$2",
    )
    .bind(row.occurrence_id)
    .bind(row.item_id)
    .fetch_optional(conn)
    .await?;
    Ok(suggestion_response(row, occurrence_version, conflicts))
}
fn suggestion_response(
    row: db::Suggestion,
    occurrence_version: Option<i32>,
    conflicts: Vec<crate::routes::items::ConflictResponse>,
) -> SuggestionResponse {
    SuggestionResponse {
        id: row.id,
        member_id: row.member_id,
        item_id: row.item_id,
        occurrence_id: row.occurrence_id,
        proposed_start: row.proposed_start,
        proposed_end: row.proposed_end,
        status: row.status,
        run_id: row.run_id,
        version: row.version,
        occurrence_version,
        created_at: row.created_at,
        conflicts,
    }
}
async fn checked_suggestion(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
    edit: bool,
) -> Result<db::Suggestion, ApiError> {
    let row = db::suggestion(conn, c.household_id(), id)
        .await
        .map_err(items::domain_error)?;
    if !edit {
        self_or_admin(c, row.member_id)?;
    }
    let item = derive::load_item(conn, row.item_id)
        .await
        .map_err(items::domain_error)?;
    items::authorize_item(conn, c, &item, Capability::View).await?;
    if edit && c.member_id() != row.member_id {
        items::authorize_item(conn, c, &item, Capability::Edit).await?;
    }
    Ok(row)
}
#[derive(Clone, Copy)]
pub enum Operation {
    Availability,
    Suggest,
    Accept,
    Dismiss,
}
pub async fn mutate(
    s: &AppState,
    c: Caller,
    tx: &mut IdemTx,
    op: Operation,
    body: Value,
) -> Result<Response, ApiError> {
    let id = match op {
        Operation::Suggest => c.member_id(),
        _ => target(tx)?,
    };
    let headers = tx.context.if_match.clone();
    let now = (s.item_clock)();
    let mut domain = tx.tx.begin().await?;
    let result = execute(&mut domain, &c, id, &headers, op, body, now).await;
    match result {
        Ok(response) => {
            domain.commit().await?;
            Ok(response)
        }
        Err(e) => {
            domain.rollback().await?;
            // Binding S4a contract: enrich stale occurrence errors only AFTER all
            // domain effects/locks/markers are rolled back, using the same IdemTx.
            if matches!(op, Operation::Accept) && e.problem() == Problem::VersionConflict {
                let row = checked_suggestion(&mut tx.tx, &c, id, true).await?;
                Err(items::attach_current(&mut tx.tx, &c, row.occurrence_id, true, e).await)
            } else {
                Err(e)
            }
        }
    }
}
async fn execute(
    tx: &mut Transaction<'_, Postgres>,
    c: &Caller,
    id: Uuid,
    headers: &axum::http::HeaderMap,
    op: Operation,
    body: Value,
    now: DateTime<Utc>,
) -> Result<Response, ApiError> {
    match op {
        Operation::Availability => {
            check_member(tx, c, id).await?;
            let expected = crate::protocol::if_match(headers)?;
            let body: AvailabilityRequest =
                serde_json::from_value(body).map_err(|_| ApiError::new(Problem::InvalidRequest))?;
            if body.windows.len() > 100 {
                return Err(invalid("windows", "At most 100 windows"));
            }
            let mut unique = std::collections::BTreeSet::new();
            for w in &body.windows {
                if !(0..=6).contains(&w.weekday)
                    || w.start.to_string().contains('.')
                    || w.end.to_string().contains('.')
                {
                    return Err(invalid(
                        "windows",
                        "Weekday must be 0–6; times must have whole seconds",
                    ));
                }
                if !unique.insert((w.weekday, w.start, w.end)) {
                    return Err(invalid("windows", "Duplicate window"));
                }
            }
            let scope = WriteScope::begin_at(
                tx,
                c.household_id(),
                ScopeRequest {
                    members: vec![id],
                    ..Default::default()
                },
                now,
            )
            .await
            .map_err(items::domain_error)?;
            scope.finish(now).await.map_err(items::domain_error)?;
            // Lock existing windows before the profile, matching raw child→profile triggers.
            sqlx::query("select id from porchlight.availability_windows where member_id=$1 order by id for update").bind(id).fetch_all(&mut **tx).await?;
            let current = availability(tx, id).await?;
            if current.version != expected {
                return Err(ApiError::new(Problem::VersionConflict)
                    .extension("current", serde_json::to_value(current).unwrap()));
            }
            let inserted=sqlx::query("insert into porchlight.availability_profiles(member_id,household_id) values($1,$2) on conflict do nothing").bind(id).bind(c.household_id()).execute(&mut **tx).await?.rows_affected();
            sqlx::query("select member_id from porchlight.availability_profiles where member_id=$1 for update").bind(id).fetch_one(&mut **tx).await?;
            let locked = availability(tx, id).await?;
            if !(expected == 0 && inserted == 1) && locked.version != expected {
                return Err(ApiError::new(Problem::VersionConflict)
                    .extension("current", serde_json::to_value(locked).unwrap()));
            }
            sqlx::query(
                "update porchlight.availability_profiles set updated_at=now() where member_id=$1",
            )
            .bind(id)
            .execute(&mut **tx)
            .await?;
            // An empty→empty PUT still has a collection version and member-scoped tickle.
            sqlx::query("delete from porchlight.availability_windows where member_id=$1")
                .bind(id)
                .execute(&mut **tx)
                .await?;
            sqlx::query("insert into porchlight.availability_windows(household_id,member_id,weekday,starts_local,ends_local) select $1,$2,w.weekday,w.start,w.\"end\" from jsonb_to_recordset($3) as w(weekday smallint,start time,\"end\" time)").bind(c.household_id()).bind(id).bind(sqlx::types::Json(&body.windows)).execute(&mut **tx).await?;
            sqlx::query("select porchlight.pl_enqueue_member_load($1)")
                .bind(id)
                .execute(&mut **tx)
                .await?;
            db::recompute_load(tx, id, now)
                .await
                .map_err(items::domain_error)?;
            let mut response = availability(tx, id).await?;
            let allowed = visible_tasks(tx, c, id).await?;
            let raw = db::load_conflicts(tx, &allowed, Some(&[id]))
                .await
                .map_err(items::domain_error)?
                .into_iter()
                .map(|(_, c)| c)
                .collect();
            response.conflicts = items::mask_conflicts(tx, c, raw).await?;
            Ok(Json(response).into_response())
        }
        Operation::Suggest => {
            let body: SuggestRequest =
                serde_json::from_value(body).map_err(|_| ApiError::new(Problem::InvalidRequest))?;
            let member = body.member_id.unwrap_or(c.member_id());
            check_run_member(tx, c, member).await?;
            let scope = WriteScope::begin_at(
                tx,
                c.household_id(),
                ScopeRequest {
                    members: vec![member],
                    ..Default::default()
                },
                now,
            )
            .await
            .map_err(items::domain_error)?;
            scope.finish(now).await.map_err(items::domain_error)?;
            let allowed = agenda_tasks(tx, c.household_id(), member).await?;
            let (run_id, result, rows) = db::suggest(tx, c.household_id(), member, &allowed, now)
                .await
                .map_err(items::domain_error)?;
            let mut suggestions = vec![];
            let visible =
                visible_ids(tx, c, &rows.iter().map(|r| r.item_id).collect::<Vec<_>>()).await?;
            for row in rows {
                if visible.contains(&row.item_id) {
                    suggestions.push(suggestion_wire(tx, row, vec![]).await?)
                }
            }
            let unplaced_ids: Vec<_> = result.unplaced.iter().map(|u| u.task_id).collect();
            let unplaced_items: Vec<(Uuid, Uuid)> = sqlx::query_as(
                "select id,item_id from porchlight.item_occurrences where id=any($1)",
            )
            .bind(&unplaced_ids)
            .fetch_all(&mut **tx)
            .await?;
            let visible_unplaced = visible_ids(
                tx,
                c,
                &unplaced_items
                    .iter()
                    .map(|(_, item)| *item)
                    .collect::<Vec<_>>(),
            )
            .await?;
            let visible_occurrences: std::collections::BTreeSet<_> = unplaced_items
                .into_iter()
                .filter(|(_, item)| visible_unplaced.contains(item))
                .map(|(id, _)| id)
                .collect();
            let unplaced = result
                .unplaced
                .into_iter()
                .filter(|u| visible_occurrences.contains(&u.task_id))
                .map(|u| Unplaced {
                    occurrence_id: u.task_id,
                    reason: serde_json::to_value(u.reason)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .into(),
                })
                .collect();
            Ok(Json(SuggestResponse {
                run_id,
                suggestions,
                unplaced,
            })
            .into_response())
        }
        Operation::Accept | Operation::Dismiss => {
            let expected = crate::protocol::if_match(headers)?;
            let row = checked_suggestion(tx, c, id, matches!(op, Operation::Accept)).await?;
            if matches!(op, Operation::Dismiss) {
                let scope = WriteScope::begin_at(
                    tx,
                    c.household_id(),
                    ScopeRequest {
                        members: vec![row.member_id],
                        ..Default::default()
                    },
                    now,
                )
                .await
                .map_err(items::domain_error)?;
                scope.finish(now).await.map_err(items::domain_error)?;
                let current = checked_suggestion(tx, c, id, false).await?;
                if current.version != expected {
                    return Err(ApiError::new(Problem::VersionConflict).extension(
                        "current",
                        serde_json::to_value(suggestion_wire(tx, current, vec![]).await?).unwrap(),
                    ));
                }
                if current.status != "proposed" {
                    return Err(rejected("suggestion", "not-proposed"));
                }
                let changed=sqlx::query("update porchlight.schedule_suggestions set status='dismissed' where id=$1 and version=$2").bind(id).bind(expected).execute(&mut **tx).await?.rows_affected();
                if changed != 1 {
                    return Err(ApiError::new(Problem::VersionConflict).extension(
                        "current",
                        serde_json::to_value(suggestion_wire(tx, current, vec![]).await?).unwrap(),
                    ));
                }
                let current_suggestion = db::suggestion(tx, c.household_id(), id)
                    .await
                    .map_err(items::domain_error)?;
                return Ok(
                    Json(suggestion_wire(tx, current_suggestion, vec![]).await?).into_response()
                );
            }
            accept_preconditions(tx, &row, now).await?;
            let member_schedule = db::member_schedule(tx, c.household_id(), row.member_id)
                .await
                .map_err(items::domain_error)?;
            let mut scope = WriteScope::begin_at(
                tx,
                c.household_id(),
                ScopeRequest {
                    items: vec![row.item_id],
                    occurrences: vec![row.occurrence_id],
                    members: vec![row.member_id],
                    ..Default::default()
                },
                now,
            )
            .await
            .map_err(items::domain_error)?;
            let item = scope.item(row.item_id).await.map_err(items::domain_error)?;
            let occurrence = scope
                .occurrence(row.occurrence_id)
                .await
                .map_err(items::domain_error)?;
            if occurrence.version != expected {
                return Err(ApiError::new(Problem::VersionConflict));
            }
            if !occurrence.status.is_open()
                || occurrence.disposition == porchlight_core::items::Disposition::Expired
                || row.status != "proposed"
            {
                return Err(rejected("suggestion", "not-proposed"));
            }
            // Retain task kind and deadline; set just this occurrence's local span.
            let zone = if item.timing.time_mode == TimeMode::Fixed {
                item.spec()
                    .map_err(items::domain_error)?
                    .zone()
                    .unwrap_or(member_schedule.tz)
            } else {
                member_schedule.tz
            };
            let start = time::to_local(row.proposed_start, zone);
            let end = time::to_local(row.proposed_end, zone);
            if time::resolve_local(start, zone) != row.proposed_start
                || time::resolve_local(end, zone) != row.proposed_end
            {
                return Err(invalid(
                    "suggestion",
                    "Proposal cannot be represented by the occurrence DST policy",
                ));
            }
            let mut replacement =
                derive::effective_anchor(&occurrence, &item.spec().map_err(items::domain_error)?)
                    .map_err(items::domain_error)?;
            replacement.local_start = Some(start);
            replacement.local_end = Some(end);
            scope
                .override_occurrence(row.occurrence_id, expected, &replacement)
                .await
                .map_err(items::domain_error)?;
            let outcomes = scope.finish(now).await.map_err(items::domain_error)?;
            let locked_schedule = db::member_schedule(tx, c.household_id(), row.member_id)
                .await
                .map_err(items::domain_error)?;
            if locked_schedule.tz != member_schedule.tz {
                return Err(ApiError::transient(Problem::Conflict).retry(1));
            }
            let current = checked_suggestion(tx, c, id, true).await?;
            accept_preconditions(tx, &current, now).await?;
            if current.status != "proposed" || current.version != row.version {
                return Err(rejected("suggestion", "not-proposed"));
            }
            let changed=sqlx::query("update porchlight.schedule_suggestions set status='accepted' where id=$1 and version=$2").bind(id).bind(row.version).execute(&mut **tx).await?.rows_affected();
            if changed != 1 {
                return Err(rejected("suggestion", "not-proposed"));
            }
            let raw = outcomes
                .into_iter()
                .flat_map(|o| o.derived.conflicts)
                .collect();
            let conflicts = items::mask_conflicts(tx, c, raw).await?;
            let current = db::suggestion(tx, c.household_id(), id)
                .await
                .map_err(items::domain_error)?;
            Ok(Json(suggestion_wire(tx, current, conflicts).await?).into_response())
        }
    }
}
pub async fn list(
    s: &AppState,
    c: &Caller,
    q: SuggestionsQuery,
) -> Result<SuggestionsPage, ApiError> {
    let member = q.member_id.unwrap_or(c.member_id());
    let limit = crate::pagination::PageParams {
        cursor: q.cursor.clone(),
        limit: q.limit,
    }
    .limit()?;
    let context = format!(
        "suggestions:{}:{}:{}",
        c.household_id(),
        c.member_id(),
        member
    );
    let cursor = q
        .cursor
        .as_ref()
        .map(|v| s.idem.cursor.decode::<Uuid>(&context, v))
        .transpose()?
        .unwrap_or(Uuid::nil());
    let mut tx = items::read_snapshot(s).await?;
    check_member(&mut tx, c, member).await?;
    // Visibility before LIMIT in the same RR snapshot; never scan hidden pages.
    let sql = format!(
        "select s.id,s.household_id,s.member_id,s.item_id,s.occurrence_id,s.proposed_start,s.proposed_end,s.status,s.run_id,s.version,s.created_at from porchlight.schedule_suggestions s join porchlight.items i on i.id=s.item_id where {} and s.member_id=$4 and s.id>$5 order by s.id limit $6",
        crate::routes::reads::VISIBLE
    );
    let visible: Vec<db::Suggestion> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(c.household_id())
        .bind(c.member_id())
        .bind(c.sees_all())
        .bind(member)
        .bind(cursor)
        .bind((limit + 1) as i64)
        .fetch_all(&mut *tx)
        .await?;
    let page = s.idem.cursor.page(&context, visible, limit, |r| r.id)?;
    let mut rows = vec![];
    for row in page.items {
        rows.push(suggestion_wire(&mut tx, row, vec![]).await?)
    }
    Ok(SuggestionsPage {
        items: rows,
        next_cursor: page.next_cursor,
    })
}
pub async fn candidates(
    conn: &mut PgConnection,
    c: &Caller,
    id: Uuid,
    now: DateTime<Utc>,
) -> Result<CandidatesResponse, ApiError> {
    let item = derive::load_item(conn, id)
        .await
        .map_err(items::domain_error)?;
    items::authorize_item(conn, c, &item, Capability::View).await?;
    let sets = porchlight_db::items::relationships(conn, id)
        .await
        .map_err(items::domain_error)?;
    let dependent:bool=sqlx::query_scalar("select exists(select 1 from porchlight.household_members where id=$1 and household_id=$2 and role='dependent' and deleted_at is null)").bind(item.subject_member_id).bind(c.household_id()).fetch_one(&mut *conn).await?;
    if item.timing.kind != porchlight_core::items::Kind::Appointment
        || !dependent
        || !sets.assignees.is_empty()
    {
        return Err(invalid(
            "item_id",
            "Requires an unassigned dependent appointment",
        ));
    }
    let (pending_derivation, attempts): (bool, i32) = sqlx::query_as(
        "select requested_gen>applied_gen,attempts from porchlight.item_derivation where item_id=$1",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    if pending_derivation {
        // Poisoned parents are omitted, not ranked from stale projections and
        // not represented as indefinitely pending. Their derive error remains
        // available through the shared operational status surface.
        return Ok(CandidatesResponse {
            item_id: id,
            occurrence_id: None,
            candidates: vec![],
            pending_derivation: attempts < 5,
        });
    }
    let occurrence:Option<(Uuid,DateTime<Utc>,DateTime<Utc>)>=sqlx::query_as("select a.occurrence_id,lower(a.busy),upper(a.busy) from porchlight.member_agenda a join porchlight.item_occurrences o on o.id=a.occurrence_id where a.item_id=$1 and a.member_id=$2 and a.busy is not null and o.status in ('backlog','todo','doing') and o.disposition<>'expired' and a.starts_at>=$3 order by a.starts_at,a.occurrence_id limit 1").bind(id).bind(item.subject_member_id).bind(now).fetch_optional(&mut *conn).await?;
    let Some((occurrence_id, start, end)) = occurrence else {
        return Ok(CandidatesResponse {
            item_id: id,
            occurrence_id: None,
            candidates: vec![],
            pending_derivation: false,
        });
    };
    let ids:Vec<Uuid>=sqlx::query_scalar("select id from porchlight.household_members where household_id=$1 and deleted_at is null and role in ('user','super_admin') order by id").bind(c.household_id()).fetch_all(&mut *conn).await?;
    let sql = format!(
        "select exists(select 1 from porchlight.items i join porchlight.item_derivation d on d.item_id=i.id where i.household_id=$1 and i.deleted_at is null and d.applied_gen<d.requested_gen and d.attempts<5 and ({} or exists(select 1 from porchlight.member_agenda a where a.item_id=i.id and a.member_id=any($2))))",
        db::agenda_membership_sql("any($2)")
    );
    let peers_pending: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(c.household_id())
        .bind(&ids)
        .fetch_one(&mut *conn)
        .await?;
    if peers_pending {
        return Ok(CandidatesResponse {
            item_id: id,
            occurrence_id: Some(occurrence_id),
            candidates: vec![],
            pending_derivation: true,
        });
    }
    let mut schedules = vec![];
    for member in ids {
        schedules.push(
            db::member_schedule(conn, c.household_id(), member)
                .await
                .map_err(items::domain_error)?,
        )
    }
    let candidates = core::dependent_candidates(core::TimeRange { start, end }, &schedules)
        .into_iter()
        .map(|c| Candidate {
            member_id: c.member_id,
            reason: c.reason,
        })
        .collect();
    Ok(CandidatesResponse {
        item_id: id,
        occurrence_id: Some(occurrence_id),
        candidates,
        pending_derivation: false,
    })
}
/// Availability-only jobs hold no domain locks; member first, queue NOWAIT last.
pub async fn run_load_batch(pool: &PgPool, now: DateTime<Utc>) -> anyhow::Result<usize> {
    let ids:Vec<Uuid>=sqlx::query_scalar("select member_id from porchlight.member_load_jobs where requested_gen>applied_gen and attempts<5 and retry_at<=$1 union select member_id from porchlight.member_load_signals order by member_id limit 50").bind(now).fetch_all(pool).await?;
    let mut count = 0;
    for member in ids {
        match run_load_member(pool, member, now).await {
            Ok(true) => count += 1,
            Ok(false) => {}
            Err(error) => {
                // Never log database messages/parameters. This includes acquisition,
                // signal folding, savepoint, retry bookkeeping and commit errors.
                let category = if let Some(error) = error.downcast_ref::<sqlx::Error>() {
                    match error.as_database_error().and_then(|e| e.code()).as_deref() {
                        Some("55P03") => "lock_unavailable",
                        Some("40P01") => "deadlock",
                        Some("40001") => "serialization",
                        Some(_) => "database",
                        None => "connection",
                    }
                } else {
                    "internal"
                };
                tracing::warn!(%member, category, "load member deferred");
            }
        }
    }
    Ok(count)
}
async fn run_load_member(pool: &PgPool, member: Uuid, now: DateTime<Utc>) -> anyhow::Result<bool> {
    let mut tx = begin_as(pool, &Actor::system()).await?;
    let held: Option<Uuid> = sqlx::query_scalar(
        "select id from porchlight.household_members where id=$1 for no key update skip locked",
    )
    .bind(member)
    .fetch_optional(&mut *tx)
    .await?;
    if held.is_none() {
        tx.rollback().await?;
        // A skipped live member is left alone; only physically orphaned wakeups are drained.
        sqlx::query("delete from porchlight.member_load_signals s where s.member_id=$1 and not exists(select 1 from porchlight.household_members m where m.id=s.member_id)").bind(member).execute(pool).await?;
        return Ok(false);
    }
    // Fold immutable peer wakeups while holding the member (worker never owns item locks).
    let signals: Vec<Uuid> = sqlx::query_scalar(
        "select id from porchlight.member_load_signals where member_id=$1 order by id",
    )
    .bind(member)
    .fetch_all(&mut *tx)
    .await?;
    if !signals.is_empty() {
        sqlx::query("select porchlight.pl_enqueue_member_load($1)")
            .bind(member)
            .execute(&mut *tx)
            .await?;
        sqlx::query("delete from porchlight.member_load_signals where id=any($1)")
            .bind(signals)
            .execute(&mut *tx)
            .await?;
    }
    let state: (i64,i32)=sqlx::query_as("select requested_gen,attempts from porchlight.member_load_jobs where member_id=$1 for update nowait").bind(member).fetch_one(&mut *tx).await?;
    let mut sp = tx.begin().await?;
    let mut processed = false;
    let result = db::recompute_load(&mut sp, member, now).await;
    match result {
        Ok(()) => {
            sp.commit().await?;
            processed = true;
        }
        Err(error) => {
            sp.rollback().await?;
            let category = derive::error_category(&error);
            let attempts = state.1 + 1;
            let delay = chrono::Duration::seconds((5_i64 * (1_i64 << attempts.min(20))).min(900));
            sqlx::query("update porchlight.member_load_jobs set attempts=$3,retry_at=$4,last_error=$5 where member_id=$1 and requested_gen=$2").bind(member).bind(state.0).bind(attempts).bind(now+delay).bind(category).execute(&mut *tx).await?;
            tracing::warn!(%member,category,attempts,"load recomputation deferred");
        }
    }
    tx.commit().await?;
    Ok(processed)
}
/// Independent member transactions keep scheduler election out of domain lock order.
pub async fn nightly(pool: &PgPool, now: DateTime<Utc>) -> anyhow::Result<()> {
    let members:Vec<(Uuid,Uuid,Uuid,String)>=sqlx::query_as("select id,household_id,user_id,role from porchlight.household_members where deleted_at is null and role in ('user','super_admin') order by id").fetch_all(pool).await?;
    let mut failures = 0;
    for (member, household, user, role) in members {
        let c = Caller::Member {
            member_id: member,
            household_id: household,
            user_id: user,
            role,
            device_id: Uuid::nil(),
            session_id: Uuid::nil(),
            client_addr: None,
            request_id: Uuid::now_v7(),
        };
        let mut tx = match begin_as(pool, &Actor::system()).await {
            Ok(tx) => tx,
            Err(_) => {
                failures += 1;
                tracing::warn!(%member,category="connection","nightly planning member deferred");
                continue;
            }
        };
        let result=async {
            let scope=WriteScope::begin_at(&mut tx,household,ScopeRequest{members:vec![member],..Default::default()},now).await.map_err(items::domain_error)?;
            scope.finish(now).await.map_err(items::domain_error)?;
            let allowed=agenda_tasks(&mut tx,household,member).await?;
            let (run,_,_)=db::suggest(&mut tx,household,member,&allowed,now).await.map_err(items::domain_error)?;
            // Candidate notifications inherit item ACLs of each recipient.
            let dependent_items:Vec<Uuid>=sqlx::query_scalar("select i.id from porchlight.items i join porchlight.household_members m on m.id=i.subject_member_id where i.household_id=$1 and i.deleted_at is null and i.kind='appointment' and m.role='dependent' and m.deleted_at is null and not exists(select 1 from porchlight.item_assignees a where a.item_id=i.id) order by i.id").bind(household).fetch_all(&mut *tx).await?;
            let mut notifications=Vec::new();
            for id in dependent_items {
                match candidates(&mut tx,&c,id,now).await {
                    Ok(response) if !response.pending_derivation && response.candidates.iter().any(|candidate| candidate.member_id==member && candidate.reason==core::CandidateReason::FreeInsideAvailability) => {
                        if let Some(occurrence)=response.occurrence_id {notifications.push(db::SuggestionNotification{household,member,run,item:Some(id),occurrence:Some(occurrence)});}
                    },
                    Ok(_)=>{},Err(e) if e.status()==axum::http::StatusCode::NOT_FOUND=>{},Err(e)=>return Err(e)
                }
            }
            db::emit_suggestion_notifications(&mut tx,&notifications).await.map_err(items::domain_error)?;
            Ok::<_,ApiError>(())
        }.await;
        match result {
            Ok(()) => {
                if tx.commit().await.is_err() {
                    failures += 1;
                    tracing::warn!(%member,category="commit","nightly planning member deferred");
                }
            }
            Err(error) => {
                if tx.rollback().await.is_err() {
                    tracing::warn!(%member,category="rollback","nightly planning rollback failed");
                }
                failures += 1;
                tracing::warn!(%member,category=?error.problem(),"nightly planning member deferred");
            }
        }
    }
    if failures > 0 {
        anyhow::bail!("nightly planning failed for {failures} members");
    }
    Ok(())
}
/// Snapshot extensions use the caller's existing read-only RR transaction, never a new pool read.
/// Member-private for sessions (including super admins); agents see the household.
pub struct PlanningSnapshot {
    pub availability: Vec<Value>,
    pub suggestions: Vec<Value>,
}
#[derive(sqlx::FromRow)]
struct SnapshotSuggestion {
    #[sqlx(flatten)]
    row: db::Suggestion,
    occurrence_version: Option<i32>,
}
pub async fn snapshot_contribution(
    conn: &mut PgConnection,
    c: &Caller,
) -> Result<PlanningSnapshot, ApiError> {
    let availability:Vec<sqlx::types::Json<Value>>=sqlx::query_scalar("select jsonb_build_object('member_id',m.id,'version',coalesce(p.version,0),'windows',coalesce((select jsonb_agg(jsonb_build_object('weekday',w.weekday,'start',w.starts_local,'end',w.ends_local) order by w.weekday,w.starts_local,w.ends_local) from porchlight.availability_windows w where w.member_id=m.id),'[]'::jsonb),'conflicts','[]'::jsonb) from porchlight.household_members m left join porchlight.availability_profiles p on p.member_id=m.id where m.household_id=$1 and m.deleted_at is null and ($3 or m.id=$2) order by m.id").bind(c.household_id()).bind(c.member_id()).bind(c.sees_all()).fetch_all(&mut *conn).await?;
    let sql = format!(
        "select s.id,s.household_id,s.member_id,s.item_id,s.occurrence_id,s.proposed_start,s.proposed_end,s.status,s.run_id,s.version,s.created_at,o.version as occurrence_version from porchlight.schedule_suggestions s join porchlight.items i on i.id=s.item_id left join porchlight.item_occurrences o on o.id=s.occurrence_id and o.item_id=s.item_id where {} and ($3 or s.member_id=$2) order by s.id",
        crate::routes::reads::VISIBLE
    );
    let suggestions: Vec<SnapshotSuggestion> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(c.household_id())
        .bind(c.member_id())
        .bind(c.sees_all())
        .fetch_all(conn)
        .await?;
    Ok(PlanningSnapshot {
        availability: availability.into_iter().map(|v| v.0).collect(),
        suggestions: suggestions
            .into_iter()
            .map(|v| {
                serde_json::to_value(suggestion_response(v.row, v.occurrence_version, vec![]))
                    .map_err(|_| ApiError::internal("suggestion"))
            })
            .collect::<Result<_, _>>()?,
    })
}
