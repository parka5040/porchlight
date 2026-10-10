//! Explicit data-only exports: never credentials, sessions, replay responses or PKI.
use crate::{
    error::{ApiError, Problem},
    extract::{ApiQuery, Caller},
    state::AppState,
};
use axum::{
    Json,
    body::Body,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use porchlight_core::ics::{EventOrigin, EventStatus, IcsTime, ImportedEvent, TimeMode};
use serde_json::{Value, json};
use sqlx::{PgConnection, types::Json as SqlJson};
mod archive;
use uuid::Uuid;

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub struct MemberParams {
    pub member_id: Option<Uuid>,
}
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct MemberExport {
    pub format_version: u32,
    pub exported_at: DateTime<Utc>,
    pub member_id: Uuid,
    pub tables: Value,
    /// Save as appointments.ics. Materialized appointments, stable occurrence UID,
    /// all retained dates (no snapshot window); floating local times have no TZID.
    pub appointments_ics: String,
}
async fn table(
    c: &mut PgConnection,
    h: Uuid,
    m: Uuid,
    name: &str,
    condition: &str,
) -> Result<Vec<Value>, ApiError> {
    let sql = format!(
        "select to_jsonb(t)-'search_document'-'during' from porchlight.{name} t where $1::uuid is not null and $2::uuid is not null and not $3 and ({condition}) order by to_jsonb(t)::text"
    );
    let rows: Vec<SqlJson<Value>> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(h)
        .bind(m)
        .bind(false)
        .fetch_all(c)
        .await?;
    Ok(rows.into_iter().map(|v| v.0).collect())
}
async fn calendar(
    c: &mut PgConnection,
    items: &[Value],
    occurrences: &[Value],
    now: DateTime<Utc>,
) -> Result<String, ApiError> {
    // Batch the authoritative typed timing inputs (including interval projection),
    // then index them: export must use the same override resolver as agenda reads.
    let ids: Vec<Uuid> = items
        .iter()
        .filter(|i| i["kind"] == "appointment")
        .filter_map(|i| i["id"].as_str()?.parse().ok())
        .collect();
    let specs: std::collections::HashMap<_, _> = porchlight_db::derive::load_items(c, &ids)
        .await
        .map_err(|_| ApiError::internal("export_timing"))?
        .into_iter()
        .map(|i| Ok((i.id, i.spec()?)))
        .collect::<Result<_, porchlight_db::derive::Error>>()
        .map_err(|_| ApiError::internal("export_timing"))?;
    let index: std::collections::HashMap<_, _> = items
        .iter()
        .filter_map(|i| Some((i["id"].as_str()?.parse::<Uuid>().ok()?, i)))
        .collect();
    let mut events = Vec::new();
    for o in occurrences {
        let row: porchlight_db::derive::OccurrenceRow = serde_json::from_value(o.clone())
            .map_err(|_| ApiError::internal("export_occurrence"))?;
        let Some(spec) = specs.get(&row.item_id) else {
            continue;
        };
        let i = index[&row.item_id];
        let anchor = porchlight_db::derive::effective_anchor(&row, spec)
            .map_err(|_| ApiError::internal("export_timing"))?;
        let all_day = i["all_day"] == true;
        let floating = i["time_mode"] == "floating";
        let start = if all_day {
            anchor
                .local_start
                .map(|t| IcsTime::Date(t.date()))
                .or_else(|| anchor.due_date.map(IcsTime::Date))
        } else if floating {
            anchor.local_start.map(IcsTime::Floating)
        } else {
            anchor.starts_at.map(IcsTime::Fixed)
        };
        let Some(start) = start else { continue };
        let end = if all_day {
            anchor
                .local_end
                .map(|t| IcsTime::Date(t.date()))
                .or_else(|| match &start {
                    IcsTime::Date(d) => d.succ_opt().map(IcsTime::Date),
                    _ => None,
                })
        } else if floating {
            anchor.local_end.map(IcsTime::Floating)
        } else {
            anchor.ends_at.map(IcsTime::Fixed)
        };
        let Some(end) = end else { continue };
        events.push(ImportedEvent {
            uid: format!("{}@porchlight", row.id),
            recurrence_id: None,
            origin: EventOrigin::Single,
            summary: i["title"].as_str().unwrap_or("").into(),
            description: i["notes"].as_str().unwrap_or("").into(),
            location: String::new(),
            start,
            end,
            all_day,
            time_mode: if floating {
                TimeMode::Floating
            } else {
                TimeMode::Fixed
            },
            tzid: i["tzid"].as_str().map(str::to_owned),
            rrule: None,
            exdates: Vec::new(),
            sequence: row.version as u32,
            status: if o["status"] == "skipped" {
                EventStatus::Cancelled
            } else {
                EventStatus::Confirmed
            },
        });
    }
    Ok(porchlight_core::ics::write(
        &events,
        "Porchlight appointments",
        now,
    ))
}
pub async fn member_data(
    c: &mut PgConnection,
    caller: &Caller,
    now: DateTime<Utc>,
) -> Result<MemberExport, ApiError> {
    let h = caller.household_id();
    let m = caller.member_id();
    let visible = super::reads::VISIBLE;
    let item_ids = format!("select i.id from porchlight.items i where {visible}");
    let mut tables = serde_json::Map::new();
    let items = table(c, h, m, "items", &format!("t.id in ({item_ids})")).await?;
    let occurrences = table(
        c,
        h,
        m,
        "item_occurrences",
        &format!("t.item_id in ({item_ids})"),
    )
    .await?;
    let ics = calendar(c, &items, &occurrences, now).await?;
    tables.insert("items".into(), json!(items));
    tables.insert("item_occurrences".into(), json!(occurrences));
    for name in ["item_assignees", "item_viewers", "item_reminders"] {
        tables.insert(
            name.into(),
            json!(table(c, h, m, name, &format!("t.item_id in ({item_ids})")).await?),
        );
    }
    for name in ["comments", "attachments"] {
        tables.insert(
            name.into(),
            json!(
                table(
                    c,
                    h,
                    m,
                    name,
                    &format!("t.deleted_at is null and t.item_id in ({item_ids})")
                )
                .await?
            ),
        );
    }
    let list_ids = "select l.id from porchlight.lists l where l.household_id=$1 and l.deleted_at is null and porchlight.pl_list_can_read(l.id,$2)";
    tables.insert(
        "lists".into(),
        json!(table(c, h, m, "lists", &format!("t.id in ({list_ids})")).await?),
    );
    for name in ["list_viewers", "list_entries"] {
        let live = if name == "list_entries" {
            " and t.archived_at is null"
        } else {
            ""
        };
        tables.insert(
            name.into(),
            json!(table(c, h, m, name, &format!("t.list_id in ({list_ids}){live}")).await?),
        );
    }
    for name in ["list_entry_checks"] {
        tables.insert(name.into(),json!(table(c,h,m,name,&format!("t.entry_id in (select e.id from porchlight.list_entries e where e.archived_at is null and e.list_id in ({list_ids}))")).await?));
    }
    // Recipient contributions reauthorize EVERY notification source and recompute counts.
    let (notifications, prefs) = super::notifications::snapshot_fields(c, caller).await?;
    tables.insert("notifications".into(), json!(notifications));
    tables.insert("notification_prefs".into(), json!(prefs));
    for name in [
        "member_settings",
        "availability_windows",
        "availability_profiles",
    ] {
        tables.insert(
            name.into(),
            json!(
                table(
                    c,
                    h,
                    m,
                    name,
                    "t.household_id=$1 and t.member_id=$2 and not $3"
                )
                .await?
            ),
        );
    }
    tables.insert(
        "schedule_suggestions".into(),
        json!(
            table(
                c,
                h,
                m,
                "schedule_suggestions",
                &format!("t.member_id=$2 and t.item_id in ({item_ids})")
            )
            .await?
        ),
    );
    for name in ["scheduled_reminders", "member_agenda"] {
        tables.insert(
            name.into(),
            json!(
                table(
                    c,
                    h,
                    m,
                    name,
                    &format!("t.member_id=$2 and t.item_id in ({item_ids})")
                )
                .await?
            ),
        );
    }
    tables.insert(
        "member_daily_load".into(),
        json!(table(c, h, m, "member_daily_load", "t.member_id=$2").await?),
    );
    tables.insert(
        "households".into(),
        json!(table(c, h, m, "households", "t.id=$1").await?),
    );
    tables.insert(
        "ics_sources".into(),
        json!(crate::calendar::snapshot_sources(c, caller).await?),
    );
    tables.insert(
        "household_members".into(),
        json!(super::members::all(c, h, m).await?),
    );
    Ok(MemberExport {
        format_version: 1,
        exported_at: now,
        member_id: m,
        tables: Value::Object(tables),
        appointments_ics: ics,
    })
}
#[utoipa::path(get,path="/v1/export/member",params(MemberParams),responses((status=200,description="Data-only JSON envelope plus appointments_ics file content, authoritative member ACL even for admin PAT target",body=MemberExport),(status=403,description="Cannot export another member without admin PAT"),crate::openapi::UniversalProblems))]
pub async fn member(
    State(s): State<AppState>,
    caller: Caller,
    ApiQuery(p): ApiQuery<MemberParams>,
) -> Result<Json<MemberExport>, ApiError> {
    let target = p.member_id.unwrap_or(caller.member_id());
    if target != caller.member_id() && !caller.sees_all() {
        return Err(ApiError::new(Problem::Forbidden));
    }
    if caller.sees_all() {
        caller.require_admin()?;
    }
    let mut tx = porchlight_db::tx::begin(&s.pool).await?;
    sqlx::query("set transaction isolation level repeatable read read only")
        .execute(&mut *tx)
        .await?;
    let row:Option<(Option<Uuid>,String)>=sqlx::query_as("select user_id,role from porchlight.household_members where household_id=$1 and id=$2 and deleted_at is null").bind(caller.household_id()).bind(target).fetch_optional(&mut *tx).await?;
    let (user, role) = row.ok_or_else(|| ApiError::new(Problem::NotFound))?;
    // Synthetic member domain caller deliberately NEVER gets agent ACL bypass.
    let member = Caller::Member {
        member_id: target,
        user_id: user.unwrap_or(Uuid::nil()),
        role,
        device_id: Uuid::nil(),
        session_id: Uuid::nil(),
        household_id: caller.household_id(),
        client_addr: None,
        request_id: Uuid::now_v7(),
    };
    let data = member_data(&mut tx, &member, (s.item_clock)()).await?;
    tx.commit().await?;
    Ok(Json(data))
}
// Fail-closed allowlist: additions require an export decision, never introspect and
// accidentally expose a future credential table. Standard encrypted backups dump all.
const HOUSEHOLD_TABLES: &[&str] = &[
    "households",
    "users",
    "household_members",
    "devices",
    "device_certificates",
    "member_settings",
    "items",
    "item_assignees",
    "item_viewers",
    "item_reminders",
    "item_occurrences",
    "member_agenda",
    "scheduled_reminders",
    "item_derivation",
    "item_revisions",
    "task_events",
    "purge_log",
    "notification_kinds",
    "notification_events",
    "notification_prefs",
    "notifications",
    "notification_sources",
    "notification_event_delivery",
    "notification_delivery_retry",
    "lists",
    "list_viewers",
    "list_entries",
    "list_entry_checks",
    "list_revisions",
    "list_entry_revisions",
    "comments",
    "attachments",
    "blobs",
    "collaboration_audit",
    "ics_sources",
    "ics_source_revisions",
    "ics_uid_map",
    "availability_windows",
    "availability_profiles",
    "schedule_suggestions",
    "planning_audit",
    "member_daily_load",
    "member_load_jobs",
    "scheduler_jobs",
    "outbox",
    "backup_runs",
];
async fn step_up(s: &AppState, c: &Caller, headers: &HeaderMap) -> Result<(), ApiError> {
    c.require_admin()?;
    if let Caller::Member {
        session_id,
        device_id,
        ..
    } = c
    {
        use porchlight_core::auth::token::{TokenKind, hash_token, looks_like};
        let token = headers
            .get("porchlight-step-up")
            .and_then(|v| v.to_str().ok())
            .filter(|t| looks_like(TokenKind::StepUp, t))
            .ok_or_else(|| ApiError::new(Problem::StepUpRequired))?;
        let mut tx = porchlight_db::tx::begin_as(&s.pool, &c.actor()).await?;
        let valid = porchlight_db::auth::consume_step_up(
            &mut tx,
            &hash_token(token),
            *session_id,
            *device_id,
            "household.export",
        )
        .await?;
        if !valid {
            return Err(ApiError::new(Problem::StepUpRequired));
        }
        tx.commit().await?;
    }
    Ok(())
}
#[utoipa::path(get,path="/v1/admin/export/household",params(crate::openapi::StepUpHeader),responses((status=200,description="Secret-free streamed tar.gz: v2 data.json manifest, tables/*.jsonl, live referenced attachment/thumbnail blobs and LAST complete.json inventory. Missing completion entry or invalid gzip trailer means incomplete. Admin PAT or super-admin app with household.export single-use step-up",body=String,content_type="application/gzip"),(status=403,description="Admin scope or step-up required"),crate::openapi::UniversalProblems))]
pub async fn household(
    State(s): State<AppState>,
    c: Caller,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    step_up(&s, &c, &headers).await?;
    let (send, receive) = tokio::sync::mpsc::channel(2);
    let household = c.household_id();
    let runtime = tokio::runtime::Handle::current();
    tokio::spawn(async move {
        let errors = send.clone();
        let task =
            tokio::task::spawn_blocking(move || archive::produce(s, household, send, runtime));
        if !matches!(task.await, Ok(Ok(()))) {
            let _ = errors
                .send(Err(std::io::Error::other("export interrupted")))
                .await;
        }
    });
    // No table read or descriptor inventory before headers. Dropping the body
    // closes the channel, cancels preparation, and drops the worker's lease guard.
    Ok((
        [
            ("content-type", "application/gzip"),
            (
                "content-disposition",
                "attachment; filename=household.tar.gz",
            ),
        ],
        Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(receive)),
    )
        .into_response())
}
#[derive(utoipa::OpenApi)]
#[openapi(paths(member, household), components(schemas(MemberExport)))]
pub struct ExportsApi;

