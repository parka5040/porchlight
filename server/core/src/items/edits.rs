use super::engine::anchor_at_seq;
use super::*;

/// Authoritative local override columns for a single detached occurrence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OccurrenceOverride {
    /// Detached occurrence index.
    pub seq: u32,
    /// Complete nominal appointment start replacement.
    pub override_local_start: Option<NaiveDateTime>,
    /// Complete nominal exclusive appointment end replacement.
    pub override_local_end: Option<NaiveDateTime>,
    /// Complete nominal deadline due replacement.
    pub override_local_due: Option<NaiveDateTime>,
    /// Complete todo due-date replacement.
    pub override_due_date: Option<NaiveDate>,
}

/// Validates a complete replacement's local/date anchors, returning authoritative
/// override columns (resolved instant fields are ignored; never persisted as overrides).
/// Calendar seq existence is checked; completion seq existence belongs to persistence.
/// This is a complete timing override, not a sparse patch or a way to clear a due date.
pub fn this_occurrence(
    spec: &SeriesSpec,
    seq: u32,
    replacement: &OccurrenceAnchor,
) -> Result<OccurrenceOverride, ValidationError> {
    if spec.input.recurrence_mode != Some(RecurrenceMode::Completion) {
        anchor_at_seq(spec, seq)?;
    }
    persisted_occurrence_override(spec, seq, replacement)
}

/// Validates timing for an occurrence whose identity has already been verified by persistence.
/// Detached exceptions and closed history can survive a shortened/changed calendar rule;
/// their stored sequence need not belong to the current expansion. Callers must load/check
/// the occurrence's parent and version before using this function. Timing validation is
/// identical to [`this_occurrence`]; only current-rule membership is not checked.
pub fn persisted_occurrence_override(
    spec: &SeriesSpec,
    seq: u32,
    replacement: &OccurrenceAnchor,
) -> Result<OccurrenceOverride, ValidationError> {
    let mut input = spec.input.clone();
    input.rrule = None;
    input.recurrence_mode = None;
    input.completion_interval = None;
    input.due_at = None;
    input.starts_at = None;
    input.ends_at = None;
    input.due_date = replacement.due_date;
    input.local_due = replacement.local_due;
    input.local_start = replacement.local_start;
    input.local_end = replacement.local_end;
    if spec.input.time_mode == TimeMode::Fixed && spec.input.kind != Kind::Todo {
        let zone = spec.zone.unwrap();
        input.due_at = input.local_due.map(|t| crate::time::resolve_local(t, zone));
        if let (Some(start), Some(end)) = (input.local_start, input.local_end) {
            validate_local("local_start", start)?;
            validate_local("local_end", end)?;
            let (start, end) = crate::time::resolve_span(start, end, zone)
                .map_err(|e| ValidationError::new("local_end", e.to_string()))?;
            input.starts_at = Some(start);
            input.ends_at = Some(end);
        }
        input.local_due = None;
        input.local_start = None;
        input.local_end = None;
    }
    // s8b: scheduled work has an occurrence-only span, while the series keeps
    // its todo/deadline shape. Validate the original deadline independently.
    if spec.input.kind != Kind::Appointment {
        input.local_start = None;
        input.local_end = None;
        input.starts_at = None;
        input.ends_at = None;
        if replacement.local_start.is_some() != replacement.local_end.is_some() {
            return Err(ValidationError::new(
                "override_local_end",
                "scheduled work requires both span bounds",
            ));
        }
    }
    SeriesSpec::new(input)?;
    for (field, local) in [
        ("override_local_due", replacement.local_due),
        ("override_local_start", replacement.local_start),
        ("override_local_end", replacement.local_end),
    ] {
        if let Some(local) = local {
            validate_local(field, local)?;
        }
    }
    if replacement.local_key().is_none() {
        return Err(ValidationError::new(
            "override",
            "at least one date/local override is required",
        ));
    }
    // Check the wall bounds too (instant conversion alone can hide a fold reversal).
    if let (Some(start), Some(end)) = (replacement.local_start, replacement.local_end) {
        if start >= end {
            return Err(ValidationError::new(
                "override_local_end",
                "end must follow start",
            ));
        }
        if spec.input.all_day
            && (start.time() != chrono::NaiveTime::MIN || end.time() != chrono::NaiveTime::MIN)
        {
            return Err(ValidationError::new(
                "all_day",
                "all-day override requires midnight bounds",
            ));
        }
    }
    Ok(OccurrenceOverride {
        seq,
        override_local_start: replacement.local_start,
        override_local_end: replacement.local_end,
        override_local_due: replacement.local_due,
        override_due_date: replacement.due_date,
    })
}

