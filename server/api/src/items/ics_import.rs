//! S8a adapter: existing ACL and validation helpers, one retained WriteScope per calendar.
use super::{
    acl::{self, caps, domain_error, load},
    validate::{validate_fields, validate_members, validation},
};
use crate::calendar::{self, ImportRequest, ImportSummary, Source, Warning};
use crate::{
    error::{ApiError, Problem},
    extract::Caller,
    idempotency::IdemTx,
};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use porchlight_core::ics::{
    self, EventOrigin, EventStatus, IcsTime, ImportedEvent, ParseOptions, WarningCode,
};
use porchlight_core::{access::Capability, items as core};
use porchlight_db::items::{self as db, ItemWrite, Relationships, ScopeRequest, WriteScope};
use sqlx::{Acquire, Postgres, Transaction, types::Json as SqlJson};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Clone, sqlx::FromRow)]
struct Mapping {
    uid: String,
    recurrence_key: String,
    item_id: Uuid,
    occurrence_id: Option<Uuid>,
    expanded_start: Option<DateTime<Utc>>,
    content_hash: Option<String>,
}

pub async fn import(
    c: Caller,
    tx: &mut IdemTx,
    body: ImportRequest,
    now: DateTime<Utc>,
) -> Result<ImportSummary, ApiError> {
    calendar::validate_name(&body.name)?;
    calendar::validate_scope(&body.scope)?;
    if let Some(v) = &body.visibility {
        calendar::validate_visibility(v)?;
    }
    if body.content.len() > calendar::fetch::MAX_BYTES {
        return Err(ApiError::new(Problem::ValidationFailed));
    }
    if body.member_id != c.member_id() {
        c.require_admin()?;
    }
    let mut domain = tx.tx.begin().await?;
    let result = async {
        calendar::target_zone(&mut domain,c.household_id(),Some(body.member_id)).await.map_err(|e| if e.status()==StatusCode::UNPROCESSABLE_ENTITY {calendar::member_reference("member_id")} else {e})?;
        let visibility = calendar::source_visibility(&mut domain,c.household_id(),Some(body.member_id),body.visibility.as_deref()).await?;
        sqlx::query("insert into porchlight.ics_sources(household_id,owner_member_id,target_member_id,kind,import_key,name,visibility) values($1,$2,$3,'import',$4,$5,$6) on conflict (household_id,owner_member_id,coalesce(target_member_id,'00000000-0000-0000-0000-000000000000'::uuid),import_key) where kind='import' do update set name=excluded.name,visibility=excluded.visibility where ics_sources.name is distinct from excluded.name or ics_sources.visibility is distinct from excluded.visibility")
            .bind(c.household_id()).bind(c.member_id()).bind(body.member_id).bind(&body.scope).bind(&body.name).bind(visibility).execute(&mut *domain).await?;
        let SqlJson(source) = sqlx::query_scalar::<_,SqlJson<Source>>("select to_jsonb(s) from porchlight.ics_sources s where household_id=$1 and owner_member_id=$2 and target_member_id=$3 and kind='import' and import_key=$4 for no key update")
            .bind(c.household_id()).bind(c.member_id()).bind(body.member_id).bind(&body.scope).fetch_one(&mut *domain).await?;
        apply_calendar(&mut domain,&c,&source,&body.content,false,now).await
    }.await;
    match result {
        Ok(mut summary) if body.preview => {
            domain.rollback().await?;
            summary.preview = true;
            summary.source_id = None;
            summary.item_ids.clear();
            summary.conflicts.clear();
            Ok(summary)
        }
        Ok(summary) => {
            domain.commit().await?;
            Ok(summary)
        }
        // R86: the deadline may have poisoned the IdemTx. Send nothing more on it (dropping the
        // savepoint only queues its rollback) and return the transient error unchanged, so the
        // mutation wrapper discards the whole transaction without sealing a response.
        Err(e) if e.is_calendar_apply_timeout() => {
            drop(domain);
            Err(e)
        }
        Err(e) => {
            domain.rollback().await?;
            Err(e)
        }
    }
}

struct CalendarInput {
    parsed: ics::ParsedCalendar,
    zone: chrono_tz::Tz,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    unsafe_global: bool,
    unsafe_uids: BTreeSet<String>,
}
// Shared by imports and feed refreshes. A cancelled request cannot stop blocking
// work, so the worker owns admission until it actually finishes (not its waiter).
static PARSE_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
async fn parse_calendar_blocking(
    content: &str,
    zone: chrono_tz::Tz,
    now: DateTime<Utc>,
) -> Result<CalendarInput, ApiError> {
    if content.len() > calendar::fetch::MAX_BYTES {
        return Err(ApiError::new(Problem::ValidationFailed));
    }
    let permit = PARSE_SLOTS
        .acquire()
        .await
        .map_err(|_| ApiError::internal("calendar_parse_admission"))?;
    let content = content.to_owned();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        parse_calendar(&content, zone, now)
    })
    .await
    .map_err(|_| ApiError::internal("calendar_parse_worker"))?
}

