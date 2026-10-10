use super::*;
use crate::time::{resolve_local, resolve_span, start_of_day, to_local};
use chrono::{Duration, Months, NaiveTime};
use rrule::{RRule, RRuleSet, Unvalidated};

/// Calendar materialization horizon, in calendar months rather than fixed-length days.
pub const HORIZON_MONTHS: u32 = 18;

/// Generated occurrence columns. Local anchors always survive gap/fold resolution.
/// All-day appointments use midnight local start and exclusive midnight local end.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OccurrenceAnchor {
    /// Stable zero-based index within this series (tail indices restart at zero).
    pub seq: u32,
    /// Nominal appointment start, retained through gap resolution.
    pub local_start: Option<NaiveDateTime>,
    /// Nominal exclusive end; seq 0 may cross a fold with reversed wall readings.
    pub local_end: Option<NaiveDateTime>,
    /// Nominal deadline wall time.
    pub local_due: Option<NaiveDateTime>,
    /// Todo due date.
    pub due_date: Option<NaiveDate>,
    /// Fixed appointment start instant; None for floating.
    pub starts_at: Option<DateTime<Utc>>,
    /// Fixed appointment exclusive end instant; None for floating.
    pub ends_at: Option<DateTime<Utc>>,
    /// Fixed deadline instant; None for floating.
    pub due_at: Option<DateTime<Utc>>,
}
impl OccurrenceAnchor {
    /// Primary wall-clock anchor used for window membership (dates mean midnight).
    pub fn local_key(&self) -> Option<NaiveDateTime> {
        self.due_date
            .map(|d| d.and_time(NaiveTime::MIN))
            .or(self.local_due)
            .or(self.local_start)
    }
}

/// Ends the 18-month horizon at the same local clock time, clamping month end.
pub fn horizon_end(now: DateTime<Utc>, tz: Tz) -> Result<DateTime<Utc>, ValidationError> {
    validate_local("now", now.naive_utc())?;
    let end = to_local(now, tz)
        .checked_add_months(Months::new(HORIZON_MONTHS))
        .ok_or_else(|| ValidationError::new("horizon", "date overflow"))?;
    validate_local("horizon", end)?;
    Ok(resolve_local(end, tz))
}

/// Expands a half-open local-wall-clock window [from, until). Seqs index the entire
/// series, not the requested window. For split tails seqs restart at 0 (see cursor).
/// Non-recurring dated items yield their single anchor iff it falls in the window;
/// an undated todo yields seq 0 in any nonempty window. Completion items only yield
/// their initial anchor: persistence supplies the current open successor separately.
/// Errors never return a partial or silently truncated expansion.
pub fn expand(
    spec: &SeriesSpec,
    from: NaiveDateTime,
    until: NaiveDateTime,
) -> Result<Vec<OccurrenceAnchor>, ValidationError> {
    validate_local("from", from)?;
    validate_local("until", until)?;
    if from > until {
        return Err(ValidationError::new(
            "until",
            "window ends before it starts",
        ));
    }
    if from == until {
        return Ok(vec![]);
    }
    if spec.rule.is_none() {
        return match spec.start() {
            None => Ok(vec![OccurrenceAnchor::default()]),
            Some(t) if t >= from && t < until => Ok(vec![initial_anchor(spec, t)?]),
            _ => Ok(vec![]),
        };
    }
    let mut result = vec![];
    visit(spec, until, |seq, local| {
        if local >= from {
            result.push(if seq == 0 {
                initial_anchor(spec, local)?
            } else {
                anchor_for_local(spec, local, seq)?
            });
        }
        Ok(true)
    })?;
    Ok(result)
}

/// Counts materialized anchors from DTSTART up to `until`, stopping at `limit + 1`.
/// Uses the same recurrence visitor as [`expand`] without allocating anchors. A
/// result larger than `limit` means the series exceeds the caller's work budget.
/// Count history too: persistence expands from DTSTART, not the import cache window.
pub fn bounded_occurrence_count(
    spec: &SeriesSpec,
    until: NaiveDateTime,
    limit: usize,
) -> Result<usize, ValidationError> {
    validate_local("until", until)?;
    if spec.rule.is_none() {
        return Ok(usize::from(spec.start().is_none_or(|start| start < until)));
    }
    let mut count = 0;
    visit(spec, until, |_, _| {
        count += 1;
        Ok(count <= limit)
    })?;
    Ok(count)
}

