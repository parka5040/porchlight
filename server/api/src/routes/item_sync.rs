//! Authoritative item-scoped pages in one repeatable-read view per response.
use super::{read_models::*, reads};
use crate::{
    error::{ApiError, Problem},
    extract::Caller,
    state::AppState,
};
use axum::{
    extract::{Path, RawQuery, State},
    response::Response,
};
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Default, Serialize, utoipa::ToSchema)]
pub struct LocalWindow {
    /// Membership zone; UTC from/to remain authoritative at DST folds.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub zone: Option<String>,
    /// Exact projection of from into zone, without an offset.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false, format = "local-date-time")]
    pub local_from: Option<NaiveDateTime>,
    /// Exact projection of to into zone, without an offset.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false, format = "local-date-time")]
    pub local_to: Option<NaiveDateTime>,
}
impl LocalWindow {
    fn new(zone: chrono_tz::Tz, from: DateTime<Utc>, to: DateTime<Utc>) -> Self {
        Self {
            zone: Some(zone.name().into()),
            local_from: Some(from.with_timezone(&zone).naive_local()),
            local_to: Some(to.with_timezone(&zone).naive_local()),
        }
    }
}
#[derive(Clone, Serialize, utoipa::ToSchema)]
pub struct SyncWindow {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    #[serde(flatten)]
    pub local: LocalWindow,
}
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize, utoipa::ToSchema)]
pub struct AgendaWindow {
    pub member_id: Uuid,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}
#[derive(Serialize, utoipa::ToSchema)]
pub struct AgendaCoverage {
    #[serde(flatten)]
    pub window: AgendaWindow,
    #[serde(flatten)]
    pub local: LocalWindow,
    /// Exclusive start of this page; null on the first page.
    pub cursor: Option<String>,
    pub next_cursor: Option<String>,
    pub complete: bool,
}
#[derive(Serialize, utoipa::ToSchema)]
pub struct SyncCoverage {
    pub item_id: Uuid,
    /// Continuations cover only their explicitly requested agenda/comments sections.
    pub continued: bool,
    pub item_sections: bool,
    pub occurrences: SyncWindow,
    pub undated_occurrences: bool,
    pub reminder_member_id: Option<Uuid>,
    pub scheduled_reminders: SyncWindow,
    pub board_scope: String,
    pub agenda: Vec<AgendaCoverage>,
    pub suggestions: bool,
    /// Attachments/lists replacement coverage (initial collaboration request only).
    pub collaboration: bool,
    pub comments: bool,
    pub comments_cursor: Option<String>,
    pub comments_complete: bool,
}
#[derive(utoipa::ToSchema)]
pub struct SyncAgenda {
    pub member_id: Uuid,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    #[serde(flatten)]
    pub local: LocalWindow,
    pub items: Vec<ItemRead>,
}
#[derive(utoipa::ToSchema)]
pub struct ItemSyncRead {
    pub window_from: DateTime<Utc>,
    pub window_to: DateTime<Utc>,
    pub coverage: SyncCoverage,
    pub items: Option<Vec<ItemRead>>,
    pub occurrences: Option<Vec<OccurrenceRead>>,
    pub scheduled_reminders: Option<Vec<ScheduledReminderRead>>,
    pub board: Option<Vec<ItemRead>>,
    pub agenda: Option<Vec<SyncAgenda>>,
    pub suggestions: Option<Vec<super::planning::SuggestionResponse>>,
    pub comments: Option<Vec<crate::collaboration::Comment>>,
    pub comments_next_cursor: Option<String>,
    pub attachments: Option<Vec<crate::blobs::Attachment>>,
    pub lists: Option<Vec<crate::list_types::ListResponse>>,
}

