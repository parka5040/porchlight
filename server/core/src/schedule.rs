//! Pure, deterministic planning math. All ranges are half-open [start, end).
//! Windows are anchored to a local weekday. end < start means overnight; equal
//! endpoints mean empty, not 24 hours. Overlapping/adjacent windows are unioned.
//! Available/load minutes are elapsed minutes (a DST day can have 23 or 25 hours).
//! A date deadline includes the whole date in the member's zone; no deadline sorts
//! last. EDF never moves existing work and does not backtrack when a task cannot fit.
use crate::time::{resolve_local, start_of_day, to_local};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Timelike, Utc, Weekday};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailabilityWindow {
    pub weekday: Weekday,
    pub start: NaiveTime,
    pub end: NaiveTime,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}
impl TimeRange {
    pub fn overlaps(self, other: Self) -> bool {
        self.start < self.end
            && other.start < other.end
            && self.start < other.end
            && other.start < self.end
    }
}
/// Used for both EDF members and dependent-assignee candidates. Zone is a parsed
/// IANA `chrono_tz::Tz`; busy ranges already include any appointment buffers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberSchedule {
    pub member_id: Uuid,
    pub tz: Tz,
    pub windows: Vec<AvailabilityWindow>,
    pub busy_ranges: Vec<TimeRange>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Deadline {
    Instant(DateTime<Utc>),
    Date(NaiveDate),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub member_id: Uuid,
    pub est_minutes: u32,
    pub deadline: Option<Deadline>,
    pub priority: i16,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    pub task_id: Uuid,
    pub member_id: Uuid,
    pub range: TimeRange,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnplacedReason {
    UnknownMember,
    InvalidEstimate,
    InvalidDeadline,
    NoSlotBeforeDeadline,
    DuplicateTask,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnplacedTask {
    pub task_id: Uuid,
    pub reason: UnplacedReason,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleResult {
    pub suggestions: Vec<Suggestion>,
    pub unplaced: Vec<UnplacedTask>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyLoad {
    pub date: NaiveDate,
    pub planned_minutes: u64,
    pub available_minutes: u64,
    pub overload_minutes: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateReason {
    FreeInsideAvailability,
    FreeOutsideAvailability,
    Conflicting,
    InvalidAppointment,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependentCandidate {
    pub member_id: Uuid,
    pub reason: CandidateReason,
}

fn union(mut ranges: Vec<TimeRange>) -> Vec<TimeRange> {
    ranges.retain(|r| r.start < r.end);
    ranges.sort_by_key(|r| (r.start, r.end));
    let mut out: Vec<TimeRange> = Vec::new();
    for r in ranges {
        if let Some(last) = out.last_mut()
            && r.start <= last.end
        {
            last.end = last.end.max(r.end);
            continue;
        }
        out.push(r);
    }
    out
}
fn subtract(slots: &[TimeRange], busy: &[TimeRange]) -> Vec<TimeRange> {
    let busy = union(busy.to_vec());
    let mut out = Vec::new();
    for slot in slots {
        let mut cursor = slot.start;
        for b in &busy {
            if b.end <= cursor {
                continue;
            }
            if b.start >= slot.end {
                break;
            }
            if cursor < b.start {
                out.push(TimeRange {
                    start: cursor,
                    end: b.start.min(slot.end),
                });
            }
            cursor = cursor.max(b.end);
            if cursor >= slot.end {
                break;
            }
        }
        if cursor < slot.end {
            out.push(TimeRange {
                start: cursor,
                end: slot.end,
            });
        }
    }
    out
}

/// Resolve each window on each calendar date with the project's DST policy;
/// clip to [from,until), union availability, then subtract unioned busy ranges.
/// Reversed/empty query or busy ranges are treated as empty. Overnight windows
/// beginning on the previous local date are included.
pub fn free_slots(
    windows: &[AvailabilityWindow],
    busy_ranges: &[TimeRange],
    from: DateTime<Utc>,
    until: DateTime<Utc>,
    tz: Tz,
) -> Vec<TimeRange> {
    if from >= until || windows.is_empty() {
        return Vec::new();
    }
    let first = to_local(from, tz).date();
    let last = to_local(until, tz).date();
    let mut date = first.pred_opt().unwrap_or(first);
    let mut ranges = Vec::new();
    while date <= last {
        for w in windows
            .iter()
            .filter(|w| w.weekday == date.weekday() && w.start != w.end)
        {
            let end_date = if w.end < w.start {
                match date.succ_opt() {
                    Some(d) => d,
                    None => continue,
                }
            } else {
                date
            };
            let start = resolve_local(date.and_time(w.start), tz).max(from);
            let end = resolve_local(end_date.and_time(w.end), tz).min(until);
            if start < end {
                ranges.push(TimeRange { start, end });
            }
        }
        match date.succ_opt() {
            Some(next) => date = next,
            None => break,
        }
    }
    subtract(&union(ranges), busy_ranges)
}
fn deadline(
    task: &Task,
    member: Option<&MemberSchedule>,
) -> Result<Option<DateTime<Utc>>, UnplacedReason> {
    match task.deadline {
        None => Ok(None),
        Some(Deadline::Instant(t)) => Ok(Some(t)),
        Some(Deadline::Date(d)) => {
            let tz = member.ok_or(UnplacedReason::UnknownMember)?.tz;
            let next = d.succ_opt().ok_or(UnplacedReason::InvalidDeadline)?;
            Ok(Some(start_of_day(next, tz)))
        }
    }
}

/// Strictly next member-local quarter-hour boundary. Resolve gaps/folds using
/// the shared policy; an earlier fold resolution is advanced until it is future.
pub fn proposal_start(now: DateTime<Utc>, tz: Tz) -> DateTime<Utc> {
    let local = to_local(now, tz);
    let floor = local.with_second(0).unwrap().with_nanosecond(0).unwrap();
    let mut boundary =
        floor.checked_add_signed(Duration::minutes(i64::from(15 - local.minute() % 15)));
    while let Some(wall) = boundary {
        let instant = resolve_local(wall, tz);
        if instant > now {
            return instant;
        }
        boundary = wall.checked_add_signed(Duration::minutes(15));
    }
    DateTime::<Utc>::MAX_UTC
}

/// Greedy EDF, deadline ascending, priority descending, UUID ascending. Missing
/// deadlines sort last. Starts are no earlier than the strictly next 15-minute
/// boundary after `from` in each member's zone. Each placed task consumes the slot before the next task.
/// Inputs should contain unique member IDs; duplicate task IDs are all rejected
/// rather than creating multiple proposals for the same task.
pub fn suggest_edf(
    tasks: &[Task],
    members: &[MemberSchedule],
    from: DateTime<Utc>,
    until: DateTime<Utc>,
) -> ScheduleResult {
    suggest_edf_in_zones(tasks, members, from, until, &BTreeMap::new())
}

/// Fixed tasks encode overrides in their item zone; other tasks use the member zone.
/// Keep the existing EDF entry point for callers whose tasks all follow member time.
pub fn suggest_edf_in_zones(
    tasks: &[Task],
    members: &[MemberSchedule],
    from: DateTime<Utc>,
    until: DateTime<Utc>,
    override_zones: &BTreeMap<Uuid, Tz>,
) -> ScheduleResult {
    let members: BTreeMap<_, _> = members.iter().map(|m| (m.member_id, m)).collect();
    let mut slots: BTreeMap<_, _> = members
        .iter()
        .map(|(id, m)| {
            (
                *id,
                free_slots(
                    &m.windows,
                    &m.busy_ranges,
                    proposal_start(from, m.tz),
                    until,
                    m.tz,
                ),
            )
        })
        .collect();
    let mut counts = BTreeMap::new();
    for t in tasks {
        *counts.entry(t.id).or_insert(0usize) += 1;
    }
    let mut tasks: Vec<_> = tasks
        .iter()
        .map(|t| (t, deadline(t, members.get(&t.member_id).copied())))
        .collect();
    tasks.sort_by(|(a, da), (b, db)| {
        // Some deadline before None or invalid; total order even for invalid inputs.
        let key = |d: &Result<Option<DateTime<Utc>>, UnplacedReason>| {
            d.as_ref()
                .ok()
                .copied()
                .flatten()
                .map(|t| (false, t))
                .unwrap_or((true, DateTime::<Utc>::MAX_UTC))
        };
        key(da)
            .cmp(&key(db))
            .then_with(|| b.priority.cmp(&a.priority))
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.member_id.cmp(&b.member_id))
            .then_with(|| a.est_minutes.cmp(&b.est_minutes))
    });
    let mut result = ScheduleResult::default();
    for (task, deadline) in tasks {
        let reason = if counts[&task.id] > 1 {
            Some(UnplacedReason::DuplicateTask)
        } else if task.est_minutes == 0 {
            Some(UnplacedReason::InvalidEstimate)
        } else if !members.contains_key(&task.member_id) {
            Some(UnplacedReason::UnknownMember)
        } else {
            deadline.as_ref().err().copied()
        };
        if let Some(reason) = reason {
            result.unplaced.push(UnplacedTask {
                task_id: task.id,
                reason,
            });
            continue;
        }
        let limit = deadline.ok().flatten().unwrap_or(until).min(until);
        let duration = Duration::minutes(i64::from(task.est_minutes));
        let free = slots.get_mut(&task.member_id).expect("known member");
        let zone = override_zones
            .get(&task.id)
            .copied()
            .unwrap_or(members[&task.member_id].tz);
        let placement = free.iter().enumerate().find_map(|(i, s)| {
            let mut start = s.start;
            loop {
                let end = start.checked_add_signed(duration)?;
                if end > s.end || end > limit {
                    return None;
                }
                let range = TimeRange { start, end };
                if representable_override(range, zone) {
                    return Some((i, range));
                }
                // Search whole-second boundaries: availability and busy endpoints
                // can carry seconds, and a fold can leave exactly one valid slot.
                start = start
                    .checked_add_signed(Duration::seconds(1))?
                    .with_nanosecond(0)?;
            }
        });
        if let Some((i, range)) = placement {
            result.suggestions.push(Suggestion {
                task_id: task.id,
                member_id: task.member_id,
                range,
            });
            let prefix = (free[i].start < range.start).then_some(TimeRange {
                start: free[i].start,
                end: range.start,
            });
            free[i].start = range.end;
            if free[i].start == free[i].end {
                free.remove(i);
            }
            if let Some(prefix) = prefix {
                free.insert(i, prefix);
            }
        } else {
            result.unplaced.push(UnplacedTask {
                task_id: task.id,
                reason: UnplacedReason::NoSlotBeforeDeadline,
            });
        }
    }
    result
}

/// Exact round trip through the occurrence override policy (earlier fold branch).
/// Used both when proposing slots and when accepting them.
pub fn representable_override(range: TimeRange, zone: Tz) -> bool {
    let start = to_local(range.start, zone);
    let end = to_local(range.end, zone);
    start < end
        && resolve_local(start, zone) == range.start
        && resolve_local(end, zone) == range.end
}

/// `tasks_by_day` is planned minutes already attributed to the member's local
/// dates by the caller. Output has exactly these dates, including zero-work dates
/// if the caller inserts them. Availability counts unioned elapsed whole minutes,
/// not free capacity after busy ranges; midnight-spanning windows are clipped.
pub fn daily_load(
    tasks_by_day: &BTreeMap<NaiveDate, u64>,
    windows: &[AvailabilityWindow],
    tz: Tz,
) -> Vec<DailyLoad> {
    tasks_by_day
        .iter()
        .map(|(date, planned)| {
            let available = date
                .succ_opt()
                .map(|next| {
                    free_slots(
                        windows,
                        &[],
                        start_of_day(*date, tz),
                        start_of_day(next, tz),
                        tz,
                    )
                    .iter()
                    .map(|r| (r.end - r.start).num_seconds().max(0) as u64)
                    .sum::<u64>()
                        / 60
                })
                .unwrap_or(0);
            DailyLoad {
                date: *date,
                planned_minutes: *planned,
                available_minutes: available,
                overload_minutes: planned.saturating_sub(available),
            }
        })
        .collect()
}

/// Rank by availability tier, then UUID. An appointment must be fully covered by
/// unioned availability to rank inside (partial coverage counts as outside).
/// Touching endpoints do not conflict. Invalid appointment → explicit reason.
pub fn dependent_candidates(
    appointment: TimeRange,
    candidates: &[MemberSchedule],
) -> Vec<DependentCandidate> {
    let mut out: Vec<_> = candidates
        .iter()
        .map(|c| {
            let reason = if appointment.start >= appointment.end {
                CandidateReason::InvalidAppointment
            } else if c.busy_ranges.iter().any(|b| b.overlaps(appointment)) {
                CandidateReason::Conflicting
            } else if free_slots(&c.windows, &[], appointment.start, appointment.end, c.tz)
                .iter()
                .any(|r| r.start <= appointment.start && r.end >= appointment.end)
            {
                CandidateReason::FreeInsideAvailability
            } else {
                CandidateReason::FreeOutsideAvailability
            };
            DependentCandidate {
                member_id: c.member_id,
                reason,
            }
        })
        .collect();
    out.sort_by_key(|c| (c.reason, c.member_id));
    out
}