// rrule iterates in synthetic UTC so it cannot drop gap times or change wall duration.
// Iterate a nonempty superset of active calendar periods, then apply filters.
// rrule checks UNTIL only on candidates; retaining contradictory filters inside
// its iterator would scan to year 10000. This superset always produces candidates
// in each active period, so even empty filtered periods stop at the query horizon.
// Do not use all(u16), whose limit truncates stable seqs.
fn visit(
    spec: &SeriesSpec,
    end: NaiveDateTime,
    mut callback: impl FnMut(u32, NaiveDateTime) -> Result<bool, ValidationError>,
) -> Result<(), ValidationError> {
    let rule = spec
        .rule
        .as_ref()
        .ok_or_else(|| ValidationError::new("rrule", "not a calendar series"))?;
    let dtstart = spec
        .start()
        .ok_or_else(|| ValidationError::new("due_date", "missing DTSTART"))?;
    if end <= dtstart {
        return Ok(());
    }
    let synthetic_start = dtstart
        .with_nanosecond(0)
        .unwrap()
        .and_utc()
        .with_timezone(&rrule::Tz::UTC);
    let synthetic_end = end.and_utc().with_timezone(&rrule::Tz::UTC);
    let parsed: RRule<Unvalidated> = rule
        .scan_pattern()
        .parse()
        .map_err(|e| ValidationError::new("rrule", format!("{e}")))?;
    let validated = parsed
        .until(synthetic_end)
        .validate(synthetic_start)
        .map_err(|e| ValidationError::new("rrule", format!("{e}")))?;
    let set = RRuleSet::new(synthetic_start)
        .rrule(validated)
        .rdate(synthetic_start);
    // A full Gregorian/weekday cycle with no matches proves the active pattern
    // empty forever. This also bounds seq lookup (whose search end is year 10000).
    // One extra period permits a partial DTSTART period without a false conclusion.
    let empty_cycle = match rule.frequency() {
        "DAILY" => 7,
        "WEEKLY" => 1,
        "MONTHLY" => 4800,
        "YEARLY" => 400,
        _ => unreachable!("validated frequency"),
    };
    let mut previous_period = None;
    let mut empty_periods = 0;
    let mut previous = None;
    let mut index = 0u32;
    for instant in &set {
        let t = instant
            .naive_utc()
            .with_nanosecond(dtstart.nanosecond())
            .unwrap();
        if t >= end {
            break;
        }
        // rrule 0.14 yields both RDATE and RRULE when DTSTART matches. Union once.
        if previous == Some(t) {
            continue;
        }
        previous = Some(t);
        let period = match rule.frequency() {
            "MONTHLY" => (t.year(), t.month(), 0),
            "YEARLY" => (t.year(), 0, 0),
            "WEEKLY" => (t.iso_week().year(), t.iso_week().week(), 0),
            _ => (t.year(), t.month(), t.day()),
        };
        if previous_period != Some(period) {
            previous_period = Some(period);
            empty_periods += 1;
            if empty_periods > empty_cycle + 1 {
                break;
            }
        }
        if t != dtstart && !rule.matches_date(t.date(), dtstart.date()) {
            continue;
        }
        empty_periods = 0;
        let seq = index;
        index = index
            .checked_add(1)
            .ok_or_else(|| ValidationError::new("seq", "sequence overflow"))?;
        if rule.count.is_some_and(|count| seq >= count) || !within_until(spec, t) {
            break;
        }
        if !callback(seq, t)? {
            break;
        }
    }
    Ok(())
}
pub(super) fn within_until(spec: &SeriesSpec, t: NaiveDateTime) -> bool {
    match spec.rule.as_ref().and_then(|r| r.until.as_ref()) {
        None => true,
        Some(Until::Date(d)) => t.date() <= *d,
        Some(Until::Utc(bound)) => {
            let instant =
                if spec.input.time_mode == TimeMode::Fixed && spec.input.kind != Kind::Todo {
                    if t == spec.start().unwrap() {
                        spec.input.due_at.or(spec.input.starts_at).unwrap()
                    } else {
                        resolve_local(t, spec.zone.unwrap())
                    }
                } else {
                    t.and_utc()
                };
            instant <= *bound
        }
    }
}
/// The authoritative initial occurrence, independent of the calendar expansion horizon.
/// Use this for non-recurring items and the first completion-mode occurrence.
pub fn first_anchor(spec: &SeriesSpec) -> Result<OccurrenceAnchor, ValidationError> {
    anchor_at_seq(spec, 0)
}

