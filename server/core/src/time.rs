//! Wall-clock ↔ instant resolution shared by recurrence, agendas and conflicts (master plan Decisions → Time):
//! a local time in a DST fold resolves to the **earlier** offset; a local time in a gap resolves to the **next valid
//! wall time** (the first wall-clock minute after the gap).
use chrono::{DateTime, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;

#[path = "time_links.rs"]
mod links;
pub use links::STORAGE_ZONE_LINKS;

/// Permissive bundled IANA parser for reads, recurrence and ICS; not a storage validator.
pub fn parse_tz(tzid: &str) -> Option<Tz> {
    tzid.parse().ok()
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown or unsupported storage time zone")]
pub struct StorageZoneError;
/// R35b: resolve tzdata links and UTC aliases before persistence. Return a canonical
/// zone and its static storage name. Factory is intentionally unsupported: unspecified
/// local time is not a household zone, though permissive parsing/ICS can still read it.
pub fn storage_zone(tzid: &str) -> Result<(Tz, &'static str), StorageZoneError> {
    let name = STORAGE_ZONE_LINKS
        .binary_search_by_key(&tzid, |(link, _)| *link)
        .ok()
        .map_or(tzid, |n| STORAGE_ZONE_LINKS[n].1);
    let zone = parse_tz(name).ok_or(StorageZoneError)?;
    if zone.name() != "UTC" && !zone.name().contains('/') {
        return Err(StorageZoneError);
    }
    Ok((zone, zone.name()))
}

/// The instant a wall-clock time denotes in `tz` (fold → earlier offset, gap → next valid wall time).
pub fn resolve_local(local: NaiveDateTime, tz: Tz) -> DateTime<Utc> {
    match tz.from_local_datetime(&local) {
        LocalResult::Single(t) => t.with_timezone(&Utc),
        LocalResult::Ambiguous(earlier, _) => earlier.with_timezone(&Utc),
        LocalResult::None => {
            // Real transitions sit on whole minutes, so the first valid minute after `local` is the end of the gap.
            // Gaps are at most a day (Pacific/Apia skipped 2011-12-30); 48 h bounds the walk.
            let start = local
                .with_second(0)
                .and_then(|t| t.with_nanosecond(0))
                .unwrap_or(local);
            (1..=48 * 60)
                .map(|m| start + Duration::minutes(m))
                .find_map(|t| match tz.from_local_datetime(&t) {
                    LocalResult::Single(r) => Some(r.with_timezone(&Utc)),
                    LocalResult::Ambiguous(earlier, _) => Some(earlier.with_timezone(&Utc)),
                    LocalResult::None => None,
                })
                .expect("tz database gaps are shorter than 48 hours")
        }
    }
}

/// Failure to resolve a positive wall-clock span.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ResolveSpanError {
    /// The nominal end must be strictly after the nominal start.
    #[error("end must follow start")]
    InvalidSpan,
    /// Correcting a collapsed span would overflow the instant range.
    #[error("resolved span timestamp overflow")]
    TimestampOverflow,
}

/// Resolves both nominal endpoints with the shared fold/gap rule (R10).
/// If resolution collapses or inverts the span, retains the resolved start and
/// sets the end to start + positive wall duration, using checked arithmetic.
/// Callers retain the nominal locals unchanged. Known fixed instants are input,
/// not wall times: do not pass authoritative seq-0 bounds through this helper.
pub fn resolve_span(
    local_start: NaiveDateTime,
    local_end: NaiveDateTime,
    tz: Tz,
) -> Result<(DateTime<Utc>, DateTime<Utc>), ResolveSpanError> {
    if local_start >= local_end {
        return Err(ResolveSpanError::InvalidSpan);
    }
    let start = resolve_local(local_start, tz);
    let end = resolve_local(local_end, tz);
    let end = if end <= start {
        start
            .checked_add_signed(local_end - local_start)
            .ok_or(ResolveSpanError::TimestampOverflow)?
    } else {
        end
    };
    Ok((start, end))
}

/// Midnight starting `date` in `tz`, resolved like any other wall time.
pub fn start_of_day(date: NaiveDate, tz: Tz) -> DateTime<Utc> {
    resolve_local(date.and_hms_opt(0, 0, 0).expect("midnight exists"), tz)
}

/// The wall-clock time of `instant` in `tz`.
pub fn to_local(instant: DateTime<Utc>, tz: Tz) -> NaiveDateTime {
    instant.with_timezone(&tz).naive_local()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").unwrap()
    }
    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn ordinary_time_resolves_with_its_offset() {
        let ny = parse_tz("America/New_York").unwrap();
        assert_eq!(
            resolve_local(local("2026-10-12T14:00:00"), ny),
            utc("2026-10-12T18:00:00Z")
        );
    }

    #[test]
    fn fold_takes_the_earlier_offset() {
        let ny = parse_tz("America/New_York").unwrap();
        // 01:30 happens twice on 2026-11-01: EDT (05:30Z) first, then EST (06:30Z).
        assert_eq!(
            resolve_local(local("2026-11-01T01:30:00"), ny),
            utc("2026-11-01T05:30:00Z")
        );
    }

    #[test]
    fn gap_takes_the_next_valid_wall_time() {
        let ny = parse_tz("America/New_York").unwrap();
        // 02:00–03:00 does not exist on 2026-03-08; 03:00 EDT is 07:00Z.
        assert_eq!(
            resolve_local(local("2026-03-08T02:30:00"), ny),
            utc("2026-03-08T07:00:00Z")
        );
        assert_eq!(
            resolve_local(local("2026-03-08T02:59:59"), ny),
            utc("2026-03-08T07:00:00Z")
        );
        // Lord Howe moves by 30 minutes: 02:00–02:30 is skipped on 2026-10-04 (+10:30 → +11).
        let lh = parse_tz("Australia/Lord_Howe").unwrap();
        assert_eq!(
            resolve_local(local("2026-10-04T02:10:00"), lh),
            utc("2026-10-03T15:30:00Z")
        );
    }

    #[test]
    fn a_skipped_day_resolves_to_the_following_midnight() {
        let apia = parse_tz("Pacific/Apia").unwrap();
        // Samoa skipped 2011-12-30 entirely.
        assert_eq!(
            resolve_local(local("2011-12-30T12:00:00"), apia),
            utc("2011-12-30T10:00:00Z")
        );
        assert_eq!(
            start_of_day(NaiveDate::from_ymd_opt(2011, 12, 31).unwrap(), apia),
            utc("2011-12-30T10:00:00Z")
        );
    }

    #[test]
    fn unknown_zones_are_rejected() {
        assert!(parse_tz("Mars/Olympus_Mons").is_none());
        assert!(parse_tz("").is_none());
    }

    #[test]
    fn to_local_round_trips_ordinary_times() {
        let berlin = parse_tz("Europe/Berlin").unwrap();
        let t = utc("2026-07-01T08:00:00Z");
        assert_eq!(to_local(t, berlin), local("2026-07-01T10:00:00"));
        assert_eq!(resolve_local(to_local(t, berlin), berlin), t);
    }
}
