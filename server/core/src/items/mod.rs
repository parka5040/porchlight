//! Pure item recurrence and edit arithmetic. Windows are half-open **local wall-clock**
//! intervals, including for fixed series. Convert an instant window to the item's zone
//! before calling [`expand`]. Floating expansion never consults a member's zone.
//!
//! DTSTART is always seq 0 (even when it does not match the filters) and counts toward
//! COUNT. Date UNTIL includes its entire local day; UTC UNTIL compares resolved instants
//! for fixed items and synthetic UTC wall times for floating/date-only items.
//! All-day appointments retain midnight local_start/local_end, with an exclusive end date.
//! Persistence, completed/detached-row preservation and successor uniqueness belong to S3.
#![deny(missing_docs)]

mod edits;
mod engine;
mod rule;

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Timelike, Utc};
use chrono_tz::Tz;
pub use edits::{
    OccurrenceOverride, all, persisted_occurrence_override, this_and_future, this_occurrence,
};
pub use engine::{
    HORIZON_MONTHS, OccurrenceAnchor, OpenOccurrence, bounded_occurrence_count, dispositions,
    expand, first_anchor, horizon_end, next_after_completion,
};
pub use rule::{Rule, Until};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// A field-level error suitable for a validation-failed/422 response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, thiserror::Error)]
#[error("{field}: {reason}")]
pub struct ValidationError {
    /// Wire field responsible for the validation error.
    pub field: &'static str,
    /// Human-readable explanation without secrets.
    pub reason: String,
}
impl ValidationError {
    pub(super) fn new(field: &'static str, reason: impl Into<String>) -> Self {
        Self {
            field,
            reason: reason.into(),
        }
    }
}
macro_rules! db_enum {
    ($name:ident, $field:literal, $default:ident, $($variant:ident => $text:literal),+ $(,)?) => {
        #[doc = concat!("Migration 0003 `", stringify!($name), "` values; FromStr/Display/Serde use the exact database strings.")]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name { $(#[doc = $text] #[serde(rename = $text)] $variant),+ }
        impl $name {
            /// Complete wire-value inventory for contract parity checks.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
            /// Exact database spelling, also used by Display and Serde.
            pub const fn as_str(self) -> &'static str { match self { $(Self::$variant => $text),+ } }
        }
        impl Default for $name { fn default() -> Self { Self::$default } }
        impl FromStr for $name { type Err = ValidationError; fn from_str(s: &str) -> Result<Self, Self::Err> { match s { $($text => Ok(Self::$variant),)+ _ => Err(ValidationError::new($field, "unknown DB enum value")) } } }
        impl fmt::Display for $name { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.as_str()) } }
    };
}
db_enum!(Kind, "kind", Todo, Todo => "todo", Deadline => "deadline", Appointment => "appointment");
db_enum!(TimeMode, "time_mode", Fixed, Fixed => "fixed", Floating => "floating");
db_enum!(RecurrenceMode, "recurrence_mode", Calendar, Calendar => "calendar", Completion => "completion");
db_enum!(MissedPolicy, "missed_policy", CarryOver, CarryOver => "carry_over", Skip => "skip", Stack => "stack");
db_enum!(Status, "status", Todo, Backlog => "backlog", Todo => "todo", Doing => "doing", Done => "done", Skipped => "skipped");
db_enum!(Disposition, "disposition", Open, Open => "open", Overdue => "overdue", Expired => "expired");
impl Status {
    /// Done/skipped occurrences are closed and excluded from missed-policy arithmetic.
    pub const fn is_open(self) -> bool {
        !matches!(self, Self::Done | Self::Skipped)
    }
}