pub(super) fn anchor_at_seq(
    spec: &SeriesSpec,
    target: u32,
) -> Result<OccurrenceAnchor, ValidationError> {
    if spec.rule.is_none() {
        if target == 0 {
            return spec
                .start()
                .map_or(Ok(OccurrenceAnchor::default()), |t| initial_anchor(spec, t));
        } else {
            return Err(ValidationError::new(
                "seq",
                "non-recurring item has only seq 0",
            ));
        }
    }
    let mut found = None;
    visit(
        spec,
        // Internal exclusive sentinel, so even the last supported second is searchable.
        NaiveDate::from_ymd_opt(10000, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap(),
        |seq, t| {
            if seq == target {
                found = Some(if seq == 0 {
                    initial_anchor(spec, t)?
                } else {
                    anchor_for_local(spec, t, seq)?
                });
                return Ok(false);
            }
            Ok(true)
        },
    )?;
    found.ok_or_else(|| ValidationError::new("seq", "sequence does not exist"))
}
// Seq 0 is input, not a generated ambiguous wall time. Preserve authoritative
// fixed instants and their wall readings (even a fold-crossing local reversal).
fn initial_anchor(
    spec: &SeriesSpec,
    t: NaiveDateTime,
) -> Result<OccurrenceAnchor, ValidationError> {
    if spec.input.time_mode != TimeMode::Fixed || spec.input.kind == Kind::Todo {
        return anchor_for_local(spec, t, 0);
    }
    let mut anchor = OccurrenceAnchor {
        due_at: spec.input.due_at,
        starts_at: spec.input.starts_at,
        ends_at: spec.input.ends_at,
        ..Default::default()
    };
    match spec.input.kind {
        Kind::Deadline => anchor.local_due = Some(t),
        Kind::Appointment => {
            anchor.local_start = Some(t);
            anchor.local_end = Some(if spec.cursor.is_some() {
                t.checked_add_signed(spec.wall_duration().unwrap())
                    .ok_or_else(|| ValidationError::new("local_end", "date overflow"))?
            } else {
                spec.local_end().unwrap()
            });
        }
        Kind::Todo => unreachable!(),
    }
    Ok(anchor)
}
pub(super) fn anchor_for_local(
    spec: &SeriesSpec,
    t: NaiveDateTime,
    seq: u32,
) -> Result<OccurrenceAnchor, ValidationError> {
    validate_local("anchor", t)?;
    let mut anchor = OccurrenceAnchor {
        seq,
        ..Default::default()
    };
    match spec.input.kind {
        Kind::Todo => anchor.due_date = Some(t.date()),
        Kind::Deadline => {
            anchor.local_due = Some(t);
            if spec.input.time_mode == TimeMode::Fixed {
                anchor.due_at = Some(resolve_local(t, spec.zone.unwrap()));
            }
        }
        Kind::Appointment => {
            let duration = spec.wall_duration().unwrap();
            let end = t
                .checked_add_signed(duration)
                .ok_or_else(|| ValidationError::new("local_end", "date overflow"))?;
            validate_local("local_end", end)?;
            anchor.local_start = Some(t);
            anchor.local_end = Some(end);
            if spec.input.time_mode == TimeMode::Fixed {
                let (start, end) = resolve_span(t, end, spec.zone.unwrap())
                    .map_err(|e| ValidationError::new("local_end", e.to_string()))?;
                anchor.starts_at = Some(start);
                anchor.ends_at = Some(end);
            }
        }
    }
    Ok(anchor)
}

/// Returns successor anchors (seq 0 placeholder; S3 assigns previous seq + 1).
/// Months clamp the completion's local day, then days advance on the local calendar;
/// after fold/gap resolution, microseconds advance elapsed time (Postgres semantics).
/// Fixed uses the item zone, floating uses the completing member's zone. Todo due dates
/// start at midnight of the completion's local date; undated todos stay undated.
/// A deadline's successor due is this result; an appointment starts at it and keeps its
/// wall-clock duration. All-day appointments use midnight of the resulting local date.
/// Local calendar targets use the shared earlier-fold/next-valid-gap rule. Elapsed
/// arithmetic preserves a *known* instant, even when it lands on the later side of a
/// fold (it is not an ambiguous local-time resolution). Floating returns only that
/// instant's local clock reading; subsequent per-member resolution uses the usual rules.
/// Mixed-sign intervals that do not advance the completion are typed errors.
/// Persistence must enforce exactly one open occurrence transactionally.
pub fn next_after_completion(
    spec: &SeriesSpec,
    completed_at: DateTime<Utc>,
    member_tz: Tz,
) -> Result<OccurrenceAnchor, ValidationError> {
    if spec.input.recurrence_mode != Some(RecurrenceMode::Completion) {
        return Err(ValidationError::new(
            "recurrence_mode",
            "not a completion series",
        ));
    }
    validate_local("completed_at", completed_at.naive_utc())?;
    if spec.input.kind == Kind::Todo && spec.input.due_date.is_none() {
        return Ok(OccurrenceAnchor::default());
    }
    let zone = if spec.input.time_mode == TimeMode::Fixed {
        spec.zone.unwrap_or(member_tz)
    } else {
        member_tz
    };
    let mut local = to_local(completed_at, zone);
    if spec.input.kind == Kind::Todo || spec.input.all_day && spec.input.kind == Kind::Appointment {
        local = local.date().and_time(NaiveTime::MIN);
    }
    let interval = spec.input.completion_interval.unwrap();
    local = if interval.months < 0 {
        local.checked_sub_months(Months::new(interval.months.unsigned_abs()))
    } else {
        local.checked_add_months(Months::new(interval.months as u32))
    }
    .ok_or_else(|| ValidationError::new("completion_interval", "month overflow"))?;
    local = local
        .checked_add_signed(Duration::days(i64::from(interval.days)))
        .ok_or_else(|| ValidationError::new("completion_interval", "day overflow"))?;
    validate_local("completion_interval", local)?;
    let date_only =
        spec.input.kind == Kind::Todo || spec.input.all_day && spec.input.kind == Kind::Appointment;
    // With no calendar components, completion is already a known instant. Resolving
    // its local reading again would incorrectly move a late-fold completion an hour back.
    let calendar_instant = if interval.months == 0 && interval.days == 0 && !date_only {
        completed_at
    } else {
        resolve_local(local, zone)
    };
    let instant = calendar_instant
        .checked_add_signed(Duration::microseconds(interval.microseconds))
        .ok_or_else(|| ValidationError::new("completion_interval", "microsecond overflow"))?;
    validate_local("completion_interval", instant.naive_utc())?;
    // Calendar-only intervals retain their nominal local anchor, even in a gap.
    // Elapsed microseconds instead produce a new local clock reading in this zone.
    if interval.microseconds != 0 {
        local = to_local(instant, zone);
    }
    if spec.input.all_day && spec.input.kind == Kind::Appointment {
        local = local.date().and_time(NaiveTime::MIN);
    }
    let mut anchor = anchor_for_local(spec, local, 0)?;
    if date_only {
        if local.date() <= to_local(completed_at, zone).date() {
            return Err(ValidationError::new(
                "completion_interval",
                "date-only successor must advance the completion date",
            ));
        }
    } else if instant <= completed_at {
        return Err(ValidationError::new(
            "completion_interval",
            "interval does not advance completion after month/DST normalization",
        ));
    }
    if !date_only && spec.input.time_mode == TimeMode::Fixed && interval.microseconds != 0 {
        match spec.input.kind {
            Kind::Deadline => anchor.due_at = Some(instant),
            Kind::Appointment => {
                anchor.starts_at = Some(instant);
                if anchor.ends_at.is_some_and(|end| end <= instant) {
                    // Both bounds are in the same fold, but the known start is on its
                    // later branch. Advancing its wall duration stays on that branch.
                    anchor.ends_at = Some(
                        instant
                            .checked_add_signed(spec.wall_duration().unwrap())
                            .ok_or_else(|| ValidationError::new("ends_at", "date overflow"))?,
                    );
                }
            }
            Kind::Todo => unreachable!("date-only above"),
        }
    }
    Ok(anchor)
}

/// An occurrence plus authoritative status. Closed statuses do not participate in missed policies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenOccurrence {
    /// Effective generated or overridden occurrence anchors.
    pub anchor: OccurrenceAnchor,
    /// Unshifted generated anchor for a detached occurrence. Carry-over treats a row
    /// detached later than its passed generated boundary as postponed carried work.
    pub generated_anchor: Option<OccurrenceAnchor>,
    /// Authoritative occurrence status; closed rows are excluded.
    pub status: Status,
    /// Previous generated disposition: carry-over retains an overdue row and ranks
    /// non-expired work above expired work (R63, derivation-matrix.md).
    pub disposition: Disposition,
    /// Closure instant of a closed row. Reopened rows retain the marker but are not closed evidence.
    pub closed_at: Option<DateTime<Utc>>,
}

