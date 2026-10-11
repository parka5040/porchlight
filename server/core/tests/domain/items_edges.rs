use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use porchlight_core::{items::*, time::parse_tz};
use proptest::prelude::*;
fn l(s: &str) -> NaiveDateTime {
    s.parse().unwrap()
}
fn u(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}
fn d(s: &str) -> NaiveDate {
    s.parse().unwrap()
}
fn calendar(rule: &str) -> SeriesSpec {
    SeriesSpec::new(SeriesInput {
        kind: Kind::Deadline,
        time_mode: TimeMode::Floating,
        local_due: Some(l("2026-01-01T08:00:00")),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        rrule: Some(rule.into()),
        ..Default::default()
    })
    .unwrap()
}
#[test]
fn db_shapes_reject_forbidden_anchors_and_bad_recurrence_combinations() {
    let base = SeriesInput {
        kind: Kind::Appointment,
        time_mode: TimeMode::Floating,
        local_start: Some(l("2026-05-01T14:00:00")),
        local_end: Some(l("2026-05-01T15:00:00")),
        ..Default::default()
    };
    let mut invalid = base.clone();
    invalid.tzid = Some("UTC".into());
    assert!(SeriesSpec::new(invalid).is_err());
    let mut invalid = base.clone();
    invalid.local_end = invalid.local_start;
    assert!(SeriesSpec::new(invalid).is_err());
    let mut invalid = base.clone();
    invalid.local_end = None;
    assert!(SeriesSpec::new(invalid).is_err());
    let mut invalid = base.clone();
    invalid.due_date = Some(d("2026-05-01"));
    assert!(SeriesSpec::new(invalid).is_err());
    let mut invalid = base.clone();
    invalid.all_day = true;
    assert!(SeriesSpec::new(invalid).is_err());
    let mut invalid = base.clone();
    invalid.time_mode = TimeMode::Fixed;
    assert!(SeriesSpec::new(invalid).is_err());
    let mut invalid = base.clone();
    invalid.rrule = Some("FREQ=DAILY".into());
    assert!(SeriesSpec::new(invalid).is_err());
    let mut invalid = base.clone();
    invalid.recurrence_mode = Some(RecurrenceMode::Completion);
    invalid.rrule = Some("FREQ=DAILY".into());
    invalid.completion_interval = Some(PgInterval {
        days: 1,
        ..Default::default()
    });
    assert!(SeriesSpec::new(invalid).is_err());
    let invalid = SeriesInput {
        tzid: Some("Mars/Olympus".into()),
        ..Default::default()
    };
    assert!(SeriesSpec::new(invalid).is_err());
    let invalid = SeriesInput {
        recurrence_mode: Some(RecurrenceMode::Calendar),
        rrule: Some("FREQ=DAILY".into()),
        ..Default::default()
    };
    assert_eq!(SeriesSpec::new(invalid).unwrap_err().field, "due_date");
    // Migration 0003 deliberately does not forbid tzid/all_day on todos.
    assert!(
        SeriesSpec::new(SeriesInput {
            tzid: Some("UTC".into()),
            all_day: true,
            ..Default::default()
        })
        .is_ok()
    );
}
#[test]
fn windows_and_subseconds_preserve_seq() {
    let mut input = calendar("FREQ=DAILY;COUNT=3").input().clone();
    input.local_due = Some(l("2026-01-01T08:00:00.123456"));
    let spec = SeriesSpec::new(input).unwrap();
    let rows = expand(
        &spec,
        l("2026-01-02T08:00:00.123456"),
        l("2026-01-03T08:00:00.123456"),
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].seq, 1);
    assert_eq!(rows[0].local_due, Some(l("2026-01-02T08:00:00.123456")));
    assert!(
        expand(&spec, l("2026-01-01T00:00:00"), l("2026-01-01T00:00:00"))
            .unwrap()
            .is_empty()
    );
    assert!(expand(&spec, l("2026-01-02T00:00:00"), l("2026-01-01T00:00:00")).is_err());
}
#[test]
fn completion_and_override_error_paths_are_typed() {
    let zone = parse_tz("America/New_York").unwrap();
    let input = SeriesInput {
        kind: Kind::Deadline,
        tzid: Some("America/New_York".into()),
        due_at: Some(u("2026-01-01T13:00:00Z")),
        recurrence_mode: Some(RecurrenceMode::Completion),
        completion_interval: Some(PgInterval {
            months: 1,
            days: -29,
            microseconds: 0,
        }),
        ..Default::default()
    };
    let spec = SeriesSpec::new(input.clone()).unwrap();
    assert!(next_after_completion(&spec, u("2026-01-31T13:00:00Z"), zone).is_err());
    assert!(this_and_future(&spec, 1).is_err());
    assert!(this_and_future(&spec, 0).is_err());
    let mut huge = input;
    huge.completion_interval = Some(PgInterval {
        months: i32::MAX,
        ..Default::default()
    });
    assert!(
        next_after_completion(
            &SeriesSpec::new(huge).unwrap(),
            u("2026-01-01T13:00:00Z"),
            zone
        )
        .is_err()
    );
    assert!(
        next_after_completion(&calendar("FREQ=DAILY"), u("2026-01-01T13:00:00Z"), zone).is_err()
    );
    assert!(
        this_occurrence(
            &calendar("FREQ=DAILY;COUNT=3"),
            3,
            &OccurrenceAnchor {
                local_due: Some(l("2026-05-01T08:00:00")),
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(this_occurrence(&spec, 4, &OccurrenceAnchor::default()).is_err());
    assert!(
        this_occurrence(
            &spec,
            4,
            &OccurrenceAnchor {
                local_due: Some(l("2026-05-01T08:00:00")),
                ..Default::default()
            }
        )
        .is_ok()
    );
    assert!(horizon_end(u("9999-12-01T00:00:00Z"), zone).is_err());
}
#[test]
fn date_boundary_and_appointment_end_determine_missedness() {
    let zone = parse_tz("America/New_York").unwrap();
    let rows = vec![
        OpenOccurrence {
            closed_at: None,
            generated_anchor: None,
            disposition: Disposition::Open,
            status: Status::Todo,
            anchor: OccurrenceAnchor {
                due_date: Some(d("2026-03-08")),
                ..Default::default()
            },
        },
        OpenOccurrence {
            closed_at: None,
            generated_anchor: None,
            disposition: Disposition::Open,
            status: Status::Todo,
            anchor: OccurrenceAnchor {
                local_start: Some(l("2026-03-08T10:00:00")),
                local_end: Some(l("2026-03-08T11:00:00")),
                ..Default::default()
            },
        },
        OpenOccurrence {
            closed_at: None,
            generated_anchor: None,
            disposition: Disposition::Open,
            status: Status::Skipped,
            anchor: OccurrenceAnchor {
                due_date: Some(d("2026-03-01")),
                ..Default::default()
            },
        },
    ];
    assert_eq!(
        dispositions(&rows, u("2026-03-08T14:30:00Z"), MissedPolicy::Stack, zone).unwrap(),
        vec![Disposition::Open, Disposition::Open, Disposition::Open]
    );
    assert_eq!(
        dispositions(&rows, u("2026-03-09T04:00:00Z"), MissedPolicy::Stack, zone).unwrap(),
        vec![Disposition::Open, Disposition::Overdue, Disposition::Open]
    );
    assert_eq!(
        dispositions(
            &rows,
            u("2026-03-09T04:00:00.000001Z"),
            MissedPolicy::Stack,
            zone
        )
        .unwrap(),
        vec![
            Disposition::Overdue,
            Disposition::Overdue,
            Disposition::Open
        ]
    );
}
#[test]
fn until_split_repeated_split_and_large_head() {
    for rule in [
        "FREQ=DAILY;UNTIL=20260110",
        "FREQ=DAILY;UNTIL=20260110T080000Z",
    ] {
        let spec = calendar(rule);
        let (head, tail) = this_and_future(&spec, 3).unwrap();
        let (middle, last) = this_and_future(&tail, 3).unwrap();
        let a = l("2026-01-01T00:00:00");
        let c = l("2026-02-01T00:00:00");
        let mut rows = expand(&head.unwrap(), a, c).unwrap();
        let mut mid = expand(&middle.unwrap(), a, c).unwrap();
        for row in &mut mid {
            row.seq += 3;
        }
        rows.extend(mid);
        let mut end = expand(&last, a, c).unwrap();
        for row in &mut end {
            row.seq += 6;
        }
        rows.extend(end);
        assert_eq!(rows, expand(&spec, a, c).unwrap());
    }
    let spec = calendar("FREQ=DAILY");
    let (head, tail) = this_and_future(&spec, 10000).unwrap();
    assert!(head.as_ref().unwrap().rule().unwrap().until().is_some());
    let c = l("2054-01-01T00:00:00");
    let a = l("2026-01-01T00:00:00");
    let mut rows = expand(&head.unwrap(), a, c).unwrap();
    let mut end = expand(&tail, a, c).unwrap();
    for row in &mut end {
        row.seq += 10000;
    }
    rows.extend(end);
    assert!(rows.len() > MAX_MATERIALIZED_OCCURRENCES);
    assert!(expand(&spec, a, c).is_err());
}
#[test]
fn last_supported_second_can_be_overridden() {
    let spec = SeriesSpec::new(SeriesInput {
        kind: Kind::Deadline,
        time_mode: TimeMode::Floating,
        local_due: Some(l("9999-12-31T23:59:59")),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        rrule: Some("FREQ=DAILY;COUNT=1".into()),
        ..Default::default()
    })
    .unwrap();
    assert!(
        this_occurrence(
            &spec,
            0,
            &OccurrenceAnchor {
                local_due: Some(l("9999-12-31T23:59:58")),
                ..Default::default()
            }
        )
        .is_ok()
    );
}
proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64, failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(5545),
        ..ProptestConfig::default()
    })]
    #[test]
    fn weekly_monthly_until_partition_and_split(
        index in 0usize..4, interval in 1u16..4,
        days in 60i64..400, boundary in 1i64..60
    ) {
        let freq = ["WEEKLY;BYDAY=MO,FR", "MONTHLY;BYMONTHDAY=-1,15", "MONTHLY;BYDAY=-1FR,2TU", "YEARLY"][index];
        let spec = calendar(&format!("FREQ={freq};INTERVAL={interval};UNTIL=20271231"));
        let a = l("2026-01-01T00:00:00");
        let b = a + Duration::days(boundary);
        let c = a + Duration::days(days);
        let rows = expand(&spec, a, c).unwrap();
        let mut parts = expand(&spec, a, b).unwrap();
        parts.extend(expand(&spec, b, c).unwrap());
        prop_assert_eq!(&rows, &parts);
        prop_assert!(rows.windows(2).all(|w| w[0].local_due < w[1].local_due));
        prop_assert!(rows.iter().all(|r| r.local_due.unwrap().date() <= d("2027-12-31")));
        if rows.len() > 1 {
            let (head, tail) = this_and_future(&spec, 1).unwrap();
            let restored = SeriesSpec::new_with_calendar_cursor(
                tail.input().clone(), tail.calendar_cursor()
            ).unwrap();
            let mut result = expand(&head.unwrap(), a, c).unwrap();
            let mut tail_rows = expand(&restored, a, c).unwrap();
            for row in &mut tail_rows { row.seq += 1; }
            result.extend(tail_rows);
            prop_assert_eq!(result, rows);
        }
        let rule = spec.rule().unwrap().to_string();
        prop_assert_eq!(calendar(&rule).rule().unwrap().to_string(), rule);
    }
}
