use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use porchlight_core::{
    items::*,
    time::{parse_tz, to_local},
};
use proptest::prelude::*;

fn l(s: &str) -> NaiveDateTime {
    s.parse().unwrap()
}
fn u(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}
fn gap_series() -> SeriesSpec {
    SeriesSpec::new(SeriesInput {
        kind: Kind::Appointment,
        tzid: Some("America/New_York".into()),
        starts_at: Some(u("2026-03-01T07:10:00Z")),
        ends_at: Some(u("2026-03-01T07:50:00Z")),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        rrule: Some("FREQ=DAILY".into()),
        ..Default::default()
    })
    .unwrap()
}
fn restore(spec: &SeriesSpec) -> SeriesSpec {
    SeriesSpec::new_with_calendar_cursor(spec.input().clone(), spec.calendar_cursor()).unwrap()
}
#[test]
fn probes_a1_a2_horizon_derives_through_gap() {
    let spec = gap_series();
    let zone = parse_tz("America/New_York").unwrap();
    let from = u("2026-10-05T12:00:00Z");
    let end = horizon_end(from, zone).unwrap();
    let rows = expand(&spec, to_local(from, zone), to_local(end, zone)).unwrap();
    let gap = rows
        .iter()
        .find(|r| r.local_start == Some(l("2027-03-14T02:10:00")))
        .unwrap();
    assert_eq!(gap.starts_at, Some(u("2027-03-14T07:00:00Z")));
    assert_eq!(gap.ends_at, Some(u("2027-03-14T07:40:00Z")));
    assert_eq!(
        expand(&spec, l("2026-04-01T00:00:00"), l("2027-03-01T00:00:00"))
            .unwrap()
            .len(),
        334
    );
}
#[test]
fn probes_a3_a4_gap_overrides_and_splits() {
    let spec = gap_series();
    let replacement = OccurrenceAnchor {
        local_start: Some(l("2026-03-08T02:10:00")),
        local_end: Some(l("2026-03-08T02:50:00")),
        ..Default::default()
    };
    assert_eq!(
        this_occurrence(&spec, 7, &replacement)
            .unwrap()
            .override_local_end,
        replacement.local_end
    );
    let (head, tail) = this_and_future(&spec, 7).unwrap();
    let a = l("2026-03-01T00:00:00");
    let c = l("2026-03-11T00:00:00");
    let mut combined = expand(&restore(&head.unwrap()), a, c).unwrap();
    let mut rest = expand(&restore(&tail), a, c).unwrap();
    for row in &mut rest {
        row.seq += 7;
    }
    combined.extend(rest);
    assert_eq!(combined, expand(&spec, a, c).unwrap());
}
#[test]
fn probe_a5_completion_gap_span() {
    let mut input = gap_series().input().clone();
    input.rrule = None;
    input.recurrence_mode = Some(RecurrenceMode::Completion);
    input.completion_interval = Some(PgInterval {
        days: 1,
        ..Default::default()
    });
    let next = next_after_completion(
        &SeriesSpec::new(input).unwrap(),
        u("2026-03-07T07:10:00Z"),
        parse_tz("UTC").unwrap(),
    )
    .unwrap();
    assert_eq!(next.local_start, Some(l("2026-03-08T02:10:00")));
    assert_eq!(next.local_end, Some(l("2026-03-08T02:50:00")));
    assert_eq!(next.starts_at, Some(u("2026-03-08T07:00:00Z")));
    assert_eq!(next.ends_at, Some(u("2026-03-08T07:40:00Z")));
}
#[test]
fn probes_b1_b2_b3_fixed_initial_instants_are_authoritative() {
    for recurring in [false, true] {
        for (kind, due, start, end) in [
            (Kind::Deadline, Some("2026-11-01T06:30:00Z"), None, None),
            (
                Kind::Appointment,
                None,
                Some("2026-11-01T06:10:00Z"),
                Some("2026-11-01T06:50:00Z"),
            ),
            (
                Kind::Appointment,
                None,
                Some("2026-11-01T05:50:00Z"),
                Some("2026-11-01T06:10:00Z"),
            ),
        ] {
            let input = SeriesInput {
                kind,
                tzid: Some("America/New_York".into()),
                due_at: due.map(u),
                starts_at: start.map(u),
                ends_at: end.map(u),
                recurrence_mode: recurring.then_some(RecurrenceMode::Calendar),
                rrule: recurring.then(|| "FREQ=DAILY;COUNT=2".into()),
                ..Default::default()
            };
            let spec = SeriesSpec::new(input.clone()).unwrap();
            let rows = expand(&spec, l("2026-11-01T00:00:00"), l("2026-11-03T00:00:00")).unwrap();
            assert_eq!(
                (rows[0].due_at, rows[0].starts_at, rows[0].ends_at),
                (input.due_at, input.starts_at, input.ends_at)
            );
            if recurring && start == Some("2026-11-01T05:50:00Z") {
                assert_eq!(rows[0].local_end, Some(l("2026-11-01T01:10:00")));
                assert_eq!(rows[1].local_end, Some(l("2026-11-02T02:10:00")));
            }
        }
    }
}
#[test]
fn floating_tail_has_no_phase_metadata() {
    let spec = SeriesSpec::new(SeriesInput {
        kind: Kind::Deadline,
        time_mode: TimeMode::Floating,
        local_due: Some(l("2026-01-01T08:00:00")),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        rrule: Some("FREQ=MONTHLY;INTERVAL=2;BYDAY=2TU,-1FR;COUNT=12".into()),
        ..Default::default()
    })
    .unwrap();
    let (_, tail) = this_and_future(&spec, 2).unwrap();
    assert!(tail.calendar_cursor().is_none());
    assert_eq!(restore(&tail), tail);
}
#[test]
fn unsatisfiable_ordinal_filters_rejected_but_possible_unions_accepted() {
    for rule in [
        "FREQ=MONTHLY;BYDAY=5MO;BYMONTHDAY=1",
        "FREQ=MONTHLY;BYDAY=1MO;BYMONTHDAY=31",
    ] {
        assert_eq!(rule.parse::<Rule>().unwrap_err().field, "rrule", "{rule}");
    }
    for rule in [
        "FREQ=MONTHLY;BYDAY=5MO,TU;BYMONTHDAY=1",
        "FREQ=MONTHLY;BYDAY=1MO;BYMONTHDAY=-31",
        "FREQ=YEARLY;BYDAY=5MO;BYMONTHDAY=29",
        "FREQ=YEARLY;BYDAY=5MO;BYMONTHDAY=1",
    ] {
        assert!(rule.parse::<Rule>().is_ok(), "{rule}");
    }
}
#[test]
fn validation_uses_wire_fields() {
    assert_eq!("invalid".parse::<Kind>().unwrap_err().field, "kind");
    assert_eq!(
        "invalid".parse::<MissedPolicy>().unwrap_err().field,
        "missed_policy"
    );
    let floating = SeriesInput {
        kind: Kind::Deadline,
        time_mode: TimeMode::Floating,
        ..Default::default()
    };
    assert_eq!(
        SeriesSpec::new(floating.clone()).unwrap_err().field,
        "local_due"
    );
    let mut bad_zone = floating;
    bad_zone.local_due = Some(l("2026-01-01T00:00:00"));
    bad_zone.tzid = Some("UTC".into());
    assert_eq!(SeriesSpec::new(bad_zone).unwrap_err().field, "tzid");
    assert_eq!(
        SeriesSpec::new(SeriesInput {
            kind: Kind::Appointment,
            time_mode: TimeMode::Floating,
            local_start: Some(l("2026-01-01T00:00:00")),
            local_end: Some(l("2026-01-01T00:00:00")),
            ..Default::default()
        })
        .unwrap_err()
        .field,
        "local_end"
    );
}
#[test]
fn span_helper_and_agenda_share_items_enums() {
    use porchlight_core::time::{ResolveSpanError, resolve_span};
    let zone = parse_tz("America/New_York").unwrap();
    assert_eq!(
        resolve_span(l("2026-03-08T02:10:00"), l("2026-03-08T02:20:00"), zone).unwrap(),
        (u("2026-03-08T07:00:00Z"), u("2026-03-08T07:10:00Z"))
    );
    assert_eq!(
        resolve_span(l("2026-03-08T02:20:00"), l("2026-03-08T02:10:00"), zone),
        Err(ResolveSpanError::InvalidSpan)
    );
    let _: porchlight_core::agenda::Kind = Kind::Appointment;
    let _: porchlight_core::agenda::TimeMode = TimeMode::Floating;
}
#[test]
fn filtered_superset_matches_rrule_candidates() {
    use rrule::{RRule, RRuleSet, Unvalidated};
    // Compare to the unmodified library on satisfiable rules, including YEARLY
    // defaults, year-wide ordinals, negative month days and mixed filters.
    for rule in [
        "FREQ=DAILY;INTERVAL=2;BYDAY=MO,FR",
        "FREQ=WEEKLY;INTERVAL=3;BYDAY=SA,SU",
        "FREQ=MONTHLY;INTERVAL=2;BYDAY=2TU,-1FR",
        "FREQ=MONTHLY;BYDAY=1MO;BYMONTHDAY=-31",
        "FREQ=MONTHLY;BYMONTHDAY=31",
        "FREQ=MONTHLY;INTERVAL=3",
        "FREQ=YEARLY",
        "FREQ=YEARLY;BYDAY=-1FR",
        "FREQ=YEARLY;BYDAY=2TU",
        "FREQ=YEARLY;INTERVAL=3;BYDAY=+5MO",
        "FREQ=YEARLY;BYDAY=MO",
        "FREQ=YEARLY;BYMONTHDAY=-31,15",
        "FREQ=YEARLY;BYDAY=5MO;BYMONTHDAY=1",
        "FREQ=YEARLY;BYDAY=-1FR;BYMONTHDAY=-1",
    ] {
        let start = l("2024-02-29T08:00:00");
        let end = l("2040-01-01T00:00:00");
        let dtstart = start.and_utc().with_timezone(&rrule::Tz::UTC);
        let rule_set = RRuleSet::new(dtstart)
            .rrule(
                rule.parse::<RRule<Unvalidated>>()
                    .unwrap()
                    .until(end.and_utc().with_timezone(&rrule::Tz::UTC))
                    .validate(dtstart)
                    .unwrap(),
            )
            .rdate(dtstart);
        let mut expected: Vec<_> = (&rule_set)
            .into_iter()
            .map(|t| t.naive_utc())
            .take_while(|t| *t < end)
            .collect();
        expected.dedup();
        let spec = SeriesSpec::new(SeriesInput {
            kind: Kind::Deadline,
            time_mode: TimeMode::Floating,
            local_due: Some(start),
            recurrence_mode: Some(RecurrenceMode::Calendar),
            rrule: Some(rule.into()),
            ..Default::default()
        })
        .unwrap();
        let actual: Vec<_> = expand(&spec, start, end)
            .unwrap()
            .iter()
            .map(|a| a.local_due.unwrap())
            .collect();
        assert_eq!(actual, expected, "{rule}");
    }
}
#[test]
fn fixed_deadline_gap_metadata_is_optional_and_all_edit_clear_is_explicit() {
    let spec = SeriesSpec::new(SeriesInput {
        kind: Kind::Deadline,
        tzid: Some("America/New_York".into()),
        due_at: Some(u("2026-03-06T07:30:00Z")),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        rrule: Some("FREQ=DAILY;COUNT=5".into()),
        ..Default::default()
    })
    .unwrap();
    let a = l("2026-03-06T00:00:00");
    let c = l("2026-03-12T00:00:00");
    let rows = expand(&spec, a, c).unwrap();
    let (_, tail) = this_and_future(&spec, 2).unwrap();
    let saved = tail.calendar_cursor().unwrap();
    assert_eq!(saved.nominal_local_anchor, l("2026-03-08T02:30:00"));
    assert_eq!(saved.wall_duration, None);
    let restored = restore(&tail);
    let mut rest = expand(&restored, a, c).unwrap();
    for row in &mut rest {
        row.seq += 2;
    }
    assert_eq!(rest, rows[2..]);
    let (_, ordinary_tail) = this_and_future(&restored, 1).unwrap();
    assert!(ordinary_tail.calendar_cursor().is_none());
    assert_eq!(all(&tail).calendar_cursor(), Some(saved));
    let mut edited = all(&tail).input().clone();
    edited.due_at = Some(u("2026-03-08T08:00:00Z"));
    let edited = SeriesSpec::new(edited).unwrap(); // timing edit clears both metadata columns
    assert!(edited.calendar_cursor().is_none());
    assert_eq!(
        expand(&edited, a, c).unwrap()[0].local_due,
        Some(l("2026-03-08T04:00:00"))
    );
}
#[test]
fn fixed_tail_keeps_duration_when_only_end_is_in_gap() {
    let spec = SeriesSpec::new(SeriesInput {
        kind: Kind::Appointment,
        tzid: Some("America/New_York".into()),
        starts_at: Some(u("2026-03-07T06:30:00Z")),
        ends_at: Some(u("2026-03-07T07:30:00Z")),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        rrule: Some("FREQ=DAILY;COUNT=3".into()),
        ..Default::default()
    })
    .unwrap();
    let a = l("2026-03-07T00:00:00");
    let c = l("2026-03-11T00:00:00");
    let original = expand(&spec, a, c).unwrap();
    let (_, tail) = this_and_future(&spec, 1).unwrap();
    assert_eq!(
        tail.calendar_cursor().unwrap().wall_duration,
        Some(Duration::hours(1))
    );
    let mut rest = expand(&restore(&tail), a, c).unwrap();
    for row in &mut rest {
        row.seq += 1;
    }
    assert_eq!(rest, original[1..]);
}
#[test]
fn floating_missed_boundary_uses_corrected_gap_span() {
    let rows = [OpenOccurrence {
        closed_at: None,
        generated_anchor: None,
        disposition: Disposition::Open,
        anchor: OccurrenceAnchor {
            local_start: Some(l("2026-03-08T02:10:00")),
            local_end: Some(l("2026-03-08T02:20:00")),
            ..Default::default()
        },
        status: Status::Todo,
    }];
    let zone = parse_tz("America/New_York").unwrap();
    assert_eq!(
        dispositions(&rows, u("2026-03-08T07:05:00Z"), MissedPolicy::Stack, zone).unwrap(),
        vec![Disposition::Open]
    );
    assert_eq!(
        dispositions(&rows, u("2026-03-08T07:11:00Z"), MissedPolicy::Stack, zone).unwrap(),
        vec![Disposition::Overdue]
    );
}
#[test]
fn empty_active_periods_terminate_without_losing_dtstart() {
    for (start, rule) in [
        ("2026-01-05T08:00:00", "FREQ=DAILY;INTERVAL=7;BYDAY=TU"),
        (
            "2026-02-01T08:00:00",
            "FREQ=MONTHLY;INTERVAL=12;BYMONTHDAY=31",
        ),
    ] {
        let spec = SeriesSpec::new(SeriesInput {
            kind: Kind::Deadline,
            time_mode: TimeMode::Floating,
            local_due: Some(l(start)),
            recurrence_mode: Some(RecurrenceMode::Calendar),
            rrule: Some(rule.into()),
            ..Default::default()
        })
        .unwrap();
        let rows = expand(&spec, l(start), l("2027-12-31T23:59:59")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].local_due, Some(l(start)));
        assert_eq!(
            this_occurrence(&spec, 1, &rows[0]).unwrap_err().field,
            "seq"
        );
    }
}
proptest! {
    #![proptest_config(ProptestConfig {
        cases: 40, failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20261005),
        ..ProptestConfig::default()
    })]
    #[test]
    fn gap_collapses_resolve_and_split_rehydrates(minute in 0u32..50, duration in 1i64..10) {
        let start = l("2026-03-07T02:00:00") + Duration::minutes(i64::from(minute));
        let zone = parse_tz("America/New_York").unwrap();
        let spec = SeriesSpec::new(SeriesInput {
            kind: Kind::Appointment, tzid: Some("America/New_York".into()),
            starts_at: Some(porchlight_core::time::resolve_local(start, zone)),
            ends_at: Some(porchlight_core::time::resolve_local(start + Duration::minutes(duration), zone)),
            recurrence_mode: Some(RecurrenceMode::Calendar),
            rrule: Some("FREQ=DAILY;COUNT=4".into()),
            ..Default::default()
        }).unwrap();
        let a = l("2026-03-07T00:00:00");
        let c = l("2026-03-11T00:00:00");
        let rows = expand(&spec, a, c).unwrap();
        prop_assert_eq!(rows[1].starts_at, Some(u("2026-03-08T07:00:00Z")));
        prop_assert_eq!(rows[1].ends_at, Some(u("2026-03-08T07:00:00Z") + Duration::minutes(duration)));
        let (head, tail) = this_and_future(&spec, 1).unwrap();
        let mut combined = expand(&restore(&head.unwrap()), a, c).unwrap();
        let mut rest = expand(&restore(&tail), a, c).unwrap();
        for row in &mut rest { row.seq += 1; }
        combined.extend(rest);
        prop_assert_eq!(combined, rows);
    }
}
