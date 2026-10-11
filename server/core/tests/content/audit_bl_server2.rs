use porchlight_core::{
    ics::{self, EventOrigin, IcsTime, ParseOptions, WarningCode},
    items::*,
};

#[test]
fn normalization_can_prove_inert_week_start_with_nominal_anchor() {
    let rule = "freq=weekly;interval=2;byday=SU;wkst=SU;count=4";
    assert!(ics::normalize_subset(rule).is_none());
    assert_eq!(
        ics::normalize_subset_at(rule, "2026-01-04".parse().unwrap()).as_deref(),
        Some("FREQ=WEEKLY;INTERVAL=2;BYDAY=SU;COUNT=4")
    );
    assert!(ics::normalize_subset_at(rule, "2026-01-05".parse().unwrap()).is_none());
}

#[test]
fn no_range_override_preserves_master_future_and_count_includes_anchor() {
    let calendar = "BEGIN:VCALENDAR\nVERSION:2.0\nBEGIN:VEVENT\nUID:sample\nDTSTART:20260302T090000\nDURATION:PT1H\nRRULE:FREQ=WEEKLY;BYDAY=TU;COUNT=3\nEND:VEVENT\nBEGIN:VEVENT\nUID:sample\nRECURRENCE-ID:20260303T090000\nDTSTART:20260303T110000\nDURATION:PT1H\nRRULE:FREQ=WEEKLY;COUNT=2\nEND:VEVENT\nEND:VCALENDAR\n";
    let parsed = ics::parse(
        calendar,
        ParseOptions {
            fallback_tz: chrono_tz::UTC,
            horizon_start: "2026-03-01T00:00:00Z".parse().unwrap(),
            horizon_end: "2026-04-01T00:00:00Z".parse().unwrap(),
        },
    );
    assert!(
        parsed
            .warnings
            .iter()
            .any(|w| w.code == WarningCode::UnsupportedProperty)
    );
    let master = parsed
        .events
        .iter()
        .find(|e| e.origin == EventOrigin::Series)
        .unwrap();
    let spec = SeriesSpec::new(SeriesInput {
        kind: Kind::Appointment,
        time_mode: TimeMode::Floating,
        local_start: Some("2026-03-02T09:00:00".parse().unwrap()),
        local_end: Some("2026-03-02T10:00:00".parse().unwrap()),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        rrule: master.rrule.clone(),
        ..Default::default()
    })
    .unwrap();
    let rows = expand(
        &spec,
        "2026-03-01T00:00:00".parse().unwrap(),
        "2026-04-01T00:00:00".parse().unwrap(),
    )
    .unwrap();
    assert_eq!(
        rows.iter()
            .map(|r| r.local_start.unwrap())
            .collect::<Vec<_>>(),
        [
            "2026-03-02T09:00:00",
            "2026-03-03T09:00:00",
            "2026-03-10T09:00:00"
        ]
        .map(|s| s.parse().unwrap())
    );
    assert!(
        parsed
            .events
            .iter()
            .any(|e| e.origin == EventOrigin::Override)
    );
    assert!(
        parsed
            .events
            .iter()
            .any(|e| matches!(e.origin, EventOrigin::Expanded { .. })
                && e.start == IcsTime::Floating("2026-03-10T11:00:00".parse().unwrap()))
    );
}