// Pure parse/normalization phase: no domain writes.
fn parse_calendar(
    content: &str,
    zone: chrono_tz::Tz,
    now: DateTime<Utc>,
) -> Result<CalendarInput, ApiError> {
    if content.len() > calendar::fetch::MAX_BYTES {
        return Err(ApiError::new(Problem::ValidationFailed));
    }
    let start = now - chrono::Duration::days(180);
    let end = core::horizon_end(now, zone).map_err(|e| validation(e.field, e.reason))?;
    let parsed = ics::parse(
        content,
        ParseOptions {
            fallback_tz: zone,
            horizon_start: start,
            horizon_end: end,
        },
    );
    if parsed
        .warnings
        .iter()
        .any(|w| w.code == WarningCode::MalformedCalendar)
    {
        return Err(ApiError::new(Problem::ValidationFailed));
    }
    // Semantic loss / a time budget limit never licenses removing old events.
    let unsafe_global = parsed.warnings.iter().any(|w| {
        w.code == WarningCode::ExpansionLimited
            || (w.uid.is_none() && w.code != WarningCode::ExpandedRule)
    });
    let unsafe_uids: BTreeSet<_> = parsed
        .warnings
        .iter()
        .filter(|w| w.code != WarningCode::ExpandedRule)
        .filter_map(|w| w.uid.clone())
        .collect();
    Ok(CalendarInput {
        parsed,
        zone,
        start,
        end,
        unsafe_global,
        unsafe_uids,
    })
}
struct CalendarPlan {
    input: CalendarInput,
    summary: ImportSummary,
    mappings: Vec<Mapping>,
    map: BTreeMap<(String, String), Mapping>,
    masters: BTreeSet<String>,
    exception_keys: BTreeSet<(String, String)>,
    hashes: BTreeMap<(String, String), String>,
    unchanged: BTreeSet<usize>,
    planned_versions: BTreeMap<Uuid, i32>,
    fields: BTreeMap<usize, ItemWrite>,
    sets: BTreeMap<usize, Relationships>,
    request: ScopeRequest,
    removed: Vec<Mapping>,
}
// Read/ACL planning phase. All item/member/closed-occurrence targets are known
// before acquiring the retained WriteScope; versions are rechecked in apply.
async fn plan_calendar(
    tx: &mut Transaction<'_, Postgres>,
    c: &Caller,
    source: &Source,
    mut input: CalendarInput,
    remove_missing: bool,
    now: DateTime<Utc>,
) -> Result<CalendarPlan, ApiError> {
    // Validate before hashing/planning, including retained-series overrides. A bad
    // event cannot poison a whole import/feed or authorize rewriting its old UID.
    let mut validation_warnings = Vec::new();
    let mut invalid_uids = BTreeSet::new();
    for event in &input.parsed.events {
        // Match PostgreSQL text / ics_uid_map identity constraints before any
        // writes, not just the item fields. NUL is also invalid in JSONB warnings.
        if event.uid.is_empty()
            || event.uid.len() > 1024
            || event.uid.contains('\0')
            || event_fields(event, source, input.zone).is_err()
        {
            invalid_uids.insert(event.uid.clone());
            validation_warnings.push(Warning {
                uid: Some(event.uid.replace('\0', "\u{fffd}")),
                code: "InvalidEvent".into(),
                message:
                    "Event violates identity, item text or timing validation; existing UID left untouched"
                        .into(),
            });
        }
    }
    input.unsafe_uids.extend(invalid_uids.iter().cloned());
    let before = input.parsed.events.len();
    // A UID owns both its series and its exceptions: protect them together when
    // any event in that UID is invalid (including representation changes).
    input
        .parsed
        .events
        .retain(|e| !invalid_uids.contains(&e.uid));
    let skipped = before - input.parsed.events.len() + input.parsed.invalid_skipped;
    // Input iteration alone exceeds admission; do not ACL-plan tens of thousands
    // of doomed writes before discovering this deterministic content limit.
    if input.parsed.events.len() > MAX_CALENDAR_WORK {
        return Err(calendar_work_error());
    }

    let CalendarInput {
        ref parsed,
        zone,
        start,
        end,
        unsafe_global,
        ref unsafe_uids,
    } = input;
    let safe_uid = |uid: &String| !unsafe_global && !unsafe_uids.contains(uid);
    let safe = !unsafe_global && unsafe_uids.is_empty();
    let mut summary = ImportSummary {
        source_id: Some(source.id),
        removal_suppressed: remove_missing && !safe,
        warnings: parsed
            .warnings
            .clone()
            .into_iter()
            .map(|w| Warning {
                uid: w.uid.map(|uid| uid.replace('\0', "\u{fffd}")),
                code: format!("{:?}", w.code),
                message: w.message.replace('\0', "\u{fffd}"),
            })
            .collect(),
        skipped: skipped as u32,
        invalid_skipped: skipped as u32,
        ..Default::default()
    };
    summary.warnings.extend(validation_warnings);
    let mappings: Vec<Mapping>=sqlx::query_as("select uid,recurrence_key,item_id,occurrence_id,expanded_start,content_hash from porchlight.ics_uid_map where source_id=$1 order by uid,recurrence_key").bind(source.id).fetch_all(&mut **tx).await?;
    let map: BTreeMap<_, _> = mappings
        .iter()
        .cloned()
        .map(|m| ((m.uid.clone(), m.recurrence_key.clone()), m))
        .collect();
    let masters: BTreeSet<String> = parsed
        .events
        .iter()
        .filter(|e| matches!(e.origin, EventOrigin::Series))
        .map(|e| e.uid.clone())
        .collect();
    let exception_keys: BTreeSet<_> = parsed
        .events
        .iter()
        .flat_map(|event| {
            let mut keys = Vec::new();
            if matches!(event.origin, EventOrigin::Series) {
                keys.extend(
                    event
                        .exdates
                        .iter()
                        .map(|t| (event.uid.clone(), key(Some(t)))),
                );
            } else if matches!(event.origin, EventOrigin::Override) && masters.contains(&event.uid)
            {
                keys.push((event.uid.clone(), key(event.recurrence_id.as_ref())));
            }
            keys
        })
        .collect();
    let hashes = normalized_hashes(&parsed.events, source, zone)?;
    let mut unchanged = BTreeSet::new();
    let mut restoring = BTreeSet::new();
    let mut planned_versions = BTreeMap::new();
    let mut fields = BTreeMap::new();
    let mut sets = BTreeMap::new();
    let mut request = ScopeRequest::default();
    let mut seen = BTreeSet::new();
    let owner = calendar::source_owner(tx, source).await?;
    for (index, event) in parsed.events.iter().enumerate() {
        seen.insert((event.uid.clone(), key(event.recurrence_id.as_ref())));
        if matches!(event.origin, EventOrigin::Override) && masters.contains(&event.uid) {
            let master = parsed
                .events
                .iter()
                .find(|e| e.uid == event.uid && matches!(e.origin, EventOrigin::Series))
                .expect("retained master");
            // V1 occurrences can override timing/status, not item-level details.
            // Report this loss on EVERY refresh, even when the hash is unchanged,
            // so an identical fetch or 304 cannot falsely claim a lossless import.
            if (
                event.summary.as_str(),
                event.description.as_str(),
                event.location.as_str(),
            ) != (
                master.summary.as_str(),
                master.description.as_str(),
                master.location.as_str(),
            ) && event.status != EventStatus::Cancelled
            {
                summary.warnings.push(Warning {
                    uid: Some(event.uid.clone()),
                    code: "UnsupportedOverrideDetails".into(),
                    message: "Retained occurrence uses the master title, notes and location; only exception timing/status is imported".into(),
                });
            }
            continue;
        }
        // An occurrence mapping is not a standalone item: a producer can switch its RRULE
        // between retained and expanded representations without rewriting the old master.
        let old = map
            .get(&(event.uid.clone(), key(event.recurrence_id.as_ref())))
            .filter(|m| m.occurrence_id.is_none());
        let mut write = if let Some(m) = old {
            let row = match load(tx, c, m.item_id, true).await {
                Ok(row) => row,
                Err(e) if e.status() == StatusCode::NOT_FOUND => {
                    summary.warnings.push(Warning {
                        uid: Some(event.uid.clone()),
                        code: "InvisibleItem".into(),
                        message: "An inaccessible mapped event was not changed".into(),
                    });
                    continue;
                }
                Err(e) => return Err(e),
            };
            // Re-import may restore a cancelled item, but never bypass its edit ACL.
            if !caps(tx, c, &row).await?.contains(&Capability::Edit) {
                return Err(ApiError::new(Problem::Forbidden));
            }
            planned_versions.insert(row.id, row.version);
            if row.deleted_at.is_some() && event.status != EventStatus::Cancelled {
                restoring.insert(index);
            }
            request.items.push(row.id);
            ItemWrite::from(&row)
        } else {
            ItemWrite {
                owner_id: owner,
                subject_member_id: source.target_member_id,
                visibility: source.visibility.clone(),
                ..Default::default()
            }
        };
        if source.kind == "feed" {
            write.owner_id = owner;
            write.subject_member_id = source.target_member_id;
            write.visibility = source.visibility.clone();
        }
        let event_write = event_fields(event, source, zone)?;
        write.title = event_write.title;
        write.notes = event_write.notes;
        write.address = event_write.address;
        write.source = event_write.source;
        write.timing = event_write.timing;
        if event.start == event.end && !event.all_day {
            summary.warnings.push(Warning {
                uid: Some(event.uid.clone()),
                code: "DurationNormalized".into(),
                message: "Zero-duration timed event imported as a one-minute busy block".into(),
            });
        }
        validate_fields(&write)?;
        let identity = (event.uid.clone(), key(event.recurrence_id.as_ref()));
        if old.is_some_and(|m| m.content_hash.as_ref() == hashes.get(&identity)) {
            unchanged.insert(index);
        }
        core::SeriesSpec::new(write.timing.clone()).map_err(|e| validation(e.field, e.reason))?;
        let relationships = if source.kind == "feed" {
            Relationships {
                assignees: Vec::new(),
                ..Default::default()
            }
        } else if let Some(m) = old {
            db::relationships(tx, m.item_id)
                .await
                .map_err(domain_error)?
        } else {
            Relationships {
                assignees: Vec::new(),
                ..Default::default()
            }
        };
        validate_members(tx, c, &write, &relationships).await?;
        request
            .members
            .extend(db::agenda_members(&write, &relationships));
        fields.insert(index, write);
        sets.insert(index, relationships);
    }
    // Retained-series exception edits include closed anchors in the original acquisition plan.
    for m in &mappings {
        if let Some(id) = m.occurrence_id {
            let exists:bool=sqlx::query_scalar("select exists(select 1 from porchlight.item_occurrences where id=$1 and item_id=$2)").bind(id).bind(m.item_id).fetch_one(&mut **tx).await?;
            if exists {
                request.occurrences.push(id);
                request.reopens.push(id);
            }
        }
    }
    let expanded_uids: BTreeSet<_> = parsed
        .events
        .iter()
        .filter(|e| matches!(e.origin, EventOrigin::Expanded { .. }))
        .map(|e| e.uid.clone())
        .collect();
    let removed: Vec<Mapping> = {
        mappings
            .iter()
            .filter(|m| {
                let replaced_representation = (m.expanded_start.is_some()
                    && masters.contains(&m.uid))
                    || (m.recurrence_key.is_empty() && expanded_uids.contains(&m.uid));
                safe_uid(&m.uid)
                    && m.occurrence_id.is_none()
                    && ((remove_missing
                        && !seen.contains(&(m.uid.clone(), m.recurrence_key.clone())))
                        || replaced_representation
                        || exception_keys.contains(&(m.uid.clone(), m.recurrence_key.clone())))
                    && m.expanded_start.is_none_or(|t| t >= start && t < end)
            })
            .cloned()
            .collect()
    };
    // Representation replacement is explicit source content, not ordinary file absence.
    // It applies to one-off reimports too, but still requires the caller's deletion ACL.
    // Source-owned history is not a live body reference. Inspect only deletion state,
    // in one batch, before ACL loading; do not expose details of hidden live items.
    let removed_ids: Vec<_> = removed.iter().map(|m| m.item_id).collect();
    let historical: BTreeSet<Uuid> = sqlx::query_scalar("select id from porchlight.items where household_id=$1 and id=any($2) and deleted_at is not null")
        .bind(c.household_id()).bind(&removed_ids).fetch_all(&mut **tx).await?.into_iter().collect();
    let mut accessible_removed = Vec::new();
    for m in removed {
        if historical.contains(&m.item_id) {
            continue;
        }
        let row = match load(tx, c, m.item_id, true).await {
            Ok(row) => row,
            Err(e) if e.status() == StatusCode::NOT_FOUND => {
                summary.warnings.push(Warning {
                    uid: Some(m.uid.clone()),
                    code: "InvisibleItem".into(),
                    message: "An inaccessible mapped event was not removed".into(),
                });
                continue;
            }
            Err(e) => return Err(e),
        };
        // Earlier refreshes already soft-deleted it (the mapping is retained): the apply does no
        // work, so it is neither locked, authorized nor charged against the work budget.
        if row.deleted_at.is_some() {
            continue;
        }
        if !caps(tx, c, &row).await?.contains(&Capability::Delete) {
            return Err(ApiError::new(Problem::Forbidden));
        }
        planned_versions.insert(row.id, row.version);
        request.items.push(row.id);
        accessible_removed.push(m);
    }
    let removed = accessible_removed;
    enforce_work_budget(
        tx, &input, &map, &hashes, &unchanged, &restoring, &fields, &removed, now,
    )
    .await?;
    // Every existing target occurrence may be skipped/reopened by an exception reset.
    if !request.items.is_empty() {
        let closed:Vec<Uuid>=sqlx::query_scalar("select id from porchlight.item_occurrences where item_id=any($1) and status in ('done','skipped')").bind(&request.items).fetch_all(&mut **tx).await?;
        request.occurrences.extend(closed);
    }
    Ok(CalendarPlan {
        input,
        summary,
        mappings,
        map,
        masters,
        exception_keys,
        hashes,
        unchanged,
        planned_versions,
        fields,
        sets,
        request,
        removed,
    })
}