type AgendaKey = (DateTime<Utc>, Uuid);
struct AgendaRequest {
    window: AgendaWindow,
    after: Option<AgendaKey>,
    cursor: Option<String>,
}
struct Parameters {
    collaboration: bool,
    agenda: Vec<AgendaRequest>,
    comments_cursor: Option<String>,
    continued: bool,
}
fn agenda_context(caller: &Caller, id: Uuid) -> String {
    format!(
        "item-sync-agenda:{}:{}:{}:{id}",
        caller.household_id(),
        caller.member_id(),
        caller.sees_all()
    )
}
fn parameters(
    raw: Option<&str>,
    state: &AppState,
    caller: &Caller,
    id: Uuid,
) -> Result<Parameters, ApiError> {
    let invalid = || ApiError::new(Problem::InvalidRequest);
    let raw = raw.unwrap_or("");
    if raw.len() > 8192 {
        return Err(invalid());
    }
    let mut p = Parameters {
        collaboration: false,
        agenda: Vec::new(),
        comments_cursor: None,
        continued: false,
    };
    let mut windows = 0;
    let mut initial = false;
    for (name, value) in url::form_urlencoded::parse(raw.as_bytes()) {
        match name.as_ref() {
            "include" if value == "collaboration" && !p.collaboration => {
                p.collaboration = true;
                initial = true;
            }
            "agenda" if windows < 8 => {
                windows += 1;
                initial = true;
                let (member, dates) = value.split_once(':').ok_or_else(invalid)?;
                let member_id = member.parse().map_err(|_| invalid())?;
                let (from, to) = dates
                    .match_indices(':')
                    .find_map(|(i, _)| {
                        Some((
                            dates[..i].parse::<DateTime<Utc>>().ok()?,
                            dates[i + 1..].parse::<DateTime<Utc>>().ok()?,
                        ))
                    })
                    .ok_or_else(invalid)?;
                if from >= to || to - from > Duration::days(730) {
                    return Err(invalid());
                }
                let window = AgendaWindow {
                    member_id,
                    from,
                    to,
                };
                if !p.agenda.iter().any(|r| r.window == window) {
                    p.agenda.push(AgendaRequest {
                        window,
                        after: None,
                        cursor: None,
                    });
                }
            }
            "agenda_cursor" if windows < 8 => {
                windows += 1;
                p.continued = true;
                let (window, after): (AgendaWindow, Option<AgendaKey>) = state
                    .idem
                    .cursor
                    .decode(&agenda_context(caller, id), &value)?;
                if p.agenda.iter().any(|r| r.window == window) {
                    return Err(invalid());
                }
                p.agenda.push(AgendaRequest {
                    window,
                    after,
                    cursor: Some(value.into_owned()),
                });
            }
            "comments_cursor" if p.comments_cursor.is_none() => {
                p.comments_cursor = Some(value.into_owned());
                p.continued = true;
            }
            _ => return Err(invalid()),
        }
    }
    if p.continued && initial {
        return Err(invalid());
    }
    Ok(p)
}

