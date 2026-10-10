//! Neutral RFC 5545 import/export, with no persistence or network access.
//!
//! DATE ends are exclusive. Missing ends mean one day for DATE and zero duration for
//! timed events (the importer must decide how to map zero-length events to SQL's
//! strictly positive appointment ranges). Unsupported rules expand through the
//! half-open horizon window, capped at 10,000 instances inside the window. Expanded
//! instances are standalone items: EXDATE and source overrides are already applied.
//! Kept series retain EXDATE metadata for S8 to apply as skipped dispositions.
//!
//! S8 refresh contract:
//! - An Override with no retained Series of the same UID (orphan or expanded master)
//!   is a standalone item, not an occurrence update against a missing parent.
//! - Expanded records outside [horizon_start, horizon_end) are absent. Feed removal
//!   soft-deletes for these records must be limited to that window, and suppressed
//!   on ExpansionLimited. Kept series, singles and source overrides are unfiltered.
//! - Duplicate same-UID masters can leave both Series and Expanded representations;
//!   InvalidEvent warns about this ambiguous source. Review it before persistence.
//! - A content line without a colon rejects the entire calendar (MalformedCalendar).
//!   S8 treats this as a failed refresh that deletes nothing.
//!
//! One parse shares 500,000 iterator pulls and 750 ms of cumulative iterator time
//! (1,500 ms in debug), including private rrule empty-period scans. A single pull
//! is additionally bounded by rrule's built-in limits. Exhaustion stops subsequent
//! unsupported rules and emits ExpansionLimited once, retaining earlier records.
//! Early iterator exhaustion without a satisfied COUNT or crossed UNTIL/horizon
//! warns too: the dependency does not expose its internal iteration-limit flag.
//! Natural UNTIL completion is checked without an unbudgeted collecting rescan.
//!
//! An override carrying RRULE retains its original RECURRENCE-ID as Override; its
//! further rule starts become Expanded standalone items, never a second copy of
//! its moved DTSTART. With RANGE=THISANDFUTURE, the master's old instances at/after
//! the original RECURRENCE-ID are superseded, following RFC 5545's range direction:
//! prior master starts are expanded, and the override's RRULE supplies the new
//! future. Without RANGE, the master future is unchanged. UnsupportedProperty warns
//! about this split and other RANGE forms; S8 must review it before persistence.
//! Multiple competing future-rule replacements are ambiguous (earliest cutoff wins).
use crate::time::{parse_tz, resolve_local, to_local};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::{Duration as ScanDuration, Instant};

// Shared by the entire parse, including rules that produce no in-window records.
// rrule does not expose its internal empty-period scans, so time every iterator
// pull as well. No collecting API may rescan history outside this budget.
const SCAN_CANDIDATES: usize = 500_000;
const SCAN_TIME: ScanDuration = if cfg!(debug_assertions) {
    ScanDuration::from_millis(1500)
} else {
    ScanDuration::from_millis(750)
};
#[derive(Default)]
struct ImportState {
    events: Vec<ImportedEvent>,
    warnings: Vec<ImportWarning>,
    unknown_zones: HashSet<(Option<String>, String)>,
    scans: usize,
    scan_time: ScanDuration,
    expansion_limited: bool,
}
impl ImportState {
    fn finish(self) -> ParsedCalendar {
        ParsedCalendar {
            events: self.events,
            warnings: self.warnings,
        }
    }
    fn scan<T>(&mut self, next: impl FnOnce() -> T) -> Option<T> {
        if self.scans >= SCAN_CANDIDATES || self.scan_time >= SCAN_TIME {
            return None;
        }
        self.scans += 1;
        let started = Instant::now();
        let value = next();
        self.scan_time += started.elapsed();
        Some(value)
    }
    fn scan_exhausted(&self) -> bool {
        self.scans >= SCAN_CANDIDATES || self.scan_time >= SCAN_TIME
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum IcsTime {
    Date(NaiveDate),
    Floating(NaiveDateTime),
    Fixed(DateTime<Utc>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeMode {
    Fixed,
    Floating,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventStatus {
    Confirmed,
    Tentative,
    Cancelled,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedEvent {
    pub uid: String,
    /// Identity key is (UID, recurrence_id). None is the series/single master;
    /// expanded standalone instances use their original rule start, not a moved start.
    pub recurrence_id: Option<IcsTime>,
    pub origin: EventOrigin,
    pub summary: String,
    pub description: String,
    pub location: String,
    pub start: IcsTime,
    pub end: IcsTime,
    pub all_day: bool,
    pub time_mode: TimeMode,
    pub tzid: Option<String>,
    pub rrule: Option<String>,
    pub exdates: Vec<IcsTime>,
    pub sequence: u32,
    pub status: EventStatus,
}
/// How S8 maps an imported record to persistence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventOrigin {
    Single,
    Series,
    /// True occurrence override only when a same-UID Series is retained.
    /// Otherwise (orphan/expanded master) S8 imports this as a standalone item.
    Override,
    /// Standalone item keyed by UID + rule_instance_start (= recurrence_id).
    /// Do not apply this record as an override of a retained series.
    Expanded {
        rule_instance_start: IcsTime,
    },
}
#[derive(Clone, Copy, Debug)]
pub struct ParseOptions {
    pub fallback_tz: Tz,
    /// Expansion start is inclusive. S8 passes now minus 180 days.
    pub horizon_start: DateTime<Utc>,
    /// Expansion end is exclusive. Kept series/singles/overrides are not filtered.
    pub horizon_end: DateTime<Utc>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningCode {
    MalformedCalendar,
    InvalidEvent,
    UnknownTimezone,
    ExpandedRule,
    InvalidRule,
    ExpansionLimited,
    UnsupportedProperty,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportWarning {
    pub uid: Option<String>,
    pub code: WarningCode,
    pub message: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedCalendar {
    pub events: Vec<ImportedEvent>,
    pub warnings: Vec<ImportWarning>,
}
fn warn(out: &mut ImportState, uid: Option<&str>, code: WarningCode, message: impl Into<String>) {
    let message = message.into();
    if code == WarningCode::UnknownTimezone
        && !out
            .unknown_zones
            .insert((uid.map(str::to_owned), message.clone()))
    {
        return;
    }
    if code == WarningCode::ExpansionLimited {
        if out.expansion_limited {
            return;
        }
        out.expansion_limited = true;
    }
    out.warnings.push(ImportWarning {
        uid: uid.map(str::to_owned),
        code,
        message,
    });
}

mod expand;
mod parse;
mod rules;
mod write;
mod zones;
pub use parse::parse;
pub use rules::normalize_subset;
pub use write::write;
pub use zones::WINDOWS_ZONES;
