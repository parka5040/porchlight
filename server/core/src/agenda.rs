//! Pure per-member agenda derivation and half-open hard conflicts.
//!
//! This module does not authorize reads. Callers must join authoritative ACLs and must drain
//! pending derivation before comparing rows. Status/disposition filtering is the caller's job.
#![deny(missing_docs)]

use crate::{
    access::Role,
    time::{ResolveSpanError, resolve_local, resolve_span},
};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

/// Shared database/API item enums, also used by recurrence.
pub use crate::items::{Kind, TimeMode};

/// Effective occurrence anchors, after applying detached occurrence overrides.
///
/// The recurrence/persistence layer chooses these values; agenda only uses columns relevant to
/// kind and time mode. Full item input-shape validation belongs to the item layer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    /// Fixed appointment start.
    pub starts_at: Option<DateTime<Utc>>,
    /// Fixed appointment end.
    pub ends_at: Option<DateTime<Utc>>,
    /// Fixed deadline due instant.
    pub due_at: Option<DateTime<Utc>>,
    /// Todo due date, never converted through a time zone.
    pub due_date: Option<NaiveDate>,
    /// Floating appointment start wall time.
    pub local_start: Option<NaiveDateTime>,
    /// Floating appointment end wall time.
    pub local_end: Option<NaiveDateTime>,
    /// Floating deadline due wall time.
    pub local_due: Option<NaiveDateTime>,
}

/// Travel buffers, in integer minutes with the same 0..=1440 limits as the item table.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Buffers {
    /// Minutes before the start.
    pub before_min: i32,
    /// Minutes after the end.
    pub after_min: i32,
}

/// Invalid agenda inputs; no raw input strings or identifiers are included in errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AgendaError {
    /// A required anchor is absent.
    #[error("required agenda anchor is missing")]
    MissingAnchor,
    /// Input appointment end is not after start, or a resolved span is inverted.
    #[error("invalid agenda span")]
    InvalidSpan,
    /// A buffer is outside the SQL limits.
    #[error("buffers must be between 0 and 1440 minutes")]
    InvalidBuffers,
    /// Applying a buffer or correcting a collapsed floating span exceeds the UTC timestamp range.
    #[error("agenda timestamp is outside the supported range")]
    TimestampOverflow,
}

/// A finite `[starts_at, ends_at)` range, matching PostgreSQL `tstzrange(..., '[)')`.
///
/// Empty ranges are permitted and never overlap, just like PostgreSQL's `empty` range.
/// Agenda derivation corrects gap-collapsed floating appointments before constructing their busy range.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct BusyRange {
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
}

impl BusyRange {
    /// Construct a finite half-open range; reject an inverted span, permit empty spans.
    pub fn new(starts_at: DateTime<Utc>, ends_at: DateTime<Utc>) -> Result<Self, AgendaError> {
        if starts_at > ends_at {
            return Err(AgendaError::InvalidSpan);
        }
        Ok(Self { starts_at, ends_at })
    }

    /// Inclusive lower bound.
    pub const fn starts_at(self) -> DateTime<Utc> {
        self.starts_at
    }

    /// Exclusive upper bound.
    pub const fn ends_at(self) -> DateTime<Utc> {
        self.ends_at
    }

    /// Nonempty half-open overlap: back-to-back or empty ranges never conflict.
    pub fn overlaps(&self, other: &Self) -> bool {
        self.starts_at < self.ends_at
            && other.starts_at < other.ends_at
            && self.starts_at < other.ends_at
            && other.starts_at < self.ends_at
    }
}

/// Generated time columns for one member's `member_agenda` row (not an ACL).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct AgendaRow {
    /// Resolved appointment start, without buffers.
    pub starts_at: Option<DateTime<Utc>>,
    /// Resolved appointment end, without buffers.
    pub ends_at: Option<DateTime<Utc>>,
    /// Resolved deadline due time.
    pub due_at: Option<DateTime<Utc>>,
    /// Todo due date only; remains independent of the member's zone.
    pub due_date: Option<NaiveDate>,
    /// Buffered timed appointment range only; always absent for all-day appointments/todos/deadlines.
    pub busy: Option<BusyRange>,
}

/// Agenda membership is assignees ∪ subject, with owner fallback only if both are absent.
///
/// An unassigned dependent item appears only on the dependent's agenda, not the owner's.
/// Subject role is explicit for the caller but does not change the union rule.
pub fn agenda_members(
    owner: Uuid,
    assignees: &[Uuid],
    subject: Option<(Uuid, Role)>,
) -> BTreeSet<Uuid> {
    let mut members: BTreeSet<_> = assignees.iter().copied().collect();
    if let Some((subject_id, _role)) = subject {
        members.insert(subject_id);
    }
    if members.is_empty() {
        members.insert(owner);
    }
    members
}