// Charge actual apply work, not a calendar's unchanged stored projection (R67).
// The 4000-unit cap (R76) bounds apply work; R12's 10 000-instance limit is parse/expansion only.
pub const MAX_CALENDAR_WORK: usize = 4_000;
fn calendar_work_error() -> ApiError {
    ApiError::new(Problem::CalendarTooLarge)
        .detail("Calendar exceeds the 4000-unit apply work budget")
}
#[allow(clippy::too_many_arguments)]
async fn enforce_work_budget(
    tx: &mut Transaction<'_, Postgres>,
    input: &CalendarInput,
    map: &BTreeMap<(String, String), Mapping>,
    hashes: &BTreeMap<(String, String), String>,
    unchanged: &BTreeSet<usize>,
    restoring: &BTreeSet<usize>,
    fields: &BTreeMap<usize, ItemWrite>,
    removed: &[Mapping],
    now: DateTime<Utc>,
) -> Result<(), ApiError> {
    let reject = calendar_work_error;
    // One baseline unit per parsed event; two per removed item for delete + derivation.
    // Retained occurrence rows are charged separately below (a standalone removal normally
    // costs three units). Historical timing ratios are not total admission weights.
    let removal_units = removed.len() * 2;
    let mut work = input.parsed.events.len() + removal_units;
    let mut existing = BTreeSet::new();
    let mut projected = 0;
    for m in removed {
        existing.insert(m.item_id);
    }
    // One pass per collection, rather than series × all exceptions.
    let mut changed_exception_uids = BTreeSet::new();
    for ((uid, k), h) in hashes {
        if !k.is_empty()
            && map
                .get(&(uid.clone(), k.clone()))
                .is_none_or(|m| m.content_hash.as_ref() != Some(h))
        {
            changed_exception_uids.insert(uid.as_str());
        }
    }
    for m in map.values() {
        if m.occurrence_id.is_some()
            && !hashes.contains_key(&(m.uid.clone(), m.recurrence_key.clone()))
            && !input.unsafe_global
            && !input.unsafe_uids.contains(&m.uid)
        {
            changed_exception_uids.insert(m.uid.as_str());
        }
    }
    for index in fields.keys() {
        let event = &input.parsed.events[*index];
        let old = map.get(&(event.uid.clone(), key(event.recurrence_id.as_ref())));
        // Exceptions can change independently of the retained master hash.
        let exceptions_changed = matches!(event.origin, EventOrigin::Series)
            && changed_exception_uids.contains(event.uid.as_str());
        if unchanged.contains(index) && !exceptions_changed && !restoring.contains(index) {
            continue;
        }
        if let Some(m) = old {
            existing.insert(m.item_id);
        }
        if event.status == EventStatus::Cancelled {
            continue;
        }
        if work > MAX_CALENDAR_WORK {
            return Err(reject());
        }
        work += event.exdates.len();
        projected += event.exdates.len();
        if work > MAX_CALENDAR_WORK {
            return Err(reject());
        }
        let spec = core::SeriesSpec::new(timing(event, input.zone)?)
            .map_err(|e| validation(e.field, e.reason))?;
        let zone = spec.zone().unwrap_or(input.zone);
        let horizon = porchlight_core::time::to_local(
            core::horizon_end(now, zone).map_err(|e| validation(e.field, e.reason))?,
            zone,
        );
        // Far-future identities require traversing history even when they do not
        // match the series. Charge that walk before acquiring domain write locks.
        let until = exception_window(event, &input.parsed, zone)?
            .map_or(horizon, |(_, end)| horizon.max(end));
        let rows = core::bounded_occurrence_count(&spec, until, MAX_CALENDAR_WORK - work)
            .map_err(|_| reject())?;
        work += rows;
        projected += rows;
    }
    if work > MAX_CALENDAR_WORK {
        return Err(reject());
    }
    let ids: Vec<_> = existing.into_iter().collect();
    // One aggregate over changed/removed items; never count unchanged mappings/history.
    let rows: i64 = sqlx::query_scalar(
        "select count(*) from porchlight.item_occurrences where item_id=any($1)",
    )
    .bind(ids)
    .fetch_one(&mut **tx)
    .await?;
    let existing = usize::try_from(rows).map_err(|_| reject())?;
    work += existing;
    #[cfg(feature = "testing")]
    crate::testing::calendar_apply_work(work, projected, existing);
    debug_assert_eq!(
        work,
        input.parsed.events.len() + removal_units + projected + existing
    );
    if work > MAX_CALENDAR_WORK {
        return Err(reject());
    }
    Ok(())
}