/// Postgres interval's three independent components (not a flattened duration).
/// Positivity follows PG comparison: a month is 30 days for comparison only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PgInterval {
    /// Calendar months, clamped at month end during completion arithmetic.
    pub months: i32,
    /// Calendar days, advanced in the effective zone.
    pub days: i32,
    /// Elapsed microseconds applied after calendar resolution.
    pub microseconds: i64,
}
impl PgInterval {
    /// Postgres `interval > interval '0'` comparison, computed without integer overflow.
    pub fn is_positive(self) -> bool {
        (i128::from(self.months) * 30 + i128::from(self.days)) * 86_400_000_000
            + i128::from(self.microseconds)
            > 0
    }
}

/// Derivation inputs from an items row. Use [`SeriesSpec::new`] before deriving.
/// Defaults describe an undated, non-recurring fixed todo (valid in migration 0003).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SeriesInput {
    /// Item kind (database/API spelling).
    pub kind: Kind,
    /// Fixed instants or floating local wall-clock anchors.
    pub time_mode: TimeMode,
    /// IANA item zone; required for fixed timed items, absent for floating.
    pub tzid: Option<String>,
    /// Optional todo due date.
    pub due_date: Option<NaiveDate>,
    /// Authoritative fixed deadline instant (including the chosen fold branch).
    pub due_at: Option<DateTime<Utc>>,
    /// Authoritative fixed appointment start instant.
    pub starts_at: Option<DateTime<Utc>>,
    /// Authoritative fixed appointment exclusive end instant.
    pub ends_at: Option<DateTime<Utc>>,
    /// Floating deadline nominal wall time.
    pub local_due: Option<NaiveDateTime>,
    /// Floating appointment nominal start wall time.
    pub local_start: Option<NaiveDateTime>,
    /// Floating appointment nominal exclusive end wall time.
    pub local_end: Option<NaiveDateTime>,
    /// Appointment bounds must be midnight locals, with an exclusive end date.
    pub all_day: bool,
    /// Calendar RRULE subset; canonicalized by the validating constructor.
    pub rrule: Option<String>,
    /// Calendar, completion, or None for a single item.
    pub recurrence_mode: Option<RecurrenceMode>,
    /// Three-component PG interval for completion recurrence only.
    pub completion_interval: Option<PgInterval>,
    /// How older open missed occurrences are classified.
    pub missed_policy: MissedPolicy,
}

/// Optional nominal timing metadata for a **fixed calendar tail** split in a DST gap.
/// This is not a phase/lineage cursor: the tail's DTSTART is the split instance,
/// COUNT is reduced, and seqs restart at 0. Floating/date-only tails need no metadata.
///
/// S3 persists `nominal_local_anchor` and `wall_duration` in two nullable item columns.
/// No anchor means no metadata (duration must also be NULL). With an anchor present,
/// deadline duration is NULL; appointment duration is positive and exact (no rounding).
/// Store it only when either resolved endpoint does not round-trip to its nominal local.
/// It restores the nominal due/start and appointment duration lost in the fixed row's
/// instants. Rehydrate via [`SeriesSpec::new_with_calendar_cursor`] so nominal all-day
/// bounds and UNTIL are validated before the resolved row loses gap readings.
/// Retain for non-timing edits; clear BOTH columns on an "all" edit changing time,
/// zone, kind, time mode, all_day, or recurrence rule/mode. New splits compute fresh
/// metadata, never inherit an original DTSTART or cumulative skip count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarCursor {
    /// Nominal local due/start at the tail's own DTSTART, before gap resolution.
    pub nominal_local_anchor: NaiveDateTime,
    /// Positive appointment wall duration; None for a deadline.
    pub wall_duration: Option<chrono::Duration>,
}

