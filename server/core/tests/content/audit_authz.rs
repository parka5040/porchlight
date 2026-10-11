use porchlight_core::ics::{self, ParseOptions, WarningCode};

fn options() -> ParseOptions {
    ParseOptions {
        fallback_tz: chrono_tz::UTC,
        horizon_start: "2026-01-01T00:00:00Z".parse().unwrap(),
        horizon_end: "2028-01-01T00:00:00Z".parse().unwrap(),
    }
}
fn event(uid: &str, fields: &str, count: usize) -> String {
    format!(
        "BEGIN:VEVENT\r\nUID:{uid}\r\nDTSTART:20260101T090000Z\r\nDTEND:20260101T100000Z\r\nRRULE:FREQ=HOURLY;COUNT={count}\r\n{fields}\r\nEND:VEVENT\r\n"
    )
}
fn calendar(events: &str) -> String {
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{events}END:VCALENDAR\r\n")
}

#[test]
fn oversized_text_is_rejected_before_expansion() {
    // Two starts reproduce the amplification safely on unfixed code, not 39 GiB.
    for fields in [
        format!("SUMMARY:{}", "é".repeat(201)),
        format!("DESCRIPTION:{}", "d".repeat(4 * 1024 * 1024)),
        format!("LOCATION:{}", "界".repeat(501)),
    ] {
        let parsed = ics::parse(&calendar(&event("oversized", &fields, 2)), options());
        assert!(
            parsed.events.is_empty(),
            "oversized text must not be cloned into events"
        );
        assert!(
            parsed
                .warnings
                .iter()
                .any(|w| w.code == WarningCode::InvalidEvent)
        );
    }
}

#[test]
fn invalid_text_protects_the_whole_uid_before_expansion() {
    let valid = event("shared", "SUMMARY:Valid", 2);
    let invalid = event(
        "shared",
        &format!("SUMMARY:Invalid\r\nDESCRIPTION:{}", "d".repeat(20_001)),
        2,
    );
    let parsed = ics::parse(&calendar(&(valid + &invalid)), options());
    assert!(
        parsed.events.is_empty(),
        "an invalid source event must protect the entire UID"
    );
}

#[test]
fn oversized_uid_is_not_retained_in_events_or_warnings() {
    let parsed = ics::parse(
        &calendar(&event(&"u".repeat(1025), "SUMMARY:Title", 2)),
        options(),
    );
    assert!(
        parsed.events.is_empty(),
        "oversized UID must be rejected before expansion"
    );
    assert!(
        parsed
            .warnings
            .iter()
            .any(|w| w.code == WarningCode::InvalidEvent)
    );
    assert!(
        parsed
            .warnings
            .iter()
            .all(|w| w.uid.as_ref().is_none_or(|u| u.len() <= 1024))
    );
}

#[test]
fn expansion_has_a_parse_wide_retained_byte_budget() {
    // Each source is valid on its own; their combined output must share admission.
    let fields = format!("SUMMARY:Title\r\nDESCRIPTION:{}", "d".repeat(20_000));
    let source = calendar(&(event("one", &fields, 1000) + &event("two", &fields, 1000)));
    let parsed = ics::parse(&source, options());
    let retained: usize = parsed
        .events
        .iter()
        .map(|e| e.uid.len() + e.summary.len() + e.description.len() + e.location.len())
        .sum();
    assert!(
        retained <= 32 * 1024 * 1024,
        "expanded text exceeded the parse-wide 32 MiB budget: {retained}"
    );
    assert!(
        parsed
            .warnings
            .iter()
            .any(|w| w.code == WarningCode::ExpansionLimited)
    );
}

#[test]
fn invalid_exdates_coalesce_without_uid_amplification() {
    // Safe on the old parser: about 2 MiB, never the multi-GiB attack payload.
    let source = calendar(&event(
        &"u".repeat(1024),
        &format!("SUMMARY:Title\r\nEXDATE:{}", ",".repeat(2000)),
        1,
    ));
    let parsed = ics::parse(&source, options());
    assert_eq!(
        parsed
            .warnings
            .iter()
            .filter(|w| w.message == "Invalid EXDATE ignored")
            .count(),
        1,
        "invalid EXDATE warnings must coalesce before cloning the UID"
    );
}

#[test]
fn diagnostics_have_count_and_retained_byte_caps() {
    let source = calendar(
        &(0..512)
            .map(|n| {
                format!(
                    "BEGIN:VEVENT\r\nUID:{n:04}{}\r\nEND:VEVENT\r\n",
                    "u".repeat(1020)
                )
            })
            .collect::<String>(),
    );
    let parsed = ics::parse(&source, options());
    let retained: usize = parsed
        .warnings
        .iter()
        .map(|w| std::mem::size_of_val(w) + w.uid.as_ref().map_or(0, String::len) + w.message.len())
        .sum();
    assert!(
        retained <= 64 * 1024,
        "diagnostics exceeded the retained-byte cap: {retained}"
    );
    assert!(
        parsed.warnings.len() <= 128,
        "diagnostics exceeded the count cap"
    );
    assert!(
        parsed.warnings.iter().any(|w| w.uid.is_none()
            && w.code == WarningCode::ExpansionLimited
            && w.message.contains("Diagnostics truncated")),
        "truncation must suppress unsafe reconciliation globally"
    );
}

#[test]
fn exdate_property_work_is_bounded() {
    let source = calendar(&event(
        "bounded",
        &format!("SUMMARY:Title\r\nEXDATE:{}", ",".repeat(10_001)),
        1,
    ));
    let parsed = ics::parse(&source, options());
    assert!(
        parsed.events.is_empty(),
        "an incomplete EXDATE property must not be emitted"
    );
    assert!(
        parsed
            .warnings
            .iter()
            .any(|w| w.code == WarningCode::ExpansionLimited)
    );
}

#[test]
fn valid_unicode_text_limits_are_preserved() {
    let fields = format!(
        "SUMMARY:{}\r\nDESCRIPTION:{}\r\nLOCATION:{}",
        "é".repeat(200),
        "界".repeat(20_000),
        "é".repeat(500)
    );
    let parsed = ics::parse(&calendar(&event("valid", &fields, 2)), options());
    assert_eq!(parsed.events.len(), 2);
    assert_eq!(parsed.events[0].description, "界".repeat(20_000));
    assert!(
        !parsed
            .warnings
            .iter()
            .any(|w| w.code == WarningCode::InvalidEvent)
    );
}