/// Dispositions in input order; done/skipped rows return Open (caller preserves their
/// persisted disposition). Missed means strictly before now; a todo date expires at
/// next local midnight. Undated todos are never missed. Skip and stack are stateless.
/// Carry-over follows Ruling R63 (derivation-matrix.md, "R63 carry-over contract"): a
/// deterministic function of the current rows, independent of input order. Missed rows
/// covered by closed carried work or by postponed work expire; among the rest, a claim
/// (a retained overdue row or postponed work) with the oldest due-since carries, else
/// the latest non-expired, else the latest expired boundary, lowest seq on ties.
pub fn dispositions(
    occurrences: &[OpenOccurrence],
    now: DateTime<Utc>,
    policy: MissedPolicy,
    zone: Tz,
) -> Vec<Disposition> {
    // Use exactly the same zone/DST boundary for generated and overridden anchors.
    let boundary = |a: &OccurrenceAnchor| {
        a.ends_at
            .or_else(|| {
                a.local_end.and_then(|end| match a.local_start {
                    Some(start) => resolve_span(start, end, zone).ok().map(|(_, end)| end),
                    None => Some(resolve_local(end, zone)),
                })
            })
            .or(a.due_at)
            .or_else(|| a.local_due.map(|t| resolve_local(t, zone)))
            .or_else(|| {
                a.due_date
                    .and_then(|d| d.succ_opt())
                    .map(|d| start_of_day(d, zone))
            })
    };
    let effective: Vec<_> = occurrences.iter().map(|o| boundary(&o.anchor)).collect();
    let missed: Vec<_> = occurrences
        .iter()
        .zip(&effective)
        .map(|(o, t)| t.filter(|t| o.status.is_open() && *t < now))
        .collect();
    let carried = match policy {
        MissedPolicy::CarryOver => carry_over(occurrences, &effective, &missed, now, boundary),
        _ => None,
    };
    missed
        .iter()
        .enumerate()
        .map(|(i, t)| match (t, policy) {
            (None, _) => Disposition::Open,
            (Some(_), MissedPolicy::Stack) => Disposition::Overdue,
            (Some(_), MissedPolicy::CarryOver) if Some(i) == carried => Disposition::Overdue,
            _ => Disposition::Expired,
        })
        .collect()
}