/// Apply under a cumulative admission budget and a whole-operation deadline.
/// As with every domain error, callers MUST roll back their domain savepoint.
/// R86: the deadline drops the apply mid-protocol. If that lands on a savepoint begin, sqlx has
/// already rolled back the caller's whole transaction (locks and actor settings included) and left
/// it in autocommit, so on `is_calendar_apply_timeout()` callers MUST discard the transaction that
/// holds `tx` and record anything else in a fresh one.
pub async fn apply_calendar(
    tx: &mut Transaction<'_, Postgres>,
    c: &Caller,
    source: &Source,
    content: &str,
    remove_missing: bool,
    now: DateTime<Utc>,
) -> Result<ImportSummary, ApiError> {
    let apply = tokio::time::timeout(
        {
            #[cfg(feature = "testing")]
            {
                crate::testing::calendar_apply_duration()
            }
            #[cfg(not(feature = "testing"))]
            {
                std::time::Duration::from_secs(20)
            }
        },
        apply_calendar_inner(tx, c, source, content, remove_missing, now),
    );
    #[cfg(feature = "testing")]
    let apply = async {
        let result = tokio::select! { biased;
            () = crate::testing::calendar_apply_cancelled() => None,
            result = apply => result.ok(),
        };
        if result.is_none() {
            crate::testing::calendar_apply_deadline_fired();
        }
        result
    };
    #[cfg(not(feature = "testing"))]
    let apply = async { apply.await.ok() };
    apply.await.ok_or_else(|| {
        ApiError::calendar_apply_timeout()
            .retry(1)
            .detail("Calendar apply deadline exceeded; retry the same request")
    })?
}