/// Validated derivation spec. Inputs and calendar cursor are immutable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeriesSpec {
    pub(super) input: SeriesInput,
    pub(super) rule: Option<Rule>,
    pub(super) zone: Option<Tz>,
    pub(super) cursor: Option<CalendarCursor>,
}
impl SeriesSpec {
    /// Enforces migration 0003's time/recurrence shapes and interval positivity.
    /// Also rejects unexpandable calendar todos without a date, non-midnight all-day
    /// bounds, unsupported dates. Ordered fixed instants are accepted even across a fold;
    /// generated instances use positive wall duration, falling back to elapsed duration.
    pub fn new(input: SeriesInput) -> Result<Self, ValidationError> {
        Self::new_with_calendar_cursor(input, None)
    }
    /// Validates row input together with its optional nominal fixed-tail metadata.
    /// S3 uses this input path for all rows, including gap tails: attaching metadata
    /// only after `new` would reject nominal all-day midnights resolved to 01:00 or
    /// date UNTIL on a skipped day whose authoritative instant reads the following date.
    /// See [`CalendarCursor`] for when to save, retain, or clear the two nullable values.
    pub fn new_with_calendar_cursor(
        mut input: SeriesInput,
        cursor: Option<CalendarCursor>,
    ) -> Result<Self, ValidationError> {
        let fail = |field, reason| Err(ValidationError::new(field, reason));
        // Reject unsupported instants before converting zones: chrono's extreme
        // UTC values can overflow naive_local when a positive offset is applied.
        for (field, instant) in [
            ("due_at", input.due_at),
            ("starts_at", input.starts_at),
            ("ends_at", input.ends_at),
        ] {
            if let Some(instant) = instant {
                validate_local(field, instant.naive_utc())?;
            }
        }
        let zone = match &input.tzid {
            Some(z) => {
                let (zone, name) = crate::time::storage_zone(z)
                    .map_err(|_| ValidationError::new("tzid", "unknown IANA time zone"))?;
                input.tzid = Some(name.to_owned());
                Some(zone)
            }
            None => None,
        };
        match input.kind {
            Kind::Todo => {
                for (field, set) in [
                    ("due_at", input.due_at.is_some()),
                    ("starts_at", input.starts_at.is_some()),
                    ("ends_at", input.ends_at.is_some()),
                    ("local_due", input.local_due.is_some()),
                    ("local_start", input.local_start.is_some()),
                    ("local_end", input.local_end.is_some()),
                ] {
                    if set {
                        return fail(field, "todo permits only due_date");
                    }
                }
            }
            Kind::Deadline => {
                for (field, set) in [
                    ("due_date", input.due_date.is_some()),
                    ("starts_at", input.starts_at.is_some()),
                    ("ends_at", input.ends_at.is_some()),
                    ("local_start", input.local_start.is_some()),
                    ("local_end", input.local_end.is_some()),
                ] {
                    if set {
                        return fail(field, "deadline permits only a due anchor");
                    }
                }
                match input.time_mode {
                    TimeMode::Fixed => {
                        if zone.is_none() {
                            return fail("tzid", "fixed timed item requires an IANA zone");
                        }
                        if input.local_due.is_some() {
                            return fail("local_due", "fixed item cannot have a floating anchor");
                        }
                        if input.due_at.is_none() {
                            return fail("due_at", "fixed deadline requires a due instant");
                        }
                    }
                    TimeMode::Floating => {
                        if zone.is_some() {
                            return fail("tzid", "floating item cannot have an item zone");
                        }
                        if input.due_at.is_some() {
                            return fail("due_at", "floating item cannot have a fixed instant");
                        }
                        if input.local_due.is_none() {
                            return fail(
                                "local_due",
                                "floating deadline requires a local due time",
                            );
                        }
                    }
                }
            }
            Kind::Appointment => {
                for (field, set) in [
                    ("due_date", input.due_date.is_some()),
                    ("due_at", input.due_at.is_some()),
                    ("local_due", input.local_due.is_some()),
                ] {
                    if set {
                        return fail(field, "appointment permits only start/end anchors");
                    }
                }
                match input.time_mode {
                    TimeMode::Fixed => {
                        if zone.is_none() {
                            return fail("tzid", "fixed timed item requires an IANA zone");
                        }
                        if input.local_start.is_some() {
                            return fail("local_start", "fixed item cannot have floating anchors");
                        }
                        if input.local_end.is_some() {
                            return fail("local_end", "fixed item cannot have floating anchors");
                        }
                        if input.starts_at.is_none() {
                            return fail("starts_at", "appointment requires a start instant");
                        }
                        if input.ends_at.is_none() || input.ends_at <= input.starts_at {
                            return fail("ends_at", "end must follow start");
                        }
                    }
                    TimeMode::Floating => {
                        if zone.is_some() {
                            return fail("tzid", "floating item cannot have an item zone");
                        }
                        if input.starts_at.is_some() {
                            return fail("starts_at", "floating item cannot have fixed instants");
                        }
                        if input.ends_at.is_some() {
                            return fail("ends_at", "floating item cannot have fixed instants");
                        }
                        if input.local_start.is_none() {
                            return fail("local_start", "appointment requires a local start");
                        }
                        if input.local_end.is_none() || input.local_end <= input.local_start {
                            return fail("local_end", "end must follow start");
                        }
                    }
                }
            }
        }
        match (
            input.recurrence_mode,
            &input.rrule,
            input.completion_interval,
        ) {
            (None, None, None) | (Some(RecurrenceMode::Calendar), Some(_), None) => {}
            (Some(RecurrenceMode::Completion), None, Some(interval)) if interval.is_positive() => {}
            _ => {
                return fail(
                    "recurrence_mode",
                    "calendar requires only rrule; completion requires only a positive interval; non-recurring requires neither",
                );
            }
        }
        if input.kind == Kind::Todo
            && input.due_date.is_none()
            && input.recurrence_mode == Some(RecurrenceMode::Calendar)
        {
            return fail("due_date", "calendar todo requires a dated DTSTART");
        }
        let rule = input.rrule.as_deref().map(str::parse::<Rule>).transpose()?;
        input.rrule = rule.as_ref().map(ToString::to_string);
        let mut spec = Self {
            input,
            rule,
            zone,
            cursor: None,
        };
        if let Some(cursor) = cursor {
            spec = spec.with_calendar_cursor(cursor)?;
        }
        for (field, anchor) in [
            (
                if spec.input.time_mode == TimeMode::Floating {
                    "local_due"
                } else {
                    "due_at"
                },
                spec.local_due(),
            ),
            (
                if spec.input.time_mode == TimeMode::Floating {
                    "local_start"
                } else {
                    "starts_at"
                },
                spec.local_start(),
            ),
            (
                if spec.input.time_mode == TimeMode::Floating {
                    "local_end"
                } else {
                    "ends_at"
                },
                spec.local_end(),
            ),
        ] {
            if let Some(t) = anchor {
                validate_local(field, t)?;
            }
        }
        if let Some(d) = spec.input.due_date {
            validate_local("due_date", d.and_hms_opt(0, 0, 0).unwrap())?;
        }
        if spec.input.kind == Kind::Appointment {
            let start = spec.start().unwrap();
            let end = if spec.cursor.is_some() {
                start
                    .checked_add_signed(spec.wall_duration().unwrap())
                    .ok_or_else(|| ValidationError::new("local_end", "date overflow"))?
            } else {
                spec.local_end().unwrap()
            };
            if spec.input.all_day
                && (start.time() != chrono::NaiveTime::MIN || end.time() != chrono::NaiveTime::MIN)
            {
                return fail(
                    "all_day",
                    "all-day appointment requires midnight date bounds (exclusive end)",
                );
            }
        }
        if spec
            .start()
            .is_some_and(|start| !engine::within_until(&spec, start))
        {
            return fail("rrule", "UNTIL precedes DTSTART");
        }
        Ok(spec)
    }
    /// Original row inputs, with RRULE replaced by its canonical spelling.
    pub fn input(&self) -> &SeriesInput {
        &self.input
    }
    /// Parsed calendar rule; None for single/completion items.
    pub fn rule(&self) -> Option<&Rule> {
        self.rule.as_ref()
    }
    /// Parsed item zone; None for floating timed items (todos follow DB shape rules).
    pub fn zone(&self) -> Option<Tz> {
        self.zone
    }
    /// Optional nominal fixed-tail anchor; see [`CalendarCursor`] for storage/clear rules.
    pub fn calendar_cursor(&self) -> Option<CalendarCursor> {
        self.cursor
    }
    /// Rehydrates optional fixed calendar-tail timing metadata. Generated nominal
    /// endpoints must resolve to the row's authoritative instants; no phase is stored.
    pub fn with_calendar_cursor(mut self, cursor: CalendarCursor) -> Result<Self, ValidationError> {
        let invalid = |reason| ValidationError::new("calendar_cursor", reason);
        if self.input.recurrence_mode != Some(RecurrenceMode::Calendar)
            || self.input.time_mode != TimeMode::Fixed
            || self.input.kind == Kind::Todo
        {
            return Err(invalid(
                "only fixed timed calendar tails have nominal metadata",
            ));
        }
        validate_local("calendar_cursor", cursor.nominal_local_anchor)?;
        match (self.input.kind, cursor.wall_duration) {
            (Kind::Appointment, Some(duration)) if duration > chrono::Duration::zero() => {}
            (Kind::Deadline, None) => {}
            _ => {
                return Err(invalid(
                    "appointment requires positive wall duration; deadline requires none",
                ));
            }
        }
        self.cursor = Some(cursor);
        let first = engine::anchor_for_local(&self, cursor.nominal_local_anchor, 0)?;
        if self.input.all_day
            && self.input.kind == Kind::Appointment
            && (first.local_start.unwrap().time() != chrono::NaiveTime::MIN
                || first.local_end.unwrap().time() != chrono::NaiveTime::MIN)
        {
            return Err(ValidationError::new(
                "all_day",
                "nominal all-day bounds must be midnight",
            ));
        }
        if first.due_at != self.input.due_at
            || first.starts_at != self.input.starts_at
            || first.ends_at != self.input.ends_at
        {
            return Err(invalid("nominal metadata does not reproduce row instants"));
        }
        Ok(self)
    }
    pub(super) fn wall_duration(&self) -> Option<chrono::Duration> {
        self.cursor.and_then(|c| c.wall_duration).or_else(|| {
            self.local_start()
                .zip(self.local_end())
                .map(|(start, end)| {
                    let wall = end - start;
                    if wall > chrono::Duration::zero() {
                        wall
                    } else {
                        self.input.ends_at.unwrap() - self.input.starts_at.unwrap()
                    }
                })
        })
    }
    pub(super) fn local_due(&self) -> Option<NaiveDateTime> {
        self.input.local_due.or_else(|| {
            self.input
                .due_at
                .map(|t| crate::time::to_local(t, self.zone.unwrap()))
        })
    }
    pub(super) fn local_start(&self) -> Option<NaiveDateTime> {
        self.input.local_start.or_else(|| {
            self.input
                .starts_at
                .map(|t| crate::time::to_local(t, self.zone.unwrap()))
        })
    }
    pub(super) fn local_end(&self) -> Option<NaiveDateTime> {
        self.input.local_end.or_else(|| {
            self.input
                .ends_at
                .map(|t| crate::time::to_local(t, self.zone.unwrap()))
        })
    }
    pub(super) fn start(&self) -> Option<NaiveDateTime> {
        self.cursor.map(|c| c.nominal_local_anchor).or_else(|| {
            self.input
                .due_date
                .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
                .or_else(|| self.local_due())
                .or_else(|| self.local_start())
        })
    }
}
pub(super) fn validate_local(field: &'static str, t: NaiveDateTime) -> Result<(), ValidationError> {
    if !(1..=9999).contains(&t.year()) || t.nanosecond() >= 1_000_000_000 {
        return Err(ValidationError::new(
            field,
            "date must be year 0001..9999 without leap seconds",
        ));
    }
    Ok(())
}
