use super::*;
use crate::items::{Kind, RecurrenceMode, Rule, SeriesInput, SeriesSpec};

/// Accept ICS case/order variations, drop inert WKST, then delegate all subset
/// syntax/semantics and canonical spelling to the authoritative items::Rule.
/// Whole-event series retention additionally requires items::SeriesSpec validation.
/// Without DTSTART this conservatively rejects non-MO WKST for interval-weekly
/// BYDAY rules; parse can retain them when every BYDAY equals DTSTART's weekday.
pub fn normalize_subset(rule: &str) -> Option<String> {
    normalize_with_anchor(rule, None)
}
/// Normalize with DTSTART's nominal local date. This can prove non-default WKST
/// inert for interval-weekly BYDAY rules that the anchorless API must reject.
/// Fixed DTSTARTs must first be converted to their event timezone (not UTC).
pub fn normalize_subset_at(rule: &str, dtstart: NaiveDate) -> Option<String> {
    use chrono::Datelike;
    let weekday = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"]
        [dtstart.weekday().num_days_from_monday() as usize];
    normalize_with_anchor(rule, Some(weekday))
}
fn normalize_with_anchor(rule: &str, weekday: Option<&str>) -> Option<String> {
    let upper = rule.trim().to_ascii_uppercase();
    let mut parts = BTreeMap::new();
    for part in upper.split(';') {
        let (key, value) = part.split_once('=')?;
        if parts.insert(key, value).is_some() {
            return None;
        }
    }
    if let Some(wkst) = parts.remove("WKST") {
        if !matches!(wkst, "MO" | "TU" | "WE" | "TH" | "FR" | "SA" | "SU") {
            return None;
        }
        let sensitive = parts.get("FREQ") == Some(&"WEEKLY")
            && parts
                .get("INTERVAL")
                .and_then(|s| s.parse::<u32>().ok())
                .is_some_and(|i| i > 1)
            && parts
                .get("BYDAY")
                .is_some_and(|s| s.split(',').any(|d| Some(d) != weekday));
        if wkst != "MO" && sensitive {
            return None;
        }
    }
    let freq = parts.remove("FREQ")?;
    let mut input = format!("FREQ={freq}");
    for (key, value) in parts {
        input.push_str(&format!(";{key}={value}"));
    }
    input.parse::<Rule>().ok().map(|r| r.to_string())
}

pub(super) fn keep_series(event: &ImportedEvent, rule: &str) -> Option<String> {
    // An override is never a second recurring master.
    if event.recurrence_id.is_some() {
        return None;
    }
    let date = match &event.start {
        IcsTime::Date(d) => *d,
        IcsTime::Floating(d) => d.date(),
        IcsTime::Fixed(d) => to_local(*d, event.tzid.as_deref().and_then(parse_tz)?).date(),
    };
    let canonical = normalize_subset_at(rule, date)?;
    let mut input = SeriesInput {
        kind: Kind::Appointment,
        time_mode: match event.time_mode {
            TimeMode::Fixed => crate::items::TimeMode::Fixed,
            TimeMode::Floating => crate::items::TimeMode::Floating,
        },
        tzid: event.tzid.clone(),
        all_day: event.all_day,
        rrule: Some(canonical),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        ..SeriesInput::default()
    };
    match (&event.start, &event.end) {
        (IcsTime::Fixed(start), IcsTime::Fixed(end)) => {
            input.starts_at = Some(*start);
            input.ends_at = Some(*end);
        }
        (IcsTime::Floating(start), IcsTime::Floating(end)) => {
            input.local_start = Some(*start);
            input.local_end = Some(*end);
        }
        (IcsTime::Date(start), IcsTime::Date(end)) => {
            input.local_start = start.and_hms_opt(0, 0, 0);
            input.local_end = end.and_hms_opt(0, 0, 0);
        }
        _ => return None,
    }
    SeriesSpec::new(input).ok()?.rule().map(ToString::to_string)
}
