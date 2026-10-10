use chrono::{DateTime, Utc};
use porchlight_core::{
    ics::{self, IcsTime, ParseOptions, WarningCode},
    items::Rule,
};

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
fn calendar(body: &str) -> String {
    format!("BEGIN:VCALENDAR\nVERSION:2.0\n{body}END:VCALENDAR\n")
}
fn event(rule: &str) -> String {
    calendar(&format!(
        "BEGIN:VEVENT\nUID:corpus\nDTSTART:20260101T090000\nDTEND:20260101T100000\nRRULE:{rule}\nEND:VEVENT\n"
    ))
}
#[test]
fn shared_rule_corpus_keep_decision_and_canonical_form() {
    for line in include_str!("fixtures/rrule_corpus.txt")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let (verdict, rule) = line.split_once(' ').unwrap();
        // ICS case/order normalization is deliberately more forgiving than raw row input.
        let expected = verdict == "accept" || matches!(rule, "freq=DAILY" | "COUNT=3;FREQ=DAILY");
        let cal = ics::parse(&event(rule), opts());
        let kept = cal.events.iter().find_map(|e| e.rrule.as_deref());
        assert_eq!(kept.is_some(), expected, "{rule}: {:?}", cal.warnings);
        if let Some(kept) = kept {
            let parsed: Rule = kept.parse().unwrap();
            assert_eq!(kept, parsed.to_string(), "{rule}");
        }
    }
}
#[test]
fn series_constructor_semantics_not_just_rule_syntax() {
    let cal = ics::parse(&event("FREQ=DAILY;UNTIL=20250101"), opts());
    assert!(cal.events.iter().all(|e| e.rrule.is_none()));
    assert!(
        cal.warnings
            .iter()
            .any(|w| matches!(w.code, WarningCode::ExpandedRule | WarningCode::InvalidRule))
    );
}
#[test]
fn expansion_removes_exdates_and_overrides_in_either_order() {
    let master = "BEGIN:VEVENT\nUID:m\nDTSTART;TZID=America/New_York:20260130T090000\nDURATION:PT1H\nRRULE:FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1;COUNT=4\nEXDATE;VALUE=DATE:20260331\nEND:VEVENT\n";
    let moved = "BEGIN:VEVENT\nUID:m\nRECURRENCE-ID;TZID=America/New_York:20260227T090000\nDTSTART;TZID=America/New_York:20260227T110000\nDURATION:PT1H\nSEQUENCE:2\nEND:VEVENT\n";
    for body in [format!("{moved}{master}"), format!("{master}{moved}")] {
        let cal = ics::parse(&calendar(&body), opts());
        assert_eq!(cal.events.len(), 3, "{:?}", cal.events);
        let keys: std::collections::BTreeSet<_> = cal
            .events
            .iter()
            .map(|e| (e.uid.clone(), e.recurrence_id.clone()))
            .collect();
        assert_eq!(keys.len(), cal.events.len());
        assert!(
            !cal.events
                .iter()
                .any(|e| e.start == IcsTime::Fixed(utc("2026-03-31T13:00:00Z")))
        );
        assert!(
            !cal.events
                .iter()
                .any(|e| e.start == IcsTime::Fixed(utc("2026-02-27T14:00:00Z")))
        );
    }
}
#[test]
fn bom_salvage_and_warning_codes() {
    let cal = ics::parse(&format!("\u{feff}{}", calendar("BEGIN:VEVENT\nUID:a\nDTSTART;VALUE=DATE:20260301\nDTEND;VALUE=DATE:20260301\nSEQUENCE:-1\nEND:VEVENT\n").replace("END:VCALENDAR\n", "END:VCALENDAR  \t\n")), opts());
    assert_eq!(cal.events.len(), 1, "{:?}", cal.warnings);
    assert_eq!(cal.events[0].sequence, 0);
    assert_ne!(cal.events[0].start, cal.events[0].end);
    assert_eq!(
        cal.warnings
            .iter()
            .filter(|w| w.code == WarningCode::InvalidEvent)
            .count(),
        2
    );
}
#[test]
fn unknown_timezone_warns_once_per_uid_and_name() {
    let body = "BEGIN:VEVENT\nUID:u\nDTSTART;TZID=Mars:20260301T090000\nDTEND;TZID=Mars:20260301T100000\nEXDATE;TZID=Mars:20260302T090000\nEND:VEVENT\n";
    let cal = ics::parse(&calendar(body), opts());
    assert_eq!(
        cal.warnings
            .iter()
            .filter(|w| w.code == WarningCode::UnknownTimezone)
            .count(),
        1
    );
}
#[test]
fn unsynchronized_dtstart_is_still_first_and_counts() {
    let text = event("FREQ=MONTHLY;BYMONTHDAY=15;BYHOUR=9;COUNT=2");
    let cal = ics::parse(&text, opts());
    assert_eq!(cal.events.len(), 2);
    assert_eq!(
        cal.events[0].start,
        IcsTime::Floating(utc("2026-01-01T09:00:00Z").naive_utc())
    );
    assert_eq!(
        cal.events[1].start,
        IcsTime::Floating(utc("2026-01-15T09:00:00Z").naive_utc())
    );
    let impossible = ics::parse(
        &event("FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30;BYHOUR=1"),
        opts(),
    );
    assert_eq!(impossible.events.len(), 1);
}
#[test]
fn wkst_outlook_inert_but_sensitive_weekly_expands() {
    for rule in [
        "FREQ=WEEKLY;BYDAY=MO,WE;WKST=SU",
        "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;WKST=MO",
        // January 1 is Thursday: non-MO WKST is inert only for that weekday.
        "FREQ=WEEKLY;INTERVAL=2;BYDAY=TH;WKST=SU",
    ] {
        let cal = ics::parse(&event(rule), opts());
        assert!(cal.events[0].rrule.is_some(), "{rule}");
        assert!(!cal.events[0].rrule.as_ref().unwrap().contains("WKST"));
    }
    let cal = ics::parse(&event("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;WKST=SU"), opts());
    assert!(cal.events.iter().all(|e| e.rrule.is_none()));
    assert!(
        cal.warnings
            .iter()
            .any(|w| w.code == WarningCode::ExpandedRule)
    );
}
#[test]
fn windows_zone_gaps() {
    for (windows, iana) in [
        ("Aleutian Standard Time", "America/Adak"),
        ("Yukon Standard Time", "America/Whitehorse"),
        ("Lord Howe Standard Time", "Australia/Lord_Howe"),
        ("Aus Central W. Standard Time", "Australia/Eucla"),
        ("Kaliningrad Standard Time", "Europe/Kaliningrad"),
        ("Belarus Standard Time", "Europe/Minsk"),
        ("Volgograd Standard Time", "Europe/Volgograd"),
    ] {
        let cal = ics::parse(
            &calendar(&format!(
                "BEGIN:VEVENT\nUID:zone\nDTSTART;TZID=\"{windows}\":20260301T090000\nEND:VEVENT\n"
            )),
            opts(),
        );
        assert!(cal.warnings.is_empty(), "{windows}: {:?}", cal.warnings);
        assert_eq!(cal.events[0].tzid.as_deref(), Some(iana));
    }
}
#[test]
fn duplicate_identity_is_never_returned() {
    let text = event("FREQ=DAILY;COUNT=2");
    let duplicate = text.replace(
        "END:VCALENDAR",
        "BEGIN:VEVENT\nUID:corpus\nDTSTART:20260101T100000\nSEQUENCE:3\nEND:VEVENT\nEND:VCALENDAR",
    );
    let cal = ics::parse(&duplicate, opts());
    assert_eq!(cal.events.len(), 1);
    assert_eq!(cal.events[0].sequence, 3);
    assert!(
        cal.warnings
            .iter()
            .any(|w| w.code == WarningCode::InvalidEvent)
    );
}

