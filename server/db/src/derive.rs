//! One Rust derivation path for synchronous writes and raw-SQL jobs. Never authorizes reads.
use chrono::{DateTime, Duration, NaiveDate, Utc};
use chrono_tz::Tz;
use porchlight_core::{agenda, items as core, time};
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, Postgres, Transaction, types::Json};
use uuid::Uuid;

/// Notification-event retention horizon in days. Single definition for the notifier reminder scan and the
/// carry-forward CTE (R74). The final migrated `pl_retention` is pinned at this boundary by
/// `o2_retention::retention_clock_boundaries_counts_and_repeat`.
pub const NOTIFICATION_EVENT_RETENTION_DAYS: i32 = 90;

#[cfg(test)]
mod retention_horizon_tests {
    #[test]
    fn retention_horizon_matches_0056() {
        let sql = include_str!("../migrations/0056_retention_jobs.sql");
        let want = format!(
            "created_at<p_now-interval '{} days'",
            super::NOTIFICATION_EVENT_RETENTION_DAYS
        );
        assert!(
            sql.contains(&want),
            "pl_retention event horizon differs from NOTIFICATION_EVENT_RETENTION_DAYS"
        );
        let sources = format!(
            "created_at<$2-interval ''{} days''",
            super::NOTIFICATION_EVENT_RETENTION_DAYS
        );
        assert_eq!(
            sql.matches(&sources).count(),
            5,
            "every event/notification retention step uses the shared horizon"
        );
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Sql(#[from] sqlx::Error),
    #[error(transparent)]
    Validation(#[from] core::ValidationError),
    #[error("item or occurrence not found")]
    NotFound,
    #[error("resource version is stale")]
    VersionConflict,
    #[error("scope acquisition changed; retry the transaction")]
    RetryableScope,
    #[error("only one WriteScope is allowed per transaction")]
    ScopeAlreadyUsed,
    #[error(transparent)]
    Agenda(#[from] agenda::AgendaError),
    #[error(transparent)]
    Ordering(#[from] porchlight_core::ordering::OrderingError),
}
/// Redacted categories only: never SQL messages, validation values or titles.
pub fn error_category(error: &Error) -> &str {
    match error {
        Error::Sql(sqlx::Error::Database(db)) => db
            .code()
            .as_deref()
            .map(|code| match code {
                "55P03" => "lock_unavailable",
                "40P01" => "deadlock",
                "23505" => "unique_violation",
                _ => "database",
            })
            .unwrap_or("database"),
        Error::Sql(_) => "connection",
        Error::Validation(_) => "validation",
        Error::Agenda(_) => "agenda",
        Error::Ordering(_) => "ordering",
        Error::NotFound => "not_found",
        Error::VersionConflict => "version",
        Error::RetryableScope => "scope_retry",
        Error::ScopeAlreadyUsed => "scope_reuse",
    }
}
pub type Result<T> = std::result::Result<T, Error>;

/// Raw conflicts; the HTTP service must mask inaccessible other items as Busy.
#[derive(Clone, Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct Conflict {
    pub member_id: Uuid,
    pub occurrence_id: Uuid,
    pub other_occurrence_id: Uuid,
    pub other_item_id: Option<Uuid>,
    pub other_title: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub kind: String,
}
#[derive(Clone, Debug)]
pub struct DeriveOutcome {
    pub applied_gen: i64,
    pub conflicts: Vec<Conflict>,
    /// Old/new member-local load dates, accumulated once by the owning scope.
    pub load_dates: Vec<(Uuid, NaiveDate)>,
}

/// Authoritative item snapshot with core's exact interval representation.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ItemRow {
    pub id: Uuid,
    pub household_id: Uuid,
    pub owner_id: Uuid,
    pub subject_member_id: Option<Uuid>,
    pub title: String,
    pub notes: String,
    pub priority: i16,
    pub visibility: String,
    pub address: Option<String>,
    pub source: Option<String>,
    /// S8a: authoritative feed marker; one-off imports leave this null.
    #[serde(default)]
    pub ics_source_id: Option<Uuid>,
    pub buffer_before_min: i32,
    pub buffer_after_min: i32,
    pub est_minutes: Option<i32>,
    pub series_id: Option<Uuid>,
    pub split_from_item_id: Option<Uuid>,
    pub split_from_seq: Option<i32>,
    pub nominal_local_anchor: Option<chrono::NaiveDateTime>,
    pub nominal_wall_duration: Option<i64>,
    pub version: i32,
    pub deleted_at: Option<DateTime<Utc>>,
    #[serde(flatten)]
    pub timing: core::SeriesInput,
}
impl ItemRow {
    pub fn spec(&self) -> Result<core::SeriesSpec> {
        let cursor = self
            .nominal_local_anchor
            .map(|nominal_local_anchor| core::CalendarCursor {
                nominal_local_anchor,
                wall_duration: self.nominal_wall_duration.map(Duration::microseconds),
            });
        Ok(core::SeriesSpec::new_with_calendar_cursor(
            self.timing.clone(),
            cursor,
        )?)
    }
}
// Complex snapshots use typed JSON decoding rather than duplicating 40 SQLx fields.
// Intervals keep months/days/microseconds separately; no epoch/month approximation.
// Every ItemRow loader must use this projection: plain to_jsonb renders the interval
// as Postgres text ("2 days"), which ItemRow (core::PgInterval) cannot decode.
const LOAD_ITEM: &str = "select to_jsonb(i) || jsonb_build_object(
    'completion_interval',porchlight.pl_interval_json(i.completion_interval))
    from porchlight.items i where id=$1";
const LOAD_ITEMS: &str = "select to_jsonb(i) || jsonb_build_object(
    'completion_interval',porchlight.pl_interval_json(i.completion_interval))
    from porchlight.items i where id=any($1)";
pub async fn load_item(conn: &mut PgConnection, id: Uuid) -> Result<ItemRow> {
    let Json(row) = sqlx::query_scalar::<_, Json<ItemRow>>(sqlx::AssertSqlSafe(LOAD_ITEM))
        .bind(id)
        .fetch_optional(conn)
        .await?
        .ok_or(Error::NotFound)?;
    Ok(row)
}
/// Batched [`load_item`] for reads: rows that exist, in no particular order. Never authorizes.
pub async fn load_items(conn: &mut PgConnection, ids: &[Uuid]) -> Result<Vec<ItemRow>> {
    Ok(
        sqlx::query_scalar::<_, Json<ItemRow>>(sqlx::AssertSqlSafe(LOAD_ITEMS))
            .bind(ids)
            .fetch_all(conn)
            .await?
            .into_iter()
            .map(|Json(row)| row)
            .collect(),
    )
}
#[derive(Clone, Debug, Deserialize)]
pub struct OccurrenceRow {
    pub id: Uuid,
    pub item_id: Uuid,
    pub status: core::Status,
    pub disposition: core::Disposition,
    pub board_position: String,
    pub completed_by: Option<Uuid>,
    pub completed_at: Option<DateTime<Utc>>,
    pub skipped_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub board_moved: bool,
    pub is_detached: bool,
    pub version: i32,
    pub override_local_start: Option<chrono::NaiveDateTime>,
    pub override_local_end: Option<chrono::NaiveDateTime>,
    pub override_local_due: Option<chrono::NaiveDateTime>,
    pub override_due_date: Option<NaiveDate>,
    #[serde(flatten)]
    pub anchor: core::OccurrenceAnchor,
}
pub async fn occurrences(conn: &mut PgConnection, item: Uuid) -> Result<Vec<OccurrenceRow>> {
    Ok(sqlx::query_scalar::<_, Json<OccurrenceRow>>(
        "select to_jsonb(o) from porchlight.item_occurrences o where item_id=$1 order by id",
    )
    .bind(item)
    .fetch_all(conn)
    .await?
    .into_iter()
    .map(|j| j.0)
    .collect())
}
pub fn effective_anchor(
    row: &OccurrenceRow,
    spec: &core::SeriesSpec,
) -> Result<core::OccurrenceAnchor> {
    effective_anchor_with_zone(row, spec.input().time_mode, spec.zone())
}

/// Apply occurrence overrides without validating a recurrence rule. Read projections
/// must remain available while a raw-SQL rule is pending/poisoned. The same time/DST
/// resolution is used by derivation and snapshot membership.
pub fn effective_anchor_with_zone(
    row: &OccurrenceRow,
    time_mode: core::TimeMode,
    zone: Option<chrono_tz::Tz>,
) -> Result<core::OccurrenceAnchor> {
    let mut a = row.anchor.clone();
    if !row.is_detached {
        return Ok(a);
    }
    a.local_start = row.override_local_start.or(a.local_start);
    a.local_end = row.override_local_end.or(a.local_end);
    a.local_due = row.override_local_due.or(a.local_due);
    a.due_date = row.override_due_date.or(a.due_date);
    if time_mode == core::TimeMode::Fixed
        && let Some(zone) = zone
    {
        if let (Some(s), Some(e)) = (a.local_start, a.local_end) {
            let (s, e) = time::resolve_span(s, e, zone).map_err(|e| core::ValidationError {
                field: "override_local_end",
                reason: e.to_string(),
            })?;
            a.starts_at = Some(s);
            a.ends_at = Some(e);
        }
        if let Some(d) = a.local_due {
            a.due_at = Some(time::resolve_local(d, zone));
        }
    }
    Ok(a)
}

/// Authoritative agenda members plus stale projection members; used by the worker.
pub async fn affected_members(conn: &mut PgConnection, item: Uuid) -> Result<Vec<Uuid>> {
    let row = load_item(conn, item).await?;
    let sets = crate::items::relationships(conn, item).await?;
    let mut members = crate::items::agenda_members(&crate::items::ItemWrite::from(&row), &sets);
    let stale: Vec<Uuid> = sqlx::query_scalar(
        "select distinct member_id from porchlight.member_agenda where item_id=$1",
    )
    .bind(item)
    .fetch_all(&mut *conn)
    .await?;
    members.extend(stale);
    members.sort();
    members.dedup();
    Ok(members)
}

/// Lock occurrences before derivation. Caller already holds the item row.
pub async fn lock_generated(conn: &mut PgConnection, items: &[Uuid]) -> Result<()> {
    sqlx::query(
        "select id from porchlight.item_occurrences where item_id=any($1) order by id for update",
    )
    .bind(items)
    .fetch_all(&mut *conn)
    .await?;
    sqlx::query("select item_id from porchlight.item_derivation where item_id=any($1) order by item_id for update").bind(items).fetch_all(&mut *conn).await?;
    Ok(())
}

/// Caller must hold the item lock; multi-item callers prelock all occurrences/derivation rows.
/// Upserts ONLY generated fields, never detached/closed anchors or authoritative state.
pub async fn derive_item(
    tx: &mut Transaction<'_, Postgres>,
    item: Uuid,
    now: DateTime<Utc>,
) -> Result<DeriveOutcome> {
    lock_generated(tx, &[item]).await?;
    let members:Vec<Uuid> = sqlx::query_scalar("select m.id from porchlight.household_members m where m.id in (select a.member_id from porchlight.member_agenda a where a.item_id=$1 union select a.member_id from porchlight.item_assignees a where a.item_id=$1 union select i.subject_member_id from porchlight.items i where i.id=$1 and i.subject_member_id is not null union select i.owner_id from porchlight.items i where i.id=$1 and i.subject_member_id is null and not exists(select 1 from porchlight.item_assignees a where a.item_id=i.id)) order by m.id for no key update").bind(item).fetch_all(&mut **tx).await?;
    let mut outcome = derive_item_prelocked(tx, item, now).await?;
    crate::planning::recompute_load_dates(tx, &members, &outcome.load_dates, now).await?;
    outcome.conflicts = conflicts(tx, item, &members).await?;
    Ok(outcome)
}

/// WriteScope has already acquired every existing domain lock BEFORE members.
/// Do not call outside that protocol; newly generated rows belong to this transaction.
pub async fn derive_item_prelocked(
    tx: &mut Transaction<'_, Postgres>,
    item: Uuid,
    now: DateTime<Utc>,
) -> Result<DeriveOutcome> {
    let row = load_item(tx, item).await?;
    let seen: i64 =
        sqlx::query_scalar("select requested_gen from porchlight.item_derivation where item_id=$1")
            .bind(item)
            .fetch_one(&mut **tx)
            .await?;
    // Capture the old membership in the existing delete, without reloading item/relationships.
    let mut load_dates: Vec<(Uuid, Option<NaiveDate>)> = sqlx::query_as(
        "delete from porchlight.member_agenda a where item_id=$1 returning member_id,coalesce((a.starts_at at time zone (select porchlight.pl_effective_member_zone(m.role,m.tzid,h.tzid) from porchlight.household_members m join porchlight.households h on h.id=m.household_id where m.id=a.member_id))::date,(a.due_at at time zone (select porchlight.pl_effective_member_zone(m.role,m.tzid,h.tzid) from porchlight.household_members m join porchlight.households h on h.id=m.household_id where m.id=a.member_id))::date,a.due_date)",
    )
    .bind(item)
    .fetch_all(&mut **tx)
    .await?;
    // A due projection is a pending delivery obligation, not just future cache.
    // Carry it across replacement only if its authoritative identity/instant is
    // still valid below (changed offsets/anchors, closed/deleted occurrences and
    // removed recipients must not resurrect obsolete reminders).
    // Emitted obligations must be retired while the dedupe witness exists;
    // otherwise retention would eventually let a carried fired tuple replay.
    // R74: tuples whose fire_at fell out of the event-retention horizon are dropped (their witness may be gone).
    // One statement replaces the projection AND filters due tuples. Keep row IDs
    // for unchanged future/pending obligations: clients key native alarms by them.
    let old_reminders: Vec<(Uuid, Uuid, Uuid, Uuid, DateTime<Utc>)> = sqlx::query_as(
        "with removed as (delete from porchlight.scheduled_reminders where item_id=$1 returning id,household_id,member_id,occurrence_id,reminder_id,fire_at) select id,member_id,occurrence_id,reminder_id,fire_at from removed r where fire_at>$2 or (fire_at<=$2 and fire_at>$2-make_interval(days=>$3) and not exists(select 1 from porchlight.notification_events e where e.household_id=r.household_id and e.dedupe_key='reminder:'||r.occurrence_id::text||':'||r.member_id::text||':'||r.reminder_id::text))",
    )
    .bind(item)
    .bind(now)
    .bind(NOTIFICATION_EVENT_RETENTION_DAYS)
    .fetch_all(&mut **tx)
    .await?;
    let retained_reminders: std::collections::BTreeMap<_, _> = old_reminders
        .into_iter()
        .map(|(id, member, occurrence, reminder, fire)| ((member, occurrence, reminder, fire), id))
        .collect();
    let mut touched = Vec::new();
    let mut calendar_horizon = None;
    let mut materialization_limited = false;
    if row.deleted_at.is_none() {
        let spec = row.spec()?;
        let household_zone: String =
            sqlx::query_scalar("select tzid from porchlight.households where id=$1")
                .bind(row.household_id)
                .fetch_one(&mut **tx)
                .await?;
        let zone = spec.zone().unwrap_or(parse_zone(&household_zone)?);
        let horizon = core::horizon_end(now, zone)?;
        if spec.input().recurrence_mode == Some(core::RecurrenceMode::Calendar) {
            calendar_horizon = Some(horizon);
        }
        let until = time::to_local(horizon, zone);
        let from = NaiveDate::from_ymd_opt(1, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let old = occurrences(tx, item).await?;
        let mut desired = if spec.input().recurrence_mode == Some(core::RecurrenceMode::Completion)
        {
            let mut opens: Vec<_> = old.iter().filter(|o| o.status.is_open()).collect();
            opens.sort_by_key(|o| o.anchor.seq);
            // Latest closure wins, independently of sequence (older rows can be reclosed).
            let prior = old
                .iter()
                .filter(|o| !o.status.is_open())
                .filter_map(|o| o.closed_at.map(|stamp| (stamp, o.anchor.seq, o)))
                .max_by_key(|(stamp, seq, _)| (*stamp, *seq))
                .map(|(_, _, o)| o);
            let reopened = opens.first().filter(|o| o.closed_at.is_some());
            let mut anchor = if let Some(open) = reopened {
                open.anchor.clone() // reopen never changes the original occurrence's anchor
            } else if let Some(closed) = prior {
                let member_zone = if let Some(member) = closed.completed_by {
                    sqlx::query_scalar::<_,String>("select porchlight.pl_effective_member_zone(m.role,m.tzid,h.tzid) from porchlight.household_members m join porchlight.households h on h.id=m.household_id where m.id=$1").bind(member).fetch_one(&mut **tx).await?
                } else {
                    household_zone.clone()
                };
                core::next_after_completion(
                    &spec,
                    closed.completed_at.or(closed.skipped_at).unwrap(),
                    parse_zone(&member_zone)?,
                )?
            } else {
                core::first_anchor(&spec)?
            };
            anchor.seq = if let Some(open) = opens.first() {
                open.anchor.seq
            } else if let Some(last) = old.iter().map(|o| o.anchor.seq).max() {
                last.checked_add(1).ok_or_else(|| core::ValidationError {
                    field: "seq",
                    reason: "sequence overflow".into(),
                })?
            } else {
                0
            };
            let mut anchors = vec![anchor];
            if let Some(reopened) = reopened {
                // Same (closure instant, seq) ordering as N1. Open rows retain
                // closed_at so reclosed older sequences remain the latest on reopen.
                let latest = old
                    .iter()
                    .filter_map(|o| o.closed_at.map(|stamp| (stamp, o.anchor.seq)))
                    .max()
                    == reopened.closed_at.map(|stamp| (stamp, reopened.anchor.seq));
                // R31: delete only untouched successors when reopening the latest closure.
                // A touched successor (including ever moved and then moved back) remains.
                anchors.extend(
                    opens
                        .iter()
                        .skip(1)
                        .filter(|o| {
                            !latest
                                || o.status != core::Status::Todo
                                || o.is_detached
                                || o.board_moved
                                || o.override_local_start.is_some()
                                || o.override_local_end.is_some()
                                || o.override_local_due.is_some()
                                || o.override_due_date.is_some()
                        })
                        .map(|o| o.anchor.clone()),
                );
            }
            anchors
        } else if spec.input().recurrence_mode.is_none() {
            vec![core::first_anchor(&spec)?]
        } else {
            let (anchors, exhausted) = core::expand_capped(&spec, from, until)?;
            materialization_limited = exhausted;
            if exhausted {
                tracing::warn!(item_id = %item, "Series exceeds materialization budget; deriving bounded history");
            }
            anchors
        };
        desired.sort_by_key(|a| a.seq);
        let seqs: Vec<i32> = desired
            .iter()
            .map(|a| {
                i32::try_from(a.seq).map_err(|_| core::ValidationError {
                    field: "seq",
                    reason: "sequence overflow".into(),
                })
            })
            .collect::<std::result::Result<_, _>>()?;
        // A capped prefix is not a complete desired set. Existing work beyond it
        // remains authoritative and still participates in agenda/reminder projection.
        if !materialization_limited {
            sqlx::query("delete from porchlight.item_occurrences where item_id=$1 and status not in ('done','skipped') and not is_detached and not (seq=any($2))").bind(item).bind(&seqs).execute(&mut **tx).await?;
        }
        let immutable_seqs: std::collections::BTreeSet<_> = old
            .iter()
            .filter(|o| !o.status.is_open() || o.is_detached)
            .map(|o| o.anchor.seq)
            .collect();
        // Never even conflict-lock immutable history after member acquisition (round A).
        let generated: Vec<_> = desired
            .into_iter()
            .filter(|a| !immutable_seqs.contains(&a.seq))
            .collect();
        let changed: Vec<Json<OccurrenceRow>> = sqlx::query_scalar(
            "insert into porchlight.item_occurrences(household_id,item_id,seq,local_start,local_end,local_due,due_date,starts_at,ends_at,due_at) select $1,$2,a.* from jsonb_to_recordset($3) as a(seq int,local_start timestamp,local_end timestamp,local_due timestamp,due_date date,starts_at timestamptz,ends_at timestamptz,due_at timestamptz) on conflict(item_id,seq) do update set local_start=excluded.local_start,local_end=excluded.local_end,local_due=excluded.local_due,due_date=excluded.due_date,starts_at=excluded.starts_at,ends_at=excluded.ends_at,due_at=excluded.due_at where not item_occurrences.is_detached and item_occurrences.status not in ('done','skipped') and (item_occurrences.local_start,item_occurrences.local_end,item_occurrences.local_due,item_occurrences.due_date,item_occurrences.starts_at,item_occurrences.ends_at,item_occurrences.due_at) is distinct from (excluded.local_start,excluded.local_end,excluded.local_due,excluded.due_date,excluded.starts_at,excluded.ends_at,excluded.due_at) returning to_jsonb(item_occurrences)"
        ).bind(row.household_id).bind(item).bind(Json(&generated)).fetch_all(&mut **tx).await?;
        let mut current: std::collections::BTreeMap<_, _> = old
            .into_iter()
            .filter(|o| {
                materialization_limited
                    || immutable_seqs.contains(&o.anchor.seq)
                    || seqs.binary_search(&(o.anchor.seq as i32)).is_ok()
            })
            .map(|o| (o.anchor.seq, o))
            .collect();
        current.extend(changed.into_iter().map(|o| (o.0.anchor.seq, o.0)));
        let current: Vec<_> = current.into_values().collect();
        let assignees: Vec<Uuid> = sqlx::query_scalar(
            "select member_id from porchlight.item_assignees where item_id=$1 order by member_id",
        )
        .bind(item)
        .fetch_all(&mut **tx)
        .await?;
        let members = agenda::agenda_members(
            row.owner_id,
            &assignees,
            row.subject_member_id
                .map(|m| (m, porchlight_core::access::Role::Dependent)),
        );
        touched.extend(members.iter().copied());
        let zones:Vec<(Uuid,String)>=sqlx::query_as("select m.id,porchlight.pl_effective_member_zone(m.role,m.tzid,h.tzid) from porchlight.household_members m join porchlight.households h on h.id=m.household_id where m.id=any($1) and m.deleted_at is null order by m.id").bind(&touched).fetch_all(&mut **tx).await?;
        // Resolve defaults/explicit offsets for all touched members in one
        // statement; fallback remains member-specific, without N member reads.
        let offsets: Vec<(Uuid, Uuid, i32)> = sqlx::query_as(
            "select m.member_id,r.id,r.offset_minutes from unnest($2::uuid[]) m(member_id) cross join lateral porchlight.pl_reminder_offsets($1,m.member_id) r order by m.member_id,r.id",
        ).bind(item).bind(zones.iter().map(|(m,_)| *m).collect::<Vec<_>>())
            .fetch_all(&mut **tx).await?;
        let mut member_offsets: std::collections::BTreeMap<_, Vec<_>> =
            zones.iter().map(|(m, _)| (*m, Vec::new())).collect();
        for (member, id, offset) in offsets {
            member_offsets
                .get_mut(&member)
                .expect("queried member")
                .push((id, offset));
        }

        let effective: Vec<core::OpenOccurrence> = current
            .iter()
            .map(|o| {
                Ok(core::OpenOccurrence {
                    anchor: effective_anchor(o, &spec)?,
                    status: o.status,
                    disposition: o.disposition,
                    closed_at: o.closed_at,
                    generated_anchor: o.is_detached.then(|| o.anchor.clone()),
                })
            })
            .collect::<Result<_>>()?;
        // Disposition is per occurrence. Floating missed boundaries use its owner/dependent's authority zone.
        let disposition_zone:String=sqlx::query_scalar("select porchlight.pl_effective_member_zone(m.role,m.tzid,h.tzid) from porchlight.household_members m join porchlight.households h on h.id=m.household_id where m.id=$1").bind(row.subject_member_id.unwrap_or(row.owner_id)).fetch_one(&mut **tx).await?;
        let dispositions = core::dispositions(
            &effective,
            now,
            spec.input().missed_policy,
            spec.zone().unwrap_or(parse_zone(&disposition_zone)?),
        )?;
        // Board disposition remains owner/subject-based. Floating projections have
        // recipient-local expiry, including when the owner is not an agenda member.
        let mut recipient_dispositions = std::collections::BTreeMap::new();
        if spec.input().time_mode == core::TimeMode::Floating {
            for (member, z) in &zones {
                recipient_dispositions.insert(
                    *member,
                    core::dispositions(
                        &effective,
                        now,
                        spec.input().missed_policy,
                        parse_zone(z)?,
                    )?,
                );
            }
        }
        let mut disposition_rows = Vec::new();
        let mut agenda_rows = Vec::new();
        let mut reminder_rows = Vec::new();
        for (index, ((o, e), d)) in current.iter().zip(&effective).zip(dispositions).enumerate() {
            if !o.status.is_open() {
                continue;
            }
            if o.disposition != d {
                disposition_rows.push(serde_json::json!({"id":o.id,"disposition":d.as_str()}));
            }
            for (member, z) in &zones {
                let recipient_disposition =
                    recipient_dispositions.get(member).map_or(d, |ds| ds[index]);
                if recipient_disposition == core::Disposition::Expired {
                    continue;
                }
                let a = &e.anchor;
                let ar = agenda::agenda_row(
                    &agenda::Anchor {
                        starts_at: a.starts_at,
                        ends_at: a.ends_at,
                        due_at: a.due_at,
                        due_date: a.due_date,
                        local_start: a.local_start,
                        local_end: a.local_end,
                        local_due: a.local_due,
                    },
                    spec.input().kind,
                    spec.input().time_mode,
                    spec.input().all_day,
                    agenda::Buffers {
                        before_min: row.buffer_before_min,
                        after_min: row.buffer_after_min,
                    },
                    parse_zone(z)?,
                )?;
                if row.est_minutes.is_some() {
                    let date = ar
                        .starts_at
                        .or(ar.due_at)
                        .map(|t| time::to_local(t, parse_zone(z).expect("validated zone")).date())
                        .or(ar.due_date);
                    load_dates.push((*member, date));
                }
                agenda_rows.push(serde_json::json!({"member_id":member,"disposition":recipient_disposition.as_str(),"occurrence_id":o.id,"starts_at":ar.starts_at,"ends_at":ar.ends_at,"due_at":ar.due_at,"due_date":ar.due_date,"busy_start":ar.busy.map(|b|b.starts_at()),"busy_end":ar.busy.map(|b|b.ends_at())}));
                let base = ar.starts_at.or(ar.due_at).or(ar.due_date.map(|date| {
                    time::start_of_day(date, parse_zone(z).expect("zone already validated"))
                }));
                if let Some(base) = base {
                    for (reminder, offset) in &member_offsets[member] {
                        let fire = base
                            .checked_sub_signed(Duration::minutes(i64::from(*offset)))
                            .ok_or_else(|| core::ValidationError {
                                field: "offset_minutes",
                                reason: "reminder overflow".into(),
                            })?;
                        let id = retained_reminders.get(&(*member, o.id, *reminder, fire));
                        if fire <= now && id.is_none() {
                            continue;
                        }
                        reminder_rows.push(serde_json::json!({"id":id,"member_id":member,"occurrence_id":o.id,"reminder_id":reminder,"fire_at":fire}));
                    }
                }
            }
        }
        sqlx::query("update porchlight.item_occurrences o set disposition=a.disposition from jsonb_to_recordset($1) as a(id uuid,disposition text) where o.id=a.id and o.disposition is distinct from a.disposition").bind(Json(&disposition_rows)).execute(&mut **tx).await?;
        // Bounded multi-row writes: no per-occurrence/member/reminder round trips.
        for chunk in agenda_rows.chunks(1000) {
            sqlx::query("insert into porchlight.member_agenda(household_id,member_id,occurrence_id,item_id,starts_at,ends_at,due_at,due_date,busy,derived_at,disposition) select $1,a.member_id,a.occurrence_id,$2,a.starts_at,a.ends_at,a.due_at,a.due_date,case when a.busy_start is null then null else tstzrange(a.busy_start,a.busy_end,'[)') end,$4,a.disposition from jsonb_to_recordset($3) as a(member_id uuid,occurrence_id uuid,starts_at timestamptz,ends_at timestamptz,due_at timestamptz,due_date date,busy_start timestamptz,busy_end timestamptz,disposition text)").bind(row.household_id).bind(item).bind(Json(chunk)).bind(now).execute(&mut **tx).await?;
        }
        for chunk in reminder_rows.chunks(1000) {
            sqlx::query("insert into porchlight.scheduled_reminders(id,household_id,member_id,occurrence_id,item_id,reminder_id,fire_at,derived_at) select coalesce(a.id,uuidv7()),$1,a.member_id,a.occurrence_id,$2,a.reminder_id,a.fire_at,$4 from jsonb_to_recordset($3) as a(id uuid,member_id uuid,occurrence_id uuid,reminder_id uuid,fire_at timestamptz)").bind(row.household_id).bind(item).bind(Json(chunk)).bind(now).execute(&mut **tx).await?;
        }
    }
    // Emit hard calendar conflicts only. The shared read function also returns
    // S8b load warnings; those are emitted by planning, not as hard conflicts.
    // Dedupe both orientations of same-item overlaps before invoking the emitter.
    // The item itself is necessarily mid-derivation; pending OTHER items are excluded.
    sqlx::query("select porchlight.pl_emit_notification_event($1,c.member_id,'conflict','occurrence',c.occurrence_id,'{}',$2,c.occurrence_id,'conflict:'||c.left_id||':'||c.member_id||':'||c.right_id) from (select distinct on (a.member_id,least(a.occurrence_id,b.occurrence_id),greatest(a.occurrence_id,b.occurrence_id)) a.member_id,a.occurrence_id,least(a.occurrence_id,b.occurrence_id) as left_id,greatest(a.occurrence_id,b.occurrence_id) as right_id from porchlight.member_agenda a join porchlight.member_agenda b on b.member_id=a.member_id and b.busy && a.busy and b.occurrence_id<>a.occurrence_id join porchlight.items i on i.id=b.item_id and i.deleted_at is null where a.item_id=$2 and a.member_id=any($3) and (b.item_id=$2 or exists(select 1 from porchlight.item_derivation d where d.item_id=b.item_id and d.applied_gen=d.requested_gen)) order by a.member_id,least(a.occurrence_id,b.occurrence_id),greatest(a.occurrence_id,b.occurrence_id),a.occurrence_id) c")
        .bind(row.household_id).bind(item).bind(&touched).execute(&mut **tx).await?;

    // A newer request remains pending; no GUC suppresses authoritative triggers.
    let applied:Option<i64>=sqlx::query_scalar("update porchlight.item_derivation set applied_gen=$2,attempts=0,retry_at=$3,last_error=case when $5 then coalesce(last_error,'Series exceeds materialization budget; existing occurrences retained') else null end,updated_at=$3,calendar_horizon=$4 where item_id=$1 and requested_gen=$2 returning applied_gen").bind(item).bind(seen).bind(now).bind(calendar_horizon).bind(materialization_limited).fetch_optional(&mut **tx).await?;
    let applied_gen = match applied {
        Some(generation) => generation,
        None => {
            sqlx::query_scalar(
                "select applied_gen from porchlight.item_derivation where item_id=$1",
            )
            .bind(item)
            .fetch_one(&mut **tx)
            .await?
        }
    };
    sqlx::query("select porchlight.pl_emit_derived($1,$2,false)")
        .bind(item)
        .bind(applied_gen)
        .execute(&mut **tx)
        .await?;
    let mut load_dates: Vec<_> = load_dates
        .into_iter()
        .filter_map(|(m, d)| d.map(|d| (m, d)))
        .collect();
    load_dates.sort();
    load_dates.dedup();
    // Aggregate writes belong to the scope/worker that owns member locks, not a peer derive.
    let conflicts = Vec::new();
    Ok(DeriveOutcome {
        applied_gen,
        conflicts,
        load_dates,
    })
}
pub fn parse_zone(z: &str) -> Result<Tz> {
    time::parse_tz(z).ok_or_else(|| {
        core::ValidationError {
            field: "tzid",
            reason: "unknown IANA zone".into(),
        }
        .into()
    })
}
/// GiST && finds same-item and other-item overlaps, excludes only the same occurrence.
pub async fn conflicts(
    conn: &mut PgConnection,
    item: Uuid,
    members: &[Uuid],
) -> Result<Vec<Conflict>> {
    let mut result = sqlx::query_as::<_,Conflict>("select a.member_id,a.occurrence_id,b.occurrence_id as other_occurrence_id,b.item_id as other_item_id,i.title as other_title,greatest(lower(a.busy),lower(b.busy)) as starts_at,least(upper(a.busy),upper(b.busy)) as ends_at,'hard'::text as kind from porchlight.member_agenda a join porchlight.member_agenda b on b.member_id=a.member_id and b.busy && a.busy and b.occurrence_id<>a.occurrence_id join porchlight.items i on i.id=b.item_id and i.deleted_at is null where a.item_id=$1 and a.member_id=any($2) order by a.member_id,a.occurrence_id,b.occurrence_id").bind(item).bind(members).fetch_all(&mut *conn).await?;
    result.extend(
        crate::planning::load_conflicts(conn, &[item], Some(members))
            .await?
            .into_iter()
            .map(|(_, c)| c),
    );
    Ok(result)
}
