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
//! Text is rejected before expansion at item limits: SUMMARY 200 characters,
//! DESCRIPTION 20,000, LOCATION 500, and UID 1,024 bytes. One parse admits at most
//! 32 MiB of retained event data including per-event overhead, charged before cloning.
//! Deterministic admission caps source work at 2,000,000 property/value units and
//! expansion at 500,000 iterator pulls. An independent watchdog also caps elapsed
//! parse/expansion time at 750 ms
//! (1,500 ms in debug), including metadata cloning and private rrule empty-period scans. A single pull
//! is additionally bounded by rrule's built-in limits. Exhaustion stops subsequent
//! work and emits ExpansionLimited once. Reconciliation shares this budget; if its
//! identity index cannot be completed, all unreconciled records are omitted.
//! Early iterator exhaustion without a satisfied COUNT or crossed UNTIL/horizon
//! warns too: the dependency does not expose its internal iteration-limit flag.
//! Natural UNTIL completion is checked without an unbudgeted collecting rescan.
//!
//! An override carrying RRULE retains its original RECURRENCE-ID as Override; its
//! further rule starts become Expanded standalone items, never a second copy of
//! its moved DTSTART. With RANGE=THISANDFUTURE, the master's old instances at/after
//! the original RECURRENCE-ID are superseded, following RFC 5545's range direction:
//! prior master starts are expanded, and the override's RRULE supplies the new
//! future. Without RANGE, the master future is unchanged: overlapping master and
//! expanded-override starts are deliberately preserved, not deduplicated by time.
//! COUNT includes DTSTART (even when filters exclude it), matching the items domain;
//! this is a Porchlight import convention, not a reinterpretation of RFC expansion.
//! UnsupportedProperty warns
//! about this split and other RANGE forms; S8 must review it before persistence.
//! Multiple competing future-rule replacements are ambiguous (earliest cutoff wins).
use crate::time::{parse_tz, resolve_local, to_local};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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
const DIAGNOSTIC_COUNT: usize = 128;
const DIAGNOSTIC_BYTES: usize = 64 * 1024;
const DIAGNOSTICS_TRUNCATED: &str = "Diagnostics truncated; incomplete";
const SOURCE_WORK: usize = 2_000_000;
const EXDATE_VALUES: usize = 10_000;
#[derive(Default)]
struct ImportState {
    events: Vec<ImportedEvent>,
    warnings: Vec<ImportWarning>,
    diagnostic_bytes: usize,
    diagnostics_truncated: bool,
    source_work: usize,
    #[cfg(test)]
    reconciliation_checks: usize,
    scans: usize,
    scan_time: ScanDuration,
    expansion_limited: bool,
    emitted_bytes: usize,
    invalid_skipped: usize,
    started: Option<Instant>,
}
impl ImportState {
    // Charge the owned representation before cloning, shared across every source
    // event (including singles/overrides). Count overhead as well as retained text.
    fn admit(&mut self, event: &ImportedEvent, expanded: bool) -> bool {
        let bytes = std::mem::size_of::<ImportedEvent>()
            + event.uid.len()
            + event.summary.len()
            + event.description.len()
            + event.location.len()
            + event.tzid.as_ref().map_or(0, String::len)
            + if expanded {
                0
            } else {
                event.rrule.as_ref().map_or(0, String::len)
                    + event.exdates.len() * std::mem::size_of::<IcsTime>()
            };
        if bytes > (32 * 1024 * 1024) - self.emitted_bytes {
            warn(
                self,
                Some(&event.uid),
                WarningCode::ExpansionLimited,
                "Parse-wide emitted event/byte budget exhausted; incomplete",
            );
            return false;
        }
        self.emitted_bytes += bytes;
        true
    }
    fn emit(&mut self, event: ImportedEvent) {
        if self.admit(&event, false) {
            self.events.push(event);
        }
    }
    fn finish(self) -> ParsedCalendar {
        ParsedCalendar {
            events: self.events,
            warnings: self.warnings,
            invalid_skipped: self.invalid_skipped,
        }
    }
    fn scan<T>(&mut self, next: impl FnOnce() -> T) -> Option<T> {
        if self.scan_exhausted() {
            return None;
        }
        self.scans += 1;
        let started = Instant::now();
        let value = next();
        self.scan_time += started.elapsed();
        Some(value)
    }
    // Charge source properties and each EXDATE value before processing, not only
    // recurrence iterator pulls. An omitted source always suppresses reconciliation.
    fn source_admit(&mut self, count: usize) -> bool {
        if count > SOURCE_WORK - self.source_work
            || self.started.is_some_and(|t| t.elapsed() >= SCAN_TIME)
        {
            warn(
                self,
                None,
                WarningCode::ExpansionLimited,
                "Source property work budget exhausted; incomplete",
            );
            return false;
        }
        self.source_work += count;
        true
    }
    fn scan_exhausted(&self) -> bool {
        self.scans >= SCAN_CANDIDATES
            || self.scan_time >= SCAN_TIME
            || self.started.is_some_and(|t| t.elapsed() >= SCAN_TIME)
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
    /// Source events omitted by early item text/identity validation (before expansion).
    #[serde(default)]
    pub invalid_skipped: usize,
}
fn warn(out: &mut ImportState, uid: Option<&str>, code: WarningCode, message: impl AsRef<str>) {
    if out.diagnostics_truncated {
        return;
    }
    let message = message.as_ref();
    // Deduplicate against the bounded diagnostics themselves: a parallel set must
    // not retain another unbudgeted copy of every UID/message.
    if code == WarningCode::UnknownTimezone
        && out
            .warnings
            .iter()
            .any(|w| w.code == code && w.uid.as_deref() == uid && w.message == message)
    {
        return;
    }
    if code == WarningCode::ExpansionLimited && out.expansion_limited {
        if uid.is_none()
            && let Some(warning) = out.warnings.iter_mut().find(|w| w.code == code)
        {
            warning.uid = None;
        }
        return;
    }
    let bytes = std::mem::size_of::<ImportWarning>() + uid.map_or(0, str::len) + message.len();
    let marker_bytes = std::mem::size_of::<ImportWarning>() + DIAGNOSTICS_TRUNCATED.len();
    // Reserve one global marker, independent of ExpansionLimited deduplication.
    // Check both caps BEFORE cloning either field (including a maximum-size UID).
    if out.warnings.len() >= DIAGNOSTIC_COUNT - 1
        || bytes > DIAGNOSTIC_BYTES - marker_bytes - out.diagnostic_bytes
    {
        out.diagnostics_truncated = true;
        out.diagnostic_bytes += marker_bytes;
        out.warnings.push(ImportWarning {
            uid: None,
            code: WarningCode::ExpansionLimited,
            message: DIAGNOSTICS_TRUNCATED.to_owned(),
        });
        return;
    }
    out.expansion_limited |= code == WarningCode::ExpansionLimited;
    out.diagnostic_bytes += bytes;
    out.warnings.push(ImportWarning {
        uid: uid.map(str::to_owned),
        code,
        message: message.to_owned(),
    });
}

mod expand;
mod parse;
mod rules;
mod write;
mod zones;

pub use parse::parse;
pub use rules::{normalize_subset, normalize_subset_at};
pub use write::{Writer, write};
pub use zones::WINDOWS_ZONES;

#[cfg(test)]
mod audit_authz_bounds {
    use super::*;