/// Splits before at_seq: None head at seq 0 means 'all'. Completion recurrence cannot
/// split, including at seq 0. Head COUNT=at_seq (or date UNTIL for >9999 instances),
/// tail COUNT is reduced and DTSTART is the split instance. Only fixed tails whose
/// nominal anchors do not round-trip through their instants need [`CalendarCursor`].
/// Seqs in both specs start at 0; add at_seq to tail seqs to compare to the original.
pub fn this_and_future(
    spec: &SeriesSpec,
    at_seq: u32,
) -> Result<(Option<SeriesSpec>, SeriesSpec), ValidationError> {
    if spec.input.recurrence_mode == Some(RecurrenceMode::Completion) {
        return Err(ValidationError::new(
            "scope",
            "completion series have no this_and_future scope",
        ));
    }
    if at_seq == 0 {
        return Ok((None, spec.clone()));
    }
    let rule = spec.rule.as_ref().ok_or_else(|| {
        ValidationError::new("scope", "this_and_future requires calendar recurrence")
    })?;
    let split = anchor_at_seq(spec, at_seq)?;
    let mut head = spec.clone();
    let head_rule = head.rule.as_mut().unwrap();
    if at_seq <= 9999 {
        head_rule.count = Some(at_seq);
        head_rule.until = None;
    } else {
        head_rule.count = None;
        head_rule.until = Some(Until::Date(
            split
                .local_key()
                .unwrap()
                .date()
                .pred_opt()
                .ok_or_else(|| ValidationError::new("seq", "split date underflow"))?,
        ));
    }
    head.input.rrule = Some(head_rule.to_string());
    let mut tail_input = spec.input.clone();
    tail_input.due_date = split.due_date;
    match spec.input.time_mode {
        TimeMode::Fixed => {
            tail_input.due_at = split.due_at;
            tail_input.starts_at = split.starts_at;
            tail_input.ends_at = split.ends_at;
        }
        TimeMode::Floating => {
            tail_input.local_due = split.local_due;
            tail_input.local_start = split.local_start;
            tail_input.local_end = split.local_end;
        }
    }
    let mut tail_rule = rule.clone();
    tail_rule.count = rule.count.map(|count| count - at_seq);
    tail_input.rrule = Some(tail_rule.to_string());
    let mut cursor = None;
    if spec.input.time_mode == TimeMode::Fixed && spec.input.kind != Kind::Todo {
        let zone = spec.zone.unwrap();
        let needs_anchor = split
            .local_due
            .zip(split.due_at)
            .is_some_and(|(local, instant)| crate::time::to_local(instant, zone) != local)
            || split
                .local_start
                .zip(split.starts_at)
                .is_some_and(|(local, instant)| crate::time::to_local(instant, zone) != local)
            || split
                .local_end
                .zip(split.ends_at)
                .is_some_and(|(local, instant)| crate::time::to_local(instant, zone) != local);
        if needs_anchor {
            cursor = Some(CalendarCursor {
                nominal_local_anchor: split.local_key().unwrap(),
                wall_duration: spec.wall_duration(),
            });
        }
    }
    let tail = SeriesSpec::new_with_calendar_cursor(tail_input, cursor)?;
    Ok((Some(head), tail))
}

/// 'All' preserves the series; callers apply their edited authoritative fields then
/// revalidate with SeriesSpec::new. Retain nominal metadata only for non-timing edits;
/// clear both metadata columns when time/zone/kind/time-mode/all-day/rule/mode changes
/// (see [`CalendarCursor`]). This unchanged clone itself retains metadata.
pub fn all(spec: &SeriesSpec) -> SeriesSpec {
    spec.clone()
}