#[test]
fn old_daily_byhour_expands_only_inside_half_open_window() {
    let text = event("FREQ=DAILY;BYHOUR=8,12,18").replace("20260101", "20100101");
    let options = ParseOptions {
        horizon_start: utc("2026-03-01T13:00:00Z"),
        horizon_end: utc("2026-03-03T13:00:00Z"),
        ..opts()
    };
    let cal = ics::parse(&text, options);
    assert_eq!(cal.events.len(), 6, "{:?}", cal.warnings);
    assert_eq!(
        cal.events[0].start,
        IcsTime::Floating(utc("2026-03-01T08:00:00Z").naive_utc())
    );
    assert!(
        cal.events
            .iter()
            .all(|e| matches!(e.origin, ics::EventOrigin::Expanded { .. }))
    );
    assert!(
        !cal.warnings
            .iter()
            .any(|w| w.code == WarningCode::ExpansionLimited)
    );
    let empty = ics::parse(
        &text,
        ParseOptions {
            horizon_start: options.horizon_end,
            ..options
        },
    );
    assert!(empty.events.is_empty());
}
#[test]
fn window_cap_does_not_count_the_lower_wall_guard() {
    let text = event("FREQ=SECONDLY")
        .replace("DTSTART:20260101T090000", "DTSTART:20260301T090000Z")
        .replace("DTEND:20260101T100000", "DTEND:20260301T100000Z");
    let options = ParseOptions {
        horizon_start: utc("2026-03-02T00:00:00Z"),
        horizon_end: utc("2026-03-02T00:00:10Z"),
        ..opts()
    };
    let cal = ics::parse(&text, options);
    assert_eq!(cal.events.len(), 10, "{:?}", cal.warnings);
    assert_eq!(cal.events[0].start, IcsTime::Fixed(options.horizon_start));
    assert!(
        !cal.warnings
            .iter()
            .any(|w| w.code == WarningCode::ExpansionLimited)
    );
}
#[test]
fn expanded_origin_and_cancelled_override_are_distinct() {
    let master = "BEGIN:VEVENT\nUID:m\nDTSTART:20260301T090000\nDURATION:PT1H\nRRULE:FREQ=DAILY;BYHOUR=9;COUNT=3\nEND:VEVENT\n";
    let cancelled =
        "BEGIN:VEVENT\nUID:m\nRECURRENCE-ID:20260302T090000\nSTATUS:CANCELLED\nEND:VEVENT\n";
    let cal = ics::parse(&calendar(&format!("{master}{cancelled}")), opts());
    assert_eq!(cal.events.len(), 3);
    assert_eq!(
        cal.events
            .iter()
            .filter(|e| e.origin == ics::EventOrigin::Override)
            .count(),
        1
    );
    for e in cal
        .events
        .iter()
        .filter(|e| matches!(e.origin, ics::EventOrigin::Expanded { .. }))
    {
        assert_eq!(e.recurrence_id.as_ref(), Some(&e.start));
        assert!(e.exdates.is_empty());
    }
}
#[test]
fn writer_strips_text_controls_and_uses_supplied_dtstamp() {
    let mut events = ics::parse(&event("FREQ=DAILY;COUNT=2"), opts()).events;
    events[0].summary = "safe\0\x07\x7f\t\ntext".to_owned();
    let out = ics::write(&events, "name\x01", utc("2026-10-05T12:34:56Z"));
    assert!(out.contains("DTSTAMP:20261005T123456Z\r\n"));
    assert!(!out.contains(['\0', '\x07', '\x7f', '\x01']));
    assert!(out.contains("SUMMARY:safe\t\\ntext"));
}
use proptest::prelude::*;
proptest! {
    #![proptest_config(ProptestConfig { cases: 96, rng_seed: proptest::test_runner::RngSeed::Fixed(5545), ..ProptestConfig::default() })]
    #[test]
    fn mutated_valid_fixture_reaches_properties_without_panics(
        value in "[^\\r\\n]{0,160}",
        zone in prop_oneof![Just("Mars"), Just("Eastern Standard Time"), Just("America/New_York")],
        freq in prop_oneof![Just("DAILY"), Just("MONTHLY"), Just("HOURLY"), Just("SECONDLY"), Just("BROKEN")],
        count in 0u32..10020,
        nested in any::<bool>(),
    ) {
        let noise = if nested { "BEGIN:VALARM\nACTION:DISPLAY\nTRIGGER:-PT15M\nDESCRIPTION:Reminder\nEND:VALARM\n" } else { "" };
        let body = format!("BEGIN:VEVENT\nUID:mutated\nDTSTAMP:20261005T123456Z\nDTSTART;TZID=\"{zone}\":20260301T090000\nDTEND;TZID=\"{zone}\":20260301T100000\nSUMMARY:{value}\nATTENDEE;CN=\"A, B; C: D\":mailto:guest@example.invalid\nX-MUTATED;P=\"a:;b\":{value}\nRRULE:FREQ={freq};COUNT={count};BYHOUR=9\n{noise}END:VEVENT\n");
        let cal = ics::parse(&calendar(&body), opts());
        prop_assert!(!cal.warnings.iter().any(|w| w.code == WarningCode::MalformedCalendar));
        let keys: std::collections::BTreeSet<_> = cal.events.iter().map(|e| (&e.uid, &e.recurrence_id)).collect();
        prop_assert_eq!(keys.len(), cal.events.len());
    }
}