    #[test]
    fn source_work_has_structural_and_elapsed_limits() {
        let mut out = ImportState::default();
        assert!(out.source_admit(SOURCE_WORK));
        assert!(!out.source_admit(1));
        assert_eq!(out.source_work, SOURCE_WORK);
        assert!(
            out.warnings
                .iter()
                .any(|w| w.uid.is_none() && w.code == WarningCode::ExpansionLimited)
        );
        let mut out = ImportState {
            started: Some(Instant::now() - SCAN_TIME),
            ..Default::default()
        };
        assert!(!out.source_admit(1));
        assert_eq!(out.source_work, 0);
    }

    #[test]
    fn diagnostic_admission_reserves_a_single_global_marker() {
        for uid in ["u", &"u".repeat(1024)] {
            let mut out = ImportState::default();
            for _ in 0..1024 {
                warn(&mut out, Some(uid), WarningCode::InvalidEvent, "invalid");
            }
            assert!(out.warnings.len() <= DIAGNOSTIC_COUNT);
            assert!(out.diagnostic_bytes <= DIAGNOSTIC_BYTES);
            assert_eq!(
                out.warnings
                    .iter()
                    .filter(|w| w.uid.is_none() && w.message == DIAGNOSTICS_TRUNCATED)
                    .count(),
                1
            );
            assert_eq!(
                out.diagnostic_bytes,
                out.warnings
                    .iter()
                    .map(|w| std::mem::size_of_val(w)
                        + w.uid.as_ref().map_or(0, String::len)
                        + w.message.len())
                    .sum::<usize>()
            );
        }
    }
}

#[cfg(test)]
mod audit_b_final_server;

#[cfg(test)]
mod audit_bl_server2 {
    use super::*;

    #[test]
    fn scan_budget_is_deterministic_independent_of_watchdog() {
        let mut state = ImportState {
            scans: SCAN_CANDIDATES - 1,
            ..Default::default()
        };
        assert_eq!(state.scan(|| 7), Some(7));
        assert_eq!(state.scans, SCAN_CANDIDATES);
        assert!(state.scan(|| panic!("must not pull after cap")).is_none());
        let mut state = ImportState {
            scan_time: SCAN_TIME,
            ..Default::default()
        };
        assert!(
            state
                .scan(|| panic!("watchdog must stop a costly dependency pull"))
                .is_none()
        );
        assert_eq!(state.scans, 0);
    }
}