async fn apply_calendar_inner(
    tx: &mut Transaction<'_, Postgres>,
    c: &Caller,
    source: &Source,
    content: &str,
    remove_missing: bool,
    now: DateTime<Utc>,
) -> Result<ImportSummary, ApiError> {
    let zone = calendar::target_zone(tx, source.household_id, source.target_member_id).await?;
    let input = parse_calendar_blocking(content, zone, now).await?;
    let CalendarPlan {
        input,
        mut summary,
        mappings,
        map,
        masters,
        exception_keys,
        hashes,
        unchanged,
        planned_versions,
        fields,
        sets,
        request,
        removed,
    } = plan_calendar(tx, c, source, input, remove_missing, now).await?;
    let CalendarInput {
        parsed,
        zone,
        unsafe_global,
        unsafe_uids,
        ..
    } = input;
    let safe_uid = |uid: &String| !unsafe_global && !unsafe_uids.contains(uid);
    let mut scope = WriteScope::begin_at(tx, source.household_id, request, now)
        .await
        .map_err(domain_error)?;
    // Item versions include ACL/relationship changes. As in S4a, the planning ACL is
    // authoritative only when its version still matches under the retained scope lock.
    let mut planned_rows = BTreeMap::new();
    for (id, version) in planned_versions {
        let row = scope.item(id).await.map_err(domain_error)?;
        if row.version != version {
            return Err(ApiError::transient_conflict().retry(1));
        }
        planned_rows.insert(id, row);
    }
    let mut save = Vec::new();
    let mut imported = BTreeMap::new();
    for (index, write) in &fields {
        let event = &parsed.events[*index];
        let identity = (event.uid.clone(), key(event.recurrence_id.as_ref()));
        let old = map.get(&identity).filter(|m| m.occurrence_id.is_none());
        let id = if let Some(m) = old {
            let row = if let Some(row) = planned_rows.remove(&m.item_id) {
                row
            } else {
                scope.item(m.item_id).await.map_err(domain_error)?
            };
            if event.status == EventStatus::Cancelled {
                if row.deleted_at.is_none() {
                    scope
                        .soft_delete(row.id, row.version, now)
                        .await
                        .map_err(domain_error)?;
                    summary.deleted += 1;
                }
                if unchanged.contains(index) {
                    summary.unchanged += 1;
                }
                save.push(Mapping {
                    content_hash: hashes.get(&identity).cloned(),
                    ..m.clone()
                });
                imported.insert(event.uid.clone(), row.id);
                continue;
            }
            if unchanged.contains(index) && row.deleted_at.is_none() {
                summary.unchanged += 1;
                summary.item_ids.push(row.id);
                imported.insert(event.uid.clone(), row.id);
                continue;
            }
            let version = if row.deleted_at.is_some() {
                scope.restore_calendar(row.id).await.map_err(domain_error)?;
                scope.item(row.id).await.map_err(domain_error)?.version
            } else {
                row.version
            };
            scope
                .update(row.id, version, write)
                .await
                .map_err(domain_error)?;
            if source.kind == "feed" {
                let version = scope.item(row.id).await.map_err(domain_error)?.version;
                scope
                    .replace_relationships(row.id, version, &sets[index])
                    .await
                    .map_err(domain_error)?;
            }
            if source.kind == "feed" && row.ics_source_id != Some(source.id) {
                scope
                    .mark_feed(row.id, source.id)
                    .await
                    .map_err(domain_error)?;
            }
            summary.updated += 1;
            row.id
        } else {
            if event.status == EventStatus::Cancelled {
                continue;
            }
            summary.created += 1;
            scope
                .create_calendar(
                    write,
                    &sets[index],
                    (source.kind == "feed").then_some(source.id),
                )
                .await
                .map_err(domain_error)?
        };
        #[cfg(feature = "testing")]
        {
            if crate::testing::calendar_apply_contention() {
                return Err(ApiError::transient_conflict().retry(1));
            }
            crate::testing::calendar_after_write().await;
        }
        summary.item_ids.push(id);
        imported.insert(event.uid.clone(), id);
        save.push(Mapping {
            uid: event.uid.clone(),
            recurrence_key: identity.1.clone(),
            item_id: id,
            occurrence_id: None,
            content_hash: hashes.get(&identity).cloned(),
            expanded_start: if matches!(event.origin, EventOrigin::Expanded { .. }) {
                Some(instant(
                    event.recurrence_id.as_ref().unwrap_or(&event.start),
                    zone,
                ))
            } else {
                None
            },
        });
    }
    for m in &removed {
        let row = scope.item(m.item_id).await.map_err(domain_error)?;
        if row.deleted_at.is_none() {
            scope
                .soft_delete(row.id, row.version, now)
                .await
                .map_err(domain_error)?;
            summary.deleted += 1;
        }
    }
    save.extend(
        apply_exceptions(
            &mut scope,
            &mut summary,
            ExceptionPlan {
                parsed: &parsed,
                mappings: &mappings,
                map: &map,
                hashes: &hashes,
                imported: &imported,
                unchanged: &unchanged,
                zone,
                now,
                unsafe_global,
                unsafe_uids: &unsafe_uids,
            },
        )
        .await?,
    );
    let outcomes = scope.finish(now).await.map_err(domain_error)?;
    let changed_items: Vec<_> = outcomes.iter().map(|o| o.item_id).collect();
    summary.conflicts = acl::mask(
        tx,
        c,
        outcomes
            .into_iter()
            .flat_map(|o| o.derived.conflicts)
            .collect(),
    )
    .await?;
    // Once a complete source removes an exception, relinquish that occurrence's source
    // ownership. A later human skip/override must not be undone by another re-import.
    {
        for m in &mappings {
            if safe_uid(&m.uid)
                && m.occurrence_id.is_some()
                && masters.contains(&m.uid)
                && !exception_keys.contains(&(m.uid.clone(), m.recurrence_key.clone()))
            {
                sqlx::query("delete from porchlight.ics_uid_map where source_id=$1 and uid=$2 and recurrence_key=$3")
                    .bind(source.id).bind(&m.uid).bind(&m.recurrence_key).execute(&mut **tx).await?;
            }
        }
    }
    // Preserve sequential-upsert last-wins semantics before batching.
    let save: BTreeMap<_, _> = save
        .into_iter()
        .map(|m| ((m.uid.clone(), m.recurrence_key.clone()), m))
        .collect();
    let save: Vec<_> = save.into_values().collect();
    for batch in save.chunks(500) {
        let mut q = sqlx::QueryBuilder::<Postgres>::new(
            "insert into porchlight.ics_uid_map(household_id,source_id,uid,recurrence_key,item_id,occurrence_id,expanded_start,content_hash) ",
        );
        q.push_values(batch, |mut b, m| {
            b.push_bind(source.household_id)
                .push_bind(source.id)
                .push_bind(&m.uid)
                .push_bind(&m.recurrence_key)
                .push_bind(m.item_id)
                .push_bind(m.occurrence_id)
                .push_bind(m.expanded_start)
                .push_bind(&m.content_hash);
        });
        q.push(" on conflict(source_id,uid,recurrence_key) do update set item_id=excluded.item_id,occurrence_id=excluded.occurrence_id,expanded_start=excluded.expanded_start,content_hash=excluded.content_hash where (ics_uid_map.item_id,ics_uid_map.occurrence_id,ics_uid_map.expanded_start,ics_uid_map.content_hash) is distinct from (excluded.item_id,excluded.occurrence_id,excluded.expanded_start,excluded.content_hash)");
        q.build().execute(&mut **tx).await?;
    }
    summary.removed = summary.deleted;
    calendar::emit_import_notifications(tx, c, &changed_items).await?;
    Ok(summary)
}
pub fn key(time: Option<&IcsTime>) -> String {
    match time {
        None => String::new(),
        Some(IcsTime::Date(d)) => format!("D:{d}"),
        Some(IcsTime::Floating(t)) => format!("L:{t}"),
        Some(IcsTime::Fixed(t)) => format!("U:{}", t.to_rfc3339()),
    }
}
fn instant(t: &IcsTime, zone: chrono_tz::Tz) -> DateTime<Utc> {
    match t {
        IcsTime::Fixed(t) => *t,
        IcsTime::Floating(t) => porchlight_core::time::resolve_local(*t, zone),
        IcsTime::Date(d) => {
            porchlight_core::time::resolve_local(d.and_time(chrono::NaiveTime::MIN), zone)
        }
    }
}
fn matches_anchor(t: &IcsTime, a: &core::OccurrenceAnchor, _zone: chrono_tz::Tz) -> bool {
    match t {
        IcsTime::Date(d) => a.local_start.is_some_and(|t| t.date() == *d),
        IcsTime::Floating(t) => a.local_start == Some(*t),
        IcsTime::Fixed(t) => a.starts_at == Some(*t),
    }
}
fn timing(event: &ImportedEvent, fallback: chrono_tz::Tz) -> Result<core::SeriesInput, ApiError> {
    // Parse aliases permissively; canonicalize all stored zones through R35b.
    // SeriesSpec also validates every item/override input.
    let zone = event
        .tzid
        .as_ref()
        .and_then(|s| porchlight_core::time::parse_tz(s))
        .unwrap_or(fallback);
    let mut input = core::SeriesInput {
        kind: core::Kind::Appointment,
        all_day: event.all_day,
        rrule: event.rrule.clone(),
        recurrence_mode: event.rrule.as_ref().map(|_| core::RecurrenceMode::Calendar),
        ..Default::default()
    };
    match (&event.start, &event.end) {
        (IcsTime::Fixed(s), IcsTime::Fixed(e)) => {
            input.tzid = Some(
                porchlight_core::time::storage_zone(zone.name())
                    .map_err(|_| validation("tzid", "Zone cannot be stored"))?
                    .1
                    .into(),
            );
            input.starts_at = Some(*s);
            input.ends_at = Some(if s == e {
                *e + chrono::Duration::minutes(1)
            } else {
                *e
            });
        }
        (IcsTime::Floating(s), IcsTime::Floating(e)) => {
            input.time_mode = core::TimeMode::Floating;
            input.local_start = Some(*s);
            input.local_end = Some(if s == e {
                *e + chrono::Duration::minutes(1)
            } else {
                *e
            });
        }
        (IcsTime::Date(s), IcsTime::Date(e)) => {
            input.time_mode = core::TimeMode::Floating;
            input.local_start = Some(s.and_time(chrono::NaiveTime::MIN));
            input.local_end = Some(e.and_time(chrono::NaiveTime::MIN));
        }
        _ => return Err(validation("content", "Inconsistent event time types")),
    }
    Ok(input)
}