/// R63 carry-over election over current persisted state (derivation-matrix.md, "R63
/// carry-over contract"); returns the index of the single overdue row, if any.
fn carry_over(
    occurrences: &[OpenOccurrence],
    effective: &[Option<DateTime<Utc>>],
    missed: &[Option<DateTime<Utc>>],
    now: DateTime<Utc>,
    boundary: impl Fn(&OccurrenceAnchor) -> Option<DateTime<Utc>>,
) -> Option<usize> {
    use std::cmp::Reverse;
    // Generated boundaries use exactly the same zone/DST resolution as effective ones.
    let generated: Vec<_> = occurrences
        .iter()
        .map(|o| o.generated_anchor.as_ref().and_then(&boundary))
        .collect();
    let disposition = |i: usize| occurrences[i].disposition;
    // Postponed work: detached later than its generated boundary, which passed by `at`.
    let postponed = |i: usize, at: DateTime<Utc>| {
        generated[i]
            .zip(effective[i])
            .is_some_and(|(g, e)| g < at && e > g)
    };
    // Due-since: the earlier of generated and effective boundary; seq breaks ties.
    let since = |i: usize| {
        let t = match (generated[i], effective[i]) {
            (Some(g), Some(e)) => Some(g.min(e)),
            (g, e) => e.or(g),
        };
        (t, occurrences[i].anchor.seq, i)
    };
    // Older postponed or non-expired work is never superseded by younger postponed work.
    // (Coverage is then transitive: a covered source never covers anything extra.)
    let protected = |p: usize, w: usize| {
        since(p) < since(w) && (postponed(p, now) || disposition(p) != Disposition::Expired)
    };
    // The overdue row persists until closed, even across older closures (reopen, stack era).
    let retained = |p: usize| missed[p].is_some() && disposition(p) == Disposition::Overdue;
    // Closed carried work covers rows missed before the closure, refreshed or not.
    // Overdue at closure: the carried row. Open postponed work closed early: also carried
    // work, except over older protected work. Expired closure is not evidence.
    let closed_overdue = occurrences
        .iter()
        .filter(|o| !o.status.is_open() && o.disposition == Disposition::Overdue)
        .filter_map(|o| o.closed_at)
        .max();
    let closed_postponed: Vec<_> = occurrences
        .iter()
        .enumerate()
        .filter(|(_, o)| !o.status.is_open() && o.disposition == Disposition::Open)
        .filter_map(|(c, o)| o.closed_at.filter(|at| postponed(c, *at)).map(|at| (c, at)))
        .collect();
    let closure_covered = |p: usize| {
        effective[p].is_some_and(|t| {
            !retained(p)
                && (closed_overdue.is_some_and(|at| t < at)
                    || closed_postponed
                        .iter()
                        .any(|&(c, at)| t < at && !protected(p, c)))
        })
    };
    // Postponed open work whose generated boundary passed covers younger or expired
    // missed rows before its new boundary; equality keeps them eligible.
    let sources: Vec<usize> = (0..occurrences.len())
        .filter(|&w| occurrences[w].status.is_open() && postponed(w, now) && !closure_covered(w))
        .collect();
    let covered = |p: usize| {
        closure_covered(p)
            || sources
                .iter()
                .any(|&w| w != p && effective[p] < effective[w] && !protected(p, w))
    };
    // Claims (retained or postponed) first, oldest due-since; then non-expired work,
    // then expired work, each by latest boundary with the lowest seq on ties.
    let rank = |i: usize| {
        let claim = retained(i) || postponed(i, now);
        let class = u8::from(claim) * 2 + u8::from(claim || disposition(i) != Disposition::Expired);
        let (newest, oldest) = if claim {
            (None, Some(Reverse(since(i))))
        } else {
            (missed[i], None)
        };
        (
            class,
            oldest,
            newest,
            Reverse((occurrences[i].anchor.seq, i)),
        )
    };
    (0..occurrences.len())
        .filter(|&i| missed[i].is_some() && !covered(i))
        .max_by_key(|&i| rank(i))
}