fn member_policy(c: &crate::permissions::PolicyContext) -> u16 {
    use porchlight_core::access::CallerKind;
    if matches!(c.item.caller, CallerKind::Agent) {
        if c.pat_scope == Some(crate::permissions::PatScope::Admin) {
            200
        } else {
            403
        }
    } else if c.is_self {
        200
    } else {
        403
    }
}
fn household_policy(c: &crate::permissions::PolicyContext) -> u16 {
    use porchlight_core::access::{CallerKind, Role};
    if c.item.caller == CallerKind::AppMember(Role::SuperAdmin)
        || c.pat_scope == Some(crate::permissions::PatScope::Admin)
    {
        200
    } else {
        403
    }
}
pub const ENDPOINTS: &[crate::permissions::Endpoint] = &[
    crate::permissions::Endpoint {
        method: "GET",
        path: "/v1/export/member",
        handler: "member",
        policy: crate::permissions::Policy::Custom {
            label: "Own member ACL; admin PAT can select a member (no bypass)",
            expected: member_policy,
        },
        case: "",
    },
    crate::permissions::Endpoint {
        method: "GET",
        path: "/v1/admin/export/household",
        handler: "household",
        policy: crate::permissions::Policy::Custom {
            label: "Admin PAT or super-admin app plus household.export step-up",
            expected: household_policy,
        },
        case: "",
    },
];