fn event_fields(
    event: &ImportedEvent,
    source: &Source,
    zone: chrono_tz::Tz,
) -> Result<ItemWrite, ApiError> {
    if event.summary.contains('\0')
        || event.description.contains('\0')
        || event.location.contains('\0')
    {
        return Err(validation("content", "Calendar detail contains NUL"));
    }
    let write = ItemWrite {
        title: if event.summary.is_empty() {
            "Untitled calendar event".into()
        } else {
            event.summary.clone()
        },
        notes: event.description.clone(),
        address: Some(event.location.clone()).filter(|s| !s.is_empty()),
        source: Some(format!("ics:{}", source.name)),
        visibility: source.visibility.clone(),
        timing: timing(event, zone)?,
        ..Default::default()
    };
    validate_fields(&write)?;
    core::SeriesSpec::new(write.timing.clone()).map_err(|e| validation(e.field, e.reason))?;
    Ok(write)
}

fn normalized_hashes(
    events: &[ImportedEvent],
    source: &Source,
    zone: chrono_tz::Tz,
) -> Result<BTreeMap<(String, String), String>, ApiError> {
    let mut hashes = BTreeMap::new();
    for event in events {
        let input = timing(event, zone)?;
        core::SeriesSpec::new(input.clone()).map_err(|e| validation(e.field, e.reason))?;
        let title = if event.summary.is_empty() {
            "Untitled calendar event"
        } else {
            &event.summary
        };
        let value = serde_json::to_vec(&(
            title,
            &event.description,
            &event.location,
            &input,
            event.status,
            if source.kind == "import" {
                source.name.as_str()
            } else {
                ""
            },
        ))
        .map_err(|_| ApiError::internal("calendar_hash"))?;
        hashes.insert(
            (event.uid.clone(), key(event.recurrence_id.as_ref())),
            hex_digest(&value),
        );
        for t in &event.exdates {
            hashes.insert(
                (event.uid.clone(), key(Some(t))),
                hex_digest(
                    &serde_json::to_vec(&("exdate", t))
                        .map_err(|_| ApiError::internal("calendar_hash"))?,
                ),
            );
        }
    }
    Ok(hashes)
}