/// Derive one member's times, using shared fold→earlier / gap→next-valid-wall-time resolution.
///
/// Fixed instants pass through unchanged. Only todos carry `due_date`; only timed appointments
/// carry busy ranges. Buffers affect busy, not displayed start/end. Invalid required anchors,
/// spans, buffers and timestamp overflow return errors rather than constructing invalid ranges.
/// If floating resolution collapses or inverts the interval, keep the resolved start and add the
/// positive wall-clock duration to obtain the end; buffers are then applied to the corrected span.
pub fn agenda_row(
    anchor: &Anchor,
    kind: Kind,
    time_mode: TimeMode,
    all_day: bool,
    buffers: Buffers,
    member_tz: Tz,
) -> Result<AgendaRow, AgendaError> {
    if !(0..=1440).contains(&buffers.before_min) || !(0..=1440).contains(&buffers.after_min) {
        return Err(AgendaError::InvalidBuffers);
    }
    let mut row = AgendaRow::default();
    match kind {
        Kind::Todo => row.due_date = anchor.due_date,
        Kind::Deadline => {
            row.due_at = Some(match time_mode {
                TimeMode::Fixed => anchor.due_at.ok_or(AgendaError::MissingAnchor)?,
                TimeMode::Floating => resolve_local(
                    anchor.local_due.ok_or(AgendaError::MissingAnchor)?,
                    member_tz,
                ),
            });
        }
        Kind::Appointment => {
            let (start, end) = match time_mode {
                TimeMode::Fixed => {
                    let start = anchor.starts_at.ok_or(AgendaError::MissingAnchor)?;
                    let end = anchor.ends_at.ok_or(AgendaError::MissingAnchor)?;
                    if start >= end {
                        return Err(AgendaError::InvalidSpan);
                    }
                    (start, end)
                }
                TimeMode::Floating => {
                    let start = anchor.local_start.ok_or(AgendaError::MissingAnchor)?;
                    let end = anchor.local_end.ok_or(AgendaError::MissingAnchor)?;
                    resolve_span(start, end, member_tz).map_err(|error| match error {
                        ResolveSpanError::InvalidSpan => AgendaError::InvalidSpan,
                        ResolveSpanError::TimestampOverflow => AgendaError::TimestampOverflow,
                    })?
                }
            };
            if start > end {
                return Err(AgendaError::InvalidSpan);
            }
            row.starts_at = Some(start);
            row.ends_at = Some(end);
            if !all_day {
                let before = Duration::minutes(i64::from(buffers.before_min));
                let after = Duration::minutes(i64::from(buffers.after_min));
                row.busy = Some(BusyRange::new(
                    start
                        .checked_sub_signed(before)
                        .ok_or(AgendaError::TimestampOverflow)?,
                    end.checked_add_signed(after)
                        .ok_or(AgendaError::TimestampOverflow)?,
                )?);
            }
        }
    }
    // s8b: accepting EDF is an occurrence-only span, not a kind/series edit.
    if kind != Kind::Appointment && (anchor.local_start.is_some() || anchor.local_end.is_some()) {
        let span_mode = if anchor.starts_at.is_some() && anchor.ends_at.is_some() {
            TimeMode::Fixed
        } else {
            TimeMode::Floating
        };
        let scheduled = agenda_row(
            anchor,
            Kind::Appointment,
            span_mode,
            false,
            buffers,
            member_tz,
        )?;
        row.starts_at = scheduled.starts_at;
        row.ends_at = scheduled.ends_at;
        row.busy = scheduled.busy;
    }
    Ok(row)
}

/// Identified per-member agenda row for in-memory hard-conflict checks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemberAgendaRow {
    /// Member whose authoritative time zone resolved this row.
    pub member_id: Uuid,
    /// Occurrence identity; only equal occurrence IDs are excluded from conflicts.
    pub occurrence_id: Uuid,
    /// Item identity; different occurrences of the same item may still conflict.
    pub item_id: Uuid,
    /// Generated agenda times and busy range.
    pub row: AgendaRow,
}

/// Return sorted, deduplicated `(new_occurrence, other_occurrence)` pairs for `member`.
///
/// Compares each new row with existing rows and with the other new rows, never with itself.
/// Rows for other members and missing/empty busy ranges do not conflict. Different occurrences
/// of the same item do, even in a newly created series with no existing rows. A new/new overlap
/// yields both directed pairs, one warning per new occurrence; duplicate input rows add no pairs.
/// Replace/exclude old versions of the new occurrences before calling; this function cannot tell
/// whether an existing row is a stale version. Visibility/title redaction belongs to the read layer.
pub fn hard_conflicts(
    member: Uuid,
    new_rows: &[MemberAgendaRow],
    existing_rows: &[MemberAgendaRow],
) -> Vec<(Uuid, Uuid)> {
    let mut conflicts = BTreeSet::new();
    for new in new_rows.iter().filter(|row| row.member_id == member) {
        let Some(new_busy) = new.row.busy else {
            continue;
        };
        for other in existing_rows
            .iter()
            .chain(new_rows.iter())
            .filter(|row| row.member_id == member && row.occurrence_id != new.occurrence_id)
        {
            if other.row.busy.is_some_and(|busy| new_busy.overlaps(&busy)) {
                conflicts.insert((new.occurrence_id, other.occurrence_id));
            }
        }
    }
    conflicts.into_iter().collect()
}