#[utoipa::path(get,path="/v1/items/{id}/sync",params(
    ("id"=Uuid,Path),
    ("agenda"=Option<Vec<String>>,Query,description="Repeat up to 8 member:from:to RFC3339 windows (deduplicated), each at most 730 days"),
    ("agenda_cursor"=Option<Vec<String>>,Query,description="Repeat incomplete coverage.agenda next_cursor values; continuation-only request"),
    ("comments_cursor"=Option<String>,Query,description="comments_next_cursor; continuation-only request"),
    ("include"=Option<String>,Query,description="collaboration adds comments (at most 200 per response), attachments and lists")
),responses((status=200,description="Item-scoped page, at most 200 agenda rows total, 200 comments and 8 MiB identity bytes; exhaust every continuation before omission removal",body=ItemSyncRead),(status=422,description="item-sync-too-large: canonical nonpaged sections exceed the 8 MiB document budget; no replacement coverage",body=crate::openapi::ProblemBody,content_type="application/problem+json"),crate::openapi::UniversalProblems,crate::openapi::NotFoundProblem,crate::openapi::RateLimitProblem))]
pub async fn get(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let p = parameters(raw.as_deref(), &state, &caller, id)?;
    let permit = state
        .item_sync_limits
        .acquire(caller.admission_id(), caller.member_id())?;
    let now = (state.item_clock)();
    let (from, to) = reads::snapshot_window(now)?;
    let mut tx = crate::items::read_snapshot(&state).await?;
    crate::collaboration::read_item(&mut tx, &caller, id).await?;
    // Same RR view and zone parser as snapshot refinement; never the device zone.
    let zone: String = sqlx::query_scalar("select tzid from porchlight.households where id=$1")
        .bind(caller.household_id())
        .fetch_one(&mut *tx)
        .await?;
    let zone = porchlight_db::derive::parse_zone(&zone)
        .map_err(|_| ApiError::internal("snapshot_zone"))?;
    let mut document = crate::json_document::Document::bounded(crate::json_document::SYNC_BUDGET);
    if !p.continued {
        reads::write_item_sections(&mut tx, &caller, &mut document, Some(id), from, to).await?;
        document.field(
            "board",
            &reads::board_rows_scoped(
                &mut tx,
                &caller,
                "household",
                now,
                None,
                i32::MAX as usize,
                Some(id),
            )
            .await?,
        )?;
        crate::planning::write_suggestions(&mut tx, &caller, &mut document, Some(id)).await?;
        if p.collaboration {
            document.field(
                "attachments",
                &crate::blobs::snapshot_attachments_s4b(&mut tx, &caller, &[id]).await?,
            )?;
            document.field(
                "lists",
                &crate::lists::snapshot_scoped(&mut tx, &caller, Some(id)).await?,
            )?;
        } else {
            document.array("attachments")?.finish();
            document.array("lists")?.finish();
        }
    }
    document.field("window_from", &from)?;
    document.field("window_to", &to)?;
    let comments = p.collaboration || p.comments_cursor.is_some();
    let context = super::collaboration::comment_context(&caller, id);
    let mut after = p
        .comments_cursor
        .as_deref()
        .map(|c| state.idem.cursor.decode::<Uuid>(&context, c))
        .transpose()?;
    let mut next_cursor = None;
    if comments || !p.continued {
        let mut array = document.array("comments")?;
        if comments {
            let mut count = 0;
            loop {
                let mut rows =
                    super::collaboration::comment_rows(&mut tx, id, after, 1, None).await?;
                let Some(row) = rows.pop() else {
                    break;
                };
                if count == 200 || !array.try_row(&row, crate::json_document::SYNC_BUDGET)? {
                    next_cursor = Some(
                        state
                            .idem
                            .cursor
                            .encode(&context, &after.unwrap_or(Uuid::nil()))?,
                    );
                    break;
                }
                after = Some(row.id);
                count += 1;
            }
        }
        array.finish();
    }
    let mut coverage = Vec::new();
    if !p.continued || !p.agenda.is_empty() {
        let mut agenda = document.array("agenda")?;
        let mut count = 0;
        for request in p.agenda {
            let w = &request.window;
            let agenda_zone: Option<String> = sqlx::query_scalar("select porchlight.pl_effective_member_zone(m.role,m.tzid,h.tzid) from porchlight.household_members m join porchlight.households h on h.id=m.household_id where m.id=$1 and m.household_id=$2")
                .bind(w.member_id)
                .bind(caller.household_id())
                .fetch_optional(&mut *tx)
                .await?;
            let local = agenda_zone
                .map(|z| porchlight_db::derive::parse_zone(&z))
                .transpose()
                .map_err(|_| ApiError::internal("agenda_zone"))?
                .map(|z| LocalWindow::new(z, w.from, w.to))
                .unwrap_or_default();
            let mut object = agenda.object();
            object.field("member_id", &w.member_id)?;
            object.field("from", &w.from)?;
            object.field("to", &w.to)?;
            if local.zone.is_some() {
                object.field("zone", &local.zone)?;
                object.field("local_from", &local.local_from)?;
                object.field("local_to", &local.local_to)?;
            }
            let mut rows = object.array("items")?;
            let mut after = request.after;
            let mut next = None;
            loop {
                // A one-row SQL page bounds decoding even when notes/conflicts are large.
                let mut page = reads::agenda_rows(
                    &mut tx,
                    &caller,
                    w.member_id,
                    w.from,
                    w.to,
                    after,
                    0,
                    Some(id),
                )
                .await?;
                let Some((key, row)) = page.pop() else {
                    break;
                };
                if count == 200 || !rows.try_row(&row, crate::json_document::SYNC_BUDGET)? {
                    next = Some(
                        state
                            .idem
                            .cursor
                            .encode(&agenda_context(&caller, id), &(w, after))?,
                    );
                    break;
                }
                after = Some(key);
                count += 1;
            }
            rows.finish();
            object.finish();
            coverage.push(AgendaCoverage {
                window: request.window,
                local,
                cursor: request.cursor,
                complete: next.is_none(),
                next_cursor: next,
            });
        }
        agenda.finish();
    }
    document.field(
        "coverage",
        &SyncCoverage {
            item_id: id,
            continued: p.continued,
            item_sections: !p.continued,
            occurrences: SyncWindow {
                from,
                to,
                local: LocalWindow::new(zone, from, to),
            },
            undated_occurrences: !p.continued,
            reminder_member_id: (!caller.sees_all()).then(|| caller.member_id()),
            scheduled_reminders: SyncWindow {
                from,
                to,
                local: LocalWindow::default(),
            },
            board_scope: "household".into(),
            agenda: coverage,
            suggestions: !p.continued,
            collaboration: p.collaboration,
            comments,
            comments_cursor: p.comments_cursor,
            comments_complete: comments && next_cursor.is_none(),
        },
    )?;
    document.field("comments_next_cursor", &next_cursor)?;
    if document.identity_len() + 1 > crate::json_document::SYNC_BUDGET {
        return Err(ApiError::new(crate::error::Problem::ItemSyncTooLarge));
    }
    tx.commit().await?;
    Ok(crate::concurrency::with_body(document.response()?, permit))
}
#[derive(utoipa::OpenApi)]
#[openapi(paths(get))]
pub struct ItemSyncApi;
pub const ENDPOINTS: &[crate::permissions::Endpoint] = &[crate::permissions::Endpoint {
    method: "GET",
    path: "/v1/items/{id}/sync",
    handler: "get",
    case: "",
    policy: crate::permissions::Policy::Item(porchlight_core::access::Capability::View),
}];