/// R49/R50: configuration and removal run under the already-held source lock, then
/// one retained item scope. App read-only restrictions never block source administration.
async fn feed_targets(
    tx: &mut Transaction<'_, Postgres>,
    source: &Source,
) -> Result<Vec<Uuid>, ApiError> {
    Ok(sqlx::query_scalar("select id from porchlight.items where household_id=$1 and ics_source_id=$2 and deleted_at is null order by id").bind(source.household_id).bind(source.id).fetch_all(&mut **tx).await?)
}
pub async fn remove_feed(
    tx: &mut Transaction<'_, Postgres>,
    source: &Source,
    now: DateTime<Utc>,
) -> Result<(), ApiError> {
    let ids = feed_targets(tx, source).await?;
    let mut scope = WriteScope::begin_at(
        tx,
        source.household_id,
        ScopeRequest {
            items: ids.clone(),
            ..Default::default()
        },
        now,
    )
    .await
    .map_err(domain_error)?;
    for id in ids {
        let row = scope.item(id).await.map_err(domain_error)?;
        scope
            .soft_delete(id, row.version, now)
            .await
            .map_err(domain_error)?;
    }
    scope.finish(now).await.map_err(domain_error)?;
    sqlx::query("delete from porchlight.ics_uid_map where source_id=$1")
        .bind(source.id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
pub async fn configure_feed(
    tx: &mut Transaction<'_, Postgres>,
    source: &Source,
    body: &calendar::SourceRequest,
    now: DateTime<Utc>,
) -> Result<(), ApiError> {
    if source.target_member_id == body.target_member_id
        && source.visibility == body.visibility()
        && source.name == body.name
    {
        return Ok(());
    }
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "select id from porchlight.items where household_id=$1 and ics_source_id=$2 and deleted_at is null order by id",
    )
    .bind(source.household_id)
    .bind(source.id)
    .fetch_all(&mut **tx)
    .await?;
    let mut configured = source.clone();
    configured.target_member_id = body.target_member_id;
    let owner = calendar::source_owner(tx, &configured).await?;
    let members = db::agenda_members(
        &ItemWrite {
            owner_id: owner,
            subject_member_id: body.target_member_id,
            ..Default::default()
        },
        &Relationships::default(),
    );
    let mut scope = WriteScope::begin_at(
        tx,
        source.household_id,
        ScopeRequest {
            items: ids.clone(),
            members: members.clone(),
            ..Default::default()
        },
        now,
    )
    .await
    .map_err(domain_error)?;
    for id in ids {
        let row = scope.item(id).await.map_err(domain_error)?;
        let mut write = ItemWrite::from(&row);
        write.owner_id = owner;
        write.subject_member_id = body.target_member_id;
        write.visibility = body.visibility();
        write.source = Some(format!("ics:{}", body.name));
        validate_fields(&write)?;
        scope
            .update(id, row.version, &write)
            .await
            .map_err(domain_error)?;
        if source.target_member_id != body.target_member_id {
            let version = scope.item(id).await.map_err(domain_error)?.version;
            scope
                .replace_relationships(
                    id,
                    version,
                    &Relationships {
                        assignees: Vec::new(),
                        ..Default::default()
                    },
                )
                .await
                .map_err(domain_error)?;
        }
    }
    scope.finish(now).await.map_err(domain_error)?;
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

struct ExceptionPlan<'a> {
    parsed: &'a ics::ParsedCalendar,
    mappings: &'a [Mapping],
    map: &'a BTreeMap<(String, String), Mapping>,
    hashes: &'a BTreeMap<(String, String), String>,
    imported: &'a BTreeMap<String, Uuid>,
    unchanged: &'a BTreeSet<usize>,
    zone: chrono_tz::Tz,
    now: DateTime<Utc>,
    unsafe_global: bool,
    unsafe_uids: &'a BTreeSet<String>,
}
// One window per retained series, shared by admission and the single exception walk.
fn exception_window(
    event: &porchlight_core::ics::ImportedEvent,
    parsed: &ics::ParsedCalendar,
    zone: chrono_tz::Tz,
) -> Result<Option<(chrono::NaiveDateTime, chrono::NaiveDateTime)>, ApiError> {
    let mut bounds: Option<(chrono::NaiveDateTime, chrono::NaiveDateTime)> = None;
    for exception in event.exdates.iter().chain(
        parsed
            .events
            .iter()
            .filter(|e| matches!(e.origin, EventOrigin::Override) && e.uid == event.uid)
            .filter_map(|e| e.recurrence_id.as_ref()),
    ) {
        let local = match exception {
            IcsTime::Date(d) => d.and_time(chrono::NaiveTime::MIN),
            IcsTime::Floating(t) => *t,
            IcsTime::Fixed(t) => porchlight_core::time::to_local(*t, zone),
        };
        let end = local
            .checked_add_signed(chrono::Duration::days(1))
            .ok_or_else(calendar_work_error)?;
        bounds = Some(bounds.map_or((local, end), |(from, until)| {
            (from.min(local), until.max(end))
        }));
    }
    Ok(bounds)
}

async fn apply_exceptions(
    scope: &mut WriteScope<'_, '_>,
    summary: &mut ImportSummary,
    plan: ExceptionPlan<'_>,
) -> Result<Vec<Mapping>, ApiError> {
    let ExceptionPlan {
        parsed,
        mappings,
        map,
        hashes,
        imported,
        unchanged,
        zone,
        now,
        unsafe_global,
        unsafe_uids,
    } = plan;
    let safe_uid = |uid: &String| !unsafe_global && !unsafe_uids.contains(uid);
    let mut save = Vec::new();
    for (index, event) in parsed.events.iter().enumerate() {
        if !matches!(event.origin, EventOrigin::Series) || event.status == EventStatus::Cancelled {
            continue;
        }
        let Some(id) = imported.get(&event.uid).copied() else {
            continue;
        };
        let exceptions_unchanged = hashes
            .iter()
            .filter(|((uid, k), _)| uid == &event.uid && !k.is_empty())
            .all(|(k, h)| {
                map.get(k)
                    .is_some_and(|m| m.content_hash.as_ref() == Some(h))
            })
            && mappings
                .iter()
                .filter(|m| m.uid == event.uid && m.occurrence_id.is_some())
                .all(|m| {
                    hashes.contains_key(&(m.uid.clone(), m.recurrence_key.clone()))
                        || !safe_uid(&m.uid)
                });
        if unchanged.contains(&index) && exceptions_unchanged {
            continue;
        }
        let mut rows = scope
            .materialize_calendar(id, now)
            .await
            .map_err(domain_error)?;
        let master = scope
            .item(id)
            .await
            .map_err(domain_error)?
            .spec()
            .map_err(domain_error)?;
        let master_zone = master.zone().unwrap_or(zone);
        let extra_anchors = match exception_window(event, parsed, master_zone)? {
            Some((from, until)) => {
                core::expand(&master, from, until).map_err(|_| calendar_work_error())?
            }
            None => vec![],
        };
        let exceptions = event.exdates.iter().chain(
            parsed
                .events
                .iter()
                .filter(|e| matches!(e.origin, EventOrigin::Override) && e.uid == event.uid)
                .filter_map(|e| e.recurrence_id.as_ref()),
        );
        for exception in exceptions {
            if rows
                .iter()
                .any(|row| matches_anchor(exception, &row.anchor, master_zone))
            {
                continue;
            }
            if let Some(anchor) = extra_anchors
                .iter()
                .find(|a| matches_anchor(exception, a, master_zone))
            {
                rows.push(
                    scope
                        .ensure_calendar_anchor(id, anchor)
                        .await
                        .map_err(domain_error)?,
                );
            } else {
                summary.warnings.push(Warning {
                    uid: Some(event.uid.clone()),
                    code: "UnmappedException".into(),
                    message: "Exception identity does not belong to its retained series".into(),
                });
            }
        }
        for row in &rows {
            let except = event
                .exdates
                .iter()
                .find(|t| matches_anchor(t, &row.anchor, zone));
            let replacement = parsed.events.iter().find(|e| {
                matches!(e.origin, EventOrigin::Override)
                    && e.uid == event.uid
                    && e.recurrence_id
                        .as_ref()
                        .is_some_and(|t| matches_anchor(t, &row.anchor, zone))
            });
            if let Some(override_event) = replacement {
                let identity = (
                    event.uid.clone(),
                    key(override_event.recurrence_id.as_ref()),
                );
                if map.get(&identity).is_some_and(|m| {
                    m.occurrence_id == Some(row.id)
                        && m.content_hash.as_ref() == hashes.get(&identity)
                }) {
                    continue;
                }
                if override_event.status == EventStatus::Cancelled {
                    if row.status != core::Status::Skipped {
                        scope
                            .set_status(row.id, row.version, core::Status::Skipped, None, now)
                            .await
                            .map_err(domain_error)?;
                        summary.skipped += 1;
                        summary.cancelled_skipped += 1;
                    }
                } else {
                    let input = timing(override_event, zone)?;
                    let spec =
                        core::SeriesSpec::new(input).map_err(|e| validation(e.field, e.reason))?;
                    let mut anchor =
                        core::first_anchor(&spec).map_err(|e| validation(e.field, e.reason))?;
                    if let (Some(start), Some(end)) = (anchor.starts_at, anchor.ends_at) {
                        // Overrides persist master-local anchors, not the producer override's display zone.
                        if master.input().time_mode == core::TimeMode::Fixed {
                            anchor.local_start =
                                Some(porchlight_core::time::to_local(start, master_zone));
                            anchor.local_end =
                                Some(porchlight_core::time::to_local(end, master_zone));
                        }
                    }
                    anchor.seq = row.anchor.seq;
                    let mut version = row.version;
                    if row.status == core::Status::Skipped
                        && mappings.iter().any(|m| m.occurrence_id == Some(row.id))
                    {
                        scope
                            .set_status(row.id, version, core::Status::Todo, None, now)
                            .await
                            .map_err(domain_error)?;
                        version = scope
                            .occurrence(row.id)
                            .await
                            .map_err(domain_error)?
                            .version;
                    }
                    scope
                        .override_occurrence(row.id, version, &anchor)
                        .await
                        .map_err(domain_error)?;
                }
                save.push(Mapping {
                    uid: event.uid.clone(),
                    recurrence_key: identity.1.clone(),
                    content_hash: hashes.get(&identity).cloned(),
                    item_id: id,
                    occurrence_id: Some(row.id),
                    expanded_start: None,
                });
            } else if let Some(exdate) = except {
                let identity = (event.uid.clone(), key(Some(exdate)));
                if map.get(&identity).is_some_and(|m| {
                    m.occurrence_id == Some(row.id)
                        && m.content_hash.as_ref() == hashes.get(&identity)
                }) {
                    continue;
                }
                if row.status != core::Status::Skipped {
                    scope
                        .set_status(row.id, row.version, core::Status::Skipped, None, now)
                        .await
                        .map_err(domain_error)?;
                    summary.skipped += 1;
                    summary.cancelled_skipped += 1;
                }
                save.push(Mapping {
                    uid: event.uid.clone(),
                    recurrence_key: identity.1.clone(),
                    content_hash: hashes.get(&identity).cloned(),
                    item_id: id,
                    occurrence_id: Some(row.id),
                    expanded_start: None,
                });
            } else if safe_uid(&event.uid)
                && mappings.iter().any(|m| m.occurrence_id == Some(row.id))
            {
                // Only source-managed exceptions are reset. Human completion is retained.
                let mut version = row.version;
                if row.status == core::Status::Skipped {
                    scope
                        .set_status(row.id, version, core::Status::Todo, None, now)
                        .await
                        .map_err(domain_error)?;
                    version = scope
                        .occurrence(row.id)
                        .await
                        .map_err(domain_error)?
                        .version;
                }
                scope
                    .clear_calendar_override(row.id, version)
                    .await
                    .map_err(domain_error)?;
            }
        }
    }
    Ok(save)
}

#[cfg(test)]
mod audit_authz {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn parser_admission_waits_without_blocking_the_runtime() {
        let permits = PARSE_SLOTS.acquire_many(2).await.unwrap();
        let waiting = tokio::spawn(parse_calendar_blocking(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n",
            chrono_tz::UTC,
            "2026-10-06T12:00:00Z".parse().unwrap(),
        ));
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(
            !waiting.is_finished(),
            "parsing must wait for shared worker admission"
        );
        waiting.abort();
        assert!(matches!(waiting.await, Err(e) if e.is_cancelled()));
        assert_eq!(PARSE_SLOTS.available_permits(), 0);
        drop(permits);
        let parsed = parse_calendar_blocking(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n",
            chrono_tz::UTC,
            "2026-10-06T12:00:00Z".parse().unwrap(),
        )
        .await
        .unwrap();
        assert!(parsed.parsed.events.is_empty());
        assert_eq!(PARSE_SLOTS.available_permits(), 2);
    }
}
