use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use porchlight_core::ics::{self, EventStatus, IcsTime, ParseOptions, TimeMode};
use proptest::prelude::*;

fn utc(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}
fn opts() -> ParseOptions {
    ParseOptions {
        fallback_tz: chrono_tz::America::New_York,
        horizon_start: utc("2026-01-01T00:00:00Z"),
        horizon_end: utc("2026-05-01T00:00:00Z"),
    }
}
fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/ics/{name}.ics",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn google_recurrence_exdates_and_override() {
    let cal = ics::parse(&fixture("google"), opts());
    assert!(cal.warnings.is_empty(), "{:?}", cal.warnings);
    assert_eq!(cal.events.len(), 2);
    let e = &cal.events[0];
    assert_eq!(e.rrule.as_deref(), Some("FREQ=WEEKLY;BYDAY=MO;COUNT=6"));
    assert_eq!(e.exdates.len(), 2);
    assert_eq!(e.start, IcsTime::Fixed(utc("2026-03-02T14:00:00Z")));
    assert_eq!(e.description, "Planning, projects\nBring notes.");
    assert_eq!(
        cal.events[1].recurrence_id,
        Some(IcsTime::Fixed(utc("2026-03-23T13:00:00Z")))
    );
    assert_eq!(cal.events[1].sequence, 4);
}
#[test]
fn windows_alias_and_x_lic_location_and_duration() {
    let cal = ics::parse(&fixture("outlook"), opts());
    assert!(cal.warnings.is_empty(), "{:?}", cal.warnings);
    assert_eq!(cal.events[0].tzid.as_deref(), Some("America/New_York"));
    assert_eq!(
        cal.events[0].start,
        IcsTime::Fixed(utc("2026-11-01T05:30:00Z"))
    );
    assert_eq!(
        cal.events[0].end,
        IcsTime::Fixed(utc("2026-11-01T06:15:00Z"))
    );
    assert_eq!(cal.events[1].tzid.as_deref(), Some("Europe/Berlin"));
}
#[test]
fn all_day_school_cancelled_and_floating_defaults() {
    let apple = ics::parse(&fixture("apple"), opts());
    let e = &apple.events[0];
    assert_eq!(
        e.start,
        IcsTime::Date(NaiveDate::from_ymd_opt(2026, 10, 5).unwrap())
    );
    assert_eq!(
        e.end,
        IcsTime::Date(NaiveDate::from_ymd_opt(2026, 10, 6).unwrap())
    );
    assert!(e.all_day);
    assert_eq!(e.time_mode, TimeMode::Floating);
    assert_eq!(e.tzid, None);
    let school = ics::parse(&fixture("school"), opts());
    assert_eq!(school.events.len(), 20);
    assert_eq!(school.events[11].status, EventStatus::Cancelled);
    let floating = ics::parse(&fixture("floating"), opts());
    assert_eq!(
        floating.events[0].start,
        IcsTime::Floating(
            NaiveDateTime::parse_from_str("20260308T080000", "%Y%m%dT%H%M%S").unwrap()
        )
    );
    assert_eq!(floating.events[1].start, floating.events[1].end);
}
#[test]
fn expands_unsupported_rules_keeps_identity_and_exdates() {
    let cal = ics::parse(&fixture("unsupported"), opts());
    assert_eq!(cal.events.len(), 7, "{:?}", cal.warnings);
    assert_eq!(cal.warnings.len(), 2);
    assert!(
        cal.events
            .iter()
            .all(|e| e.rrule.is_none() && e.recurrence_id == Some(e.start.clone()))
    );
    assert_eq!(
        cal.events[1].start,
        IcsTime::Fixed(utc("2026-03-31T13:00:00Z"))
    );
    assert!(cal.events[1].exdates.is_empty());
}
#[test]
fn malformed_and_folded_and_unknown_zone() {
    let cal = ics::parse(&fixture("malformed"), opts());
    assert!(cal.events.is_empty());
    assert_eq!(cal.warnings[0].code, ics::WarningCode::MalformedCalendar);
    let folded = ics::parse(&fixture("folded"), opts());
    assert!(folded.events[0].summary.contains("boardgames"));
    let unknown = ics::parse(
        "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:unknown\nDTSTART;TZID=Mars:20260308T023000\nEND:VEVENT\nEND:VCALENDAR\n",
        opts(),
    );
    assert_eq!(unknown.warnings[0].code, ics::WarningCode::UnknownTimezone);
    assert_eq!(
        unknown.events[0].start,
        IcsTime::Fixed(utc("2026-03-08T07:00:00Z"))
    );
}
#[test]
fn subset_exact_bounds_duplicates_and_count_until() {
    for rule in [
        "FREQ=DAILY;INTERVAL=999;COUNT=9999",
        "freq=weekly;byday=mo,we",
        "FREQ=MONTHLY;BYDAY=-1MO;BYMONTHDAY=-1,31",
        "FREQ=YEARLY;UNTIL=20280101",
    ] {
        assert!(ics::normalize_subset(rule).is_some(), "{rule}");
    }
    for rule in [
        "FREQ=HOURLY",
        "FREQ=DAILY;INTERVAL=1000",
        "FREQ=DAILY;COUNT=0",
        "FREQ=DAILY;COUNT=2;UNTIL=20270101",
        "FREQ=WEEKLY;BYDAY=6MO",
        "FREQ=DAILY;COUNT=2;COUNT=3",
        "FREQ=MONTHLY;BYMONTHDAY=0",
        "FREQ=DAILY;UNTIL=20260230",
    ] {
        assert!(ics::normalize_subset(rule).is_none(), "{rule}");
    }
}
#[test]
fn writer_roundtrip_and_utf8_octet_folding() {
    for name in [
        "google",
        "outlook",
        "apple",
        "floating",
        "school",
        "unsupported",
    ] {
        let mut events = ics::parse(&fixture(name), opts()).events;
        events[0].summary = "é🙂; commas, slashes\\\nand lines".repeat(6);
        let text = ics::write(
            &events,
            "Family, calendar\nshared",
            utc("2026-10-05T12:00:00Z"),
        );
        assert!(text.contains("PRODID:-//Porchlight//EN\r\n"));
        assert!(text.contains("DTSTAMP:"));
        assert!(text.split("\r\n").all(|line| line.len() <= 75));
        assert!(!text.replace("\r\n", "").contains(['\r', '\n']));
        let parsed = ics::parse(&text, opts());
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);
        assert_eq!(parsed.events, events, "{name}");
    }
}
#[test]
fn text_escapes_are_decoded_exactly_once() {
    let mut events = ics::parse(&fixture("google"), opts()).events;
    events[0].uid = "literal\\n\\,\\;".to_owned();
    events[0].summary = "literal\\n\\,\\;".to_owned();
    let parsed = ics::parse(
        &ics::write(&events, "test", utc("2026-10-05T12:00:00Z")),
        opts(),
    );
    assert_eq!(parsed.events, events);
}
#[test]
fn nominal_day_duration_across_dst_and_z_calendar_zone() {
    let text = "BEGIN:VCALENDAR\nX-WR-TIMEZONE:America/New_York\nBEGIN:VEVENT\nUID:overnight\nDTSTART;TZID=America/New_York:20260307T120000\nDURATION:P1D\nEND:VEVENT\nBEGIN:VEVENT\nUID:zoned-z\nDTSTART:20260308T070000Z\nEND:VEVENT\nEND:VCALENDAR\n";
    let cal = ics::parse(text, opts());
    assert_eq!(
        cal.events[0].end,
        IcsTime::Fixed(utc("2026-03-08T16:00:00Z"))
    );
    assert_eq!(cal.events[1].tzid.as_deref(), Some("America/New_York"));
}
#[test]
fn late_fold_instant_roundtrips_without_losing_zone() {
    let text = "BEGIN:VCALENDAR\nX-WR-TIMEZONE:America/New_York\nBEGIN:VEVENT\nUID:fold\nDTSTART:20261101T063000Z\nDURATION:PT20M\nEND:VEVENT\nEND:VCALENDAR\n";
    let events = ics::parse(text, opts()).events;
    assert_eq!(
        ics::parse(
            &ics::write(&events, "test", utc("2026-10-05T12:00:00Z")),
            opts()
        )
        .events,
        events
    );
}
#[test]
fn expansion_half_open_horizon_invalid_rules_and_bounds() {
    let text = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:hourly\nDTSTART:20260301T080000Z\nRRULE:FREQ=HOURLY;UNTIL=20260301T100000Z\nEND:VEVENT\nEND:VCALENDAR\n";
    let cal = ics::parse(
        text,
        ParseOptions {
            horizon_end: utc("2026-03-01T09:00:00Z"),
            ..opts()
        },
    );
    assert_eq!(cal.events.len(), 1);
    let bad = ics::parse(
        &text.replace("FREQ=HOURLY;UNTIL=20260301T100000Z", "NOT=A-RULE"),
        opts(),
    );
    assert_eq!(bad.events.len(), 1);
    assert!(bad.events[0].rrule.is_none());
    assert!(
        bad.warnings
            .iter()
            .any(|w| w.code == ics::WarningCode::InvalidRule)
    );
    let limited = ics::parse(
        &text.replace("FREQ=HOURLY;UNTIL=20260301T100000Z", "FREQ=SECONDLY"),
        opts(),
    );
    assert!(limited.events.len() <= 10000);
    assert!(
        limited
            .warnings
            .iter()
            .any(|w| w.code == ics::WarningCode::ExpansionLimited)
    );
    let nested = format!(
        "{}{}",
        "BEGIN:VCALENDAR\n".repeat(20),
        "END:VCALENDAR\n".repeat(20)
    );
    assert!(!ics::parse(&nested, opts()).warnings.is_empty());
}
#[test]
fn cancelled_override_without_dtstart_and_invalid_events() {
    let text = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:cancel\nRECURRENCE-ID;VALUE=DATE:20260912\nSTATUS:CANCELLED\nEND:VEVENT\nBEGIN:VEVENT\nUID:invalid\nDTSTART:20260912T090000\nDURATION:PT-1H\nEND:VEVENT\nBEGIN:VEVENT\nUID:invalid-end\nDTSTART;VALUE=DATE:20260912\nDTEND:20260913T090000Z\nEND:VEVENT\nEND:VCALENDAR\n";
    let cal = ics::parse(text, opts());
    assert_eq!(cal.events.len(), 1);
    assert_eq!(cal.events[0].status, EventStatus::Cancelled);
    assert_eq!(cal.warnings.len(), 2);
}
#[test]
fn fixed_expansion_horizon_inside_fold_uses_instant_order() {
    let text = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:fold-hour\nDTSTART;TZID=America/New_York:20261101T004500\nRRULE:FREQ=HOURLY;COUNT=4\nEND:VEVENT\nEND:VCALENDAR\n";
    let cal = ics::parse(
        text,
        ParseOptions {
            horizon_end: utc("2026-11-01T06:30:00Z"),
            ..opts()
        },
    );
    assert_eq!(cal.events.len(), 2);
    assert_eq!(
        cal.events[1].start,
        IcsTime::Fixed(utc("2026-11-01T05:45:00Z"))
    );
}
#[test]
fn malformed_trailing_content_does_not_return_a_valid_prefix() {
    let text = format!("{}GARBAGE\nEND:VCALENDAR\n", fixture("google"));
    let cal = ics::parse(&text, opts());
    assert!(cal.events.is_empty());
    assert!(
        cal.warnings
            .iter()
            .any(|w| w.code == ics::WarningCode::MalformedCalendar)
    );
}
proptest! {
    #[test]
    fn garbage_never_panics(text in ".{0,4096}") { let _ = ics::parse(&text, opts()); }
    #[test]
    fn property_garbage_never_panics(value in ".{0,500}") {
        let text=format!("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:fuzz\nDTSTART:{value}\nRRULE:{value}\nEND:VEVENT\nEND:VCALENDAR\n");
        let _=ics::parse(&text,opts());
    }
}