#[test]
fn expanded_later_fold_dtstart_is_authoritative_not_two_instances() {
    let text = calendar(
        "X-WR-TIMEZONE:America/New_York\nBEGIN:VEVENT\nUID:later\nDTSTART:20261101T063000Z\nDURATION:PT20M\nRRULE:FREQ=DAILY;BYHOUR=1;COUNT=2\nEND:VEVENT\n",
    );
    let cal = ics::parse(
        &text,
        ParseOptions {
            horizon_start: utc("2026-11-01T00:00:00Z"),
            horizon_end: utc("2026-11-03T00:00:00Z"),
            ..opts()
        },
    );
    assert_eq!(cal.events.len(), 2);
    assert_eq!(
        cal.events[0].start,
        IcsTime::Fixed(utc("2026-11-01T06:30:00Z"))
    );
    assert_eq!(
        cal.events[1].start,
        IcsTime::Fixed(utc("2026-11-02T06:30:00Z"))
    );
}
#[test]
fn malformed_inputs_report_specific_codes_and_no_events() {
    for text in [
        "",
        "garbage",
        "BEGIN:VCALENDAR\nBEGIN:VEVENT\nBAD-LINE\nEND:VEVENT\nEND:VCALENDAR\n",
    ] {
        let cal = ics::parse(text, opts());
        assert!(cal.events.is_empty());
        assert_eq!(cal.warnings[0].code, WarningCode::MalformedCalendar);
    }
    let cal = ics::parse(
        &calendar("BEGIN:VEVENT\nUID:bad\nDTSTART:not-a-time\nEND:VEVENT\n"),
        opts(),
    );
    assert!(cal.events.is_empty());
    assert_eq!(cal.warnings[0].code, WarningCode::InvalidEvent);
}
