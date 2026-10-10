use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use porchlight_core::{
    items::*,
    time::{parse_tz, resolve_local},
};
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
fn calendar(start: &str, rule: &str) -> SeriesSpec {
    SeriesSpec::new(SeriesInput {
        kind: Kind::Deadline,
        time_mode: TimeMode::Floating,
        local_due: Some(l(start)),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        rrule: Some(rule.into()),
        ..Default::default()
    })
    .unwrap()
}
fn times(spec: &SeriesSpec, until: &str) -> Vec<String> {
    expand(spec, l("2000-01-01T00:00:00"), l(until))
        .unwrap()
        .iter()
        .map(|a| a.local_due.unwrap().format("%Y-%m-%d").to_string())
        .collect()
}
#[test]
fn filter_goldens() {
    let cases = [
        (
            "2024-02-29T08:00:00",
            "FREQ=YEARLY;COUNT=3",
            "2033-01-01T00:00:00",
            vec!["2024-02-29", "2028-02-29", "2032-02-29"],
        ),
        (
            "2026-01-31T08:00:00",
            "FREQ=MONTHLY;BYMONTHDAY=31;COUNT=4",
            "2027-01-01T00:00:00",
            vec!["2026-01-31", "2026-03-31", "2026-05-31", "2026-07-31"],
        ),
        (
            "2026-01-31T08:00:00",
            "FREQ=MONTHLY;BYMONTHDAY=-1;COUNT=3",
            "2027-01-01T00:00:00",
            vec!["2026-01-31", "2026-02-28", "2026-03-31"],
        ),
        (
            "2026-01-30T08:00:00",
            "FREQ=MONTHLY;BYDAY=-1FR;COUNT=3",
            "2027-01-01T00:00:00",
            vec!["2026-01-30", "2026-02-27", "2026-03-27"],
        ),
        (
            "2026-01-13T08:00:00",
            "FREQ=MONTHLY;BYDAY=2TU;COUNT=3",
            "2027-01-01T00:00:00",
            vec!["2026-01-13", "2026-02-10", "2026-03-10"],
        ),
        (
            "2026-01-01T08:00:00",
            "FREQ=DAILY;INTERVAL=2;COUNT=3",
            "2027-01-01T00:00:00",
            vec!["2026-01-01", "2026-01-03", "2026-01-05"],
        ),
        (
            "2026-01-01T08:00:00",
            "FREQ=WEEKLY;BYDAY=MO;COUNT=3",
            "2027-01-01T00:00:00",
            vec!["2026-01-01", "2026-01-05", "2026-01-12"],
        ),
    ];
    for (start, rule, end, expected) in cases {
        assert_eq!(times(&calendar(start, rule), end), expected, "{rule}");
    }
}
#[test]
fn until_inclusive_local_dates_vs_utc() {
    assert_eq!(
        times(
            &calendar("2026-01-01T08:00:00", "FREQ=DAILY;UNTIL=20260103"),
            "2027-01-01T00:00:00"
        )
        .len(),
        3
    );
    assert_eq!(
        times(
            &calendar("2026-01-01T08:00:00", "FREQ=DAILY;UNTIL=20260103T075959Z"),
            "2027-01-01T00:00:00"
        )
        .len(),
        2
    );
    assert_eq!(
        times(
            &calendar("2026-01-01T08:00:00", "FREQ=DAILY;UNTIL=20260103T080000Z"),
            "2027-01-01T00:00:00"
        )
        .len(),
        3
    );
    let mut input = calendar("2026-01-01T08:00:00", "FREQ=DAILY;UNTIL=20260103T130000Z")
        .input()
        .clone();
    input.time_mode = TimeMode::Fixed;
    input.local_due = None;
    input.due_at = Some(u("2026-01-01T13:00:00Z"));
    input.tzid = Some("America/New_York".into());
    let spec = SeriesSpec::new(input).unwrap();
    assert_eq!(times(&spec, "2027-01-01T00:00:00").len(), 3);
}
#[test]
fn dst_zone_goldens() {
    for (zone, start, rule, expected) in [
        (
            "America/New_York",
            "2026-10-31T01:30:00",
            "FREQ=DAILY;COUNT=3",
            vec![
                "2026-10-31T05:30:00Z",
                "2026-11-01T05:30:00Z",
                "2026-11-02T06:30:00Z",
            ],
        ),
        (
            "America/New_York",
            "2026-10-25T01:30:00",
            "FREQ=WEEKLY;COUNT=3",
            vec![
                "2026-10-25T05:30:00Z",
                "2026-11-01T05:30:00Z",
                "2026-11-08T06:30:00Z",
            ],
        ),
        (
            "Europe/London",
            "2026-03-28T01:30:00",
            "FREQ=DAILY;COUNT=3",
            vec![
                "2026-03-28T01:30:00Z",
                "2026-03-29T01:00:00Z",
                "2026-03-30T00:30:00Z",
            ],
        ),
        (
            "Europe/London",
            "2026-10-24T01:30:00",
            "FREQ=DAILY;COUNT=3",
            vec![
                "2026-10-24T00:30:00Z",
                "2026-10-25T00:30:00Z",
                "2026-10-26T01:30:00Z",
            ],
        ),
        (
            "Australia/Lord_Howe",
            "2026-10-03T02:10:00",
            "FREQ=DAILY;COUNT=3",
            vec![
                "2026-10-02T15:40:00Z",
                "2026-10-03T15:30:00Z",
                "2026-10-04T15:10:00Z",
            ],
        ),
        (
            "Australia/Lord_Howe",
            "2026-04-04T01:45:00",
            "FREQ=DAILY;COUNT=3",
            vec![
                "2026-04-03T14:45:00Z",
                "2026-04-04T14:45:00Z",
                "2026-04-05T15:15:00Z",
            ],
        ),
    ] {
        let spec = SeriesSpec::new(SeriesInput {
            kind: Kind::Deadline,
            tzid: Some(zone.into()),
            due_at: Some(resolve_local(l(start), parse_tz(zone).unwrap())),
            recurrence_mode: Some(RecurrenceMode::Calendar),
            rrule: Some(rule.into()),
            ..Default::default()
        })
        .unwrap();
        let result = expand(&spec, l(start), l(start) + Duration::days(30)).unwrap();
        assert_eq!(
            result.iter().map(|a| a.due_at.unwrap()).collect::<Vec<_>>(),
            expected.iter().map(|s| u(s)).collect::<Vec<_>>(),
            "{zone}"
        );
    }
}
#[test]
fn appointments_wall_duration_all_day_and_horizon() {
    let mut input = SeriesInput {
        kind: Kind::Appointment,
        tzid: Some("America/New_York".into()),
        starts_at: Some(u("2026-03-07T19:00:00Z")),
        ends_at: Some(u("2026-03-07T20:00:00Z")),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        rrule: Some("FREQ=DAILY;COUNT=3".into()),
        ..Default::default()
    };
    let spec = SeriesSpec::new(input.clone()).unwrap();
    let rows = expand(&spec, l("2026-03-07T00:00:00"), l("2026-03-10T00:00:00")).unwrap();
    assert_eq!(rows[1].local_start, Some(l("2026-03-08T14:00:00")));
    assert_eq!(rows[1].local_end, Some(l("2026-03-08T15:00:00")));
    assert_eq!(rows[1].starts_at, Some(u("2026-03-08T18:00:00Z")));
    input.all_day = true;
    input.starts_at = Some(u("2026-03-07T05:00:00Z"));
    input.ends_at = Some(u("2026-03-08T05:00:00Z"));
    let spec = SeriesSpec::new(input).unwrap();
    let rows = expand(&spec, l("2026-03-07T00:00:00"), l("2026-03-10T00:00:00")).unwrap();
    assert_eq!(
        rows[1].ends_at.unwrap() - rows[1].starts_at.unwrap(),
        Duration::hours(23)
    );
    assert_eq!(rows[1].local_end, Some(l("2026-03-09T00:00:00")));
    assert_eq!(HORIZON_MONTHS, 18);
    assert_eq!(
        horizon_end(
            u("2026-08-31T18:00:00Z"),
            parse_tz("America/New_York").unwrap()
        )
        .unwrap(),
        u("2028-02-29T19:00:00Z")
    );
}
#[test]
fn nonrecurring_and_undated() {
    let spec = SeriesSpec::new(SeriesInput::default()).unwrap();
    assert_eq!(
        expand(&spec, l("2026-01-01T00:00:00"), l("2027-01-01T00:00:00")).unwrap(),
        vec![OccurrenceAnchor::default()]
    );
    let spec = SeriesSpec::new(SeriesInput {
        due_date: Some(d("2026-05-05")),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        expand(&spec, l("2026-05-05T00:00:00"), l("2026-05-06T00:00:00")).unwrap()[0].seq,
        0
    );
    assert!(
        expand(&spec, l("2026-05-06T00:00:00"), l("2026-05-07T00:00:00"))
            .unwrap()
            .is_empty()
    );
}
#[test]
fn completion_month_end_days_and_elapsed_microseconds() {
    let zone = parse_tz("America/New_York").unwrap();
    let mut input = SeriesInput {
        kind: Kind::Deadline,
        tzid: Some("America/New_York".into()),
        due_at: Some(u("2026-01-01T13:00:00Z")),
        recurrence_mode: Some(RecurrenceMode::Completion),
        completion_interval: Some(PgInterval {
            months: 1,
            days: 0,
            microseconds: 0,
        }),
        ..Default::default()
    };
    let spec = SeriesSpec::new(input.clone()).unwrap();
    let next = next_after_completion(&spec, u("2026-01-31T13:00:00Z"), zone).unwrap();
    assert_eq!(next.local_due, Some(l("2026-02-28T08:00:00")));
    input.completion_interval = Some(PgInterval {
        months: 0,
        days: 1,
        microseconds: 0,
    });
    let spec = SeriesSpec::new(input.clone()).unwrap();
    let next = next_after_completion(&spec, u("2026-03-07T07:30:00Z"), zone).unwrap();
    assert_eq!(next.due_at, Some(u("2026-03-08T07:00:00Z")));
    input.completion_interval = Some(PgInterval {
        months: 0,
        days: 0,
        microseconds: 86_400_000_000,
    });
    let spec = SeriesSpec::new(input).unwrap();
    let next = next_after_completion(&spec, u("2026-03-07T07:30:00Z"), zone).unwrap();
    assert_eq!(next.local_due, Some(l("2026-03-08T03:30:00")));
    let input = SeriesInput {
        time_mode: TimeMode::Floating,
        due_date: Some(d("2026-01-01")),
        recurrence_mode: Some(RecurrenceMode::Completion),
        completion_interval: Some(PgInterval {
            months: 1,
            days: 0,
            microseconds: 0,
        }),
        ..Default::default()
    };
    let spec = SeriesSpec::new(input).unwrap();
    assert_eq!(
        next_after_completion(&spec, u("2026-02-01T02:00:00Z"), zone)
            .unwrap()
            .due_date,
        Some(d("2026-02-28"))
    );
    let spec = SeriesSpec::new(SeriesInput {
        kind: Kind::Appointment,
        time_mode: TimeMode::Floating,
        local_start: Some(l("2026-01-01T14:00:00")),
        local_end: Some(l("2026-01-01T15:00:00")),
        recurrence_mode: Some(RecurrenceMode::Completion),
        completion_interval: Some(PgInterval {
            months: 0,
            days: 1,
            microseconds: 0,
        }),
        ..Default::default()
    })
    .unwrap();
    let next = next_after_completion(&spec, u("2026-03-07T19:00:00Z"), zone).unwrap();
    assert_eq!(next.local_start, Some(l("2026-03-08T14:00:00")));
    assert_eq!(next.local_end, Some(l("2026-03-08T15:00:00")));
    assert_eq!(next.starts_at, None);
}
#[test]
fn dispositions_strict_boundary_closed_and_unsorted() {
    let zone = parse_tz("America/New_York").unwrap();
    let now = u("2026-03-08T07:00:00Z");
    let rows = vec![
        OpenOccurrence {
            closed_at: None,
            generated_anchor: None,
            disposition: Disposition::Open,
            status: Status::Todo,
            anchor: OccurrenceAnchor {
                seq: 3,
                due_at: Some(now - Duration::minutes(1)),
                ..Default::default()
            },
        },
        OpenOccurrence {
            closed_at: None,
            generated_anchor: None,
            disposition: Disposition::Open,
            status: Status::Doing,
            anchor: OccurrenceAnchor {
                seq: 1,
                due_date: Some(d("2026-03-06")),
                ..Default::default()
            },
        },
        OpenOccurrence {
            closed_at: None,
            generated_anchor: None,
            disposition: Disposition::Open,
            status: Status::Todo,
            anchor: OccurrenceAnchor {
                seq: 4,
                local_due: Some(l("2026-03-08T02:30:00")),
                ..Default::default()
            },
        },
        OpenOccurrence {
            closed_at: None,
            generated_anchor: None,
            disposition: Disposition::Open,
            status: Status::Todo,
            anchor: OccurrenceAnchor::default(),
        },
        OpenOccurrence {
            closed_at: None,
            generated_anchor: None,
            disposition: Disposition::Open,
            status: Status::Done,
            anchor: OccurrenceAnchor {
                due_at: Some(now - Duration::days(4)),
                ..Default::default()
            },
        },
    ];
    assert_eq!(
        dispositions(&rows, now, MissedPolicy::CarryOver, zone),
        vec![
            Disposition::Overdue,
            Disposition::Expired,
            Disposition::Open,
            Disposition::Open,
            Disposition::Open
        ]
    );
    assert_eq!(
        dispositions(&rows, now, MissedPolicy::Stack, zone),
        vec![
            Disposition::Overdue,
            Disposition::Overdue,
            Disposition::Open,
            Disposition::Open,
            Disposition::Open
        ]
    );
    assert_eq!(
        dispositions(&rows, now, MissedPolicy::Skip, zone),
        vec![
            Disposition::Expired,
            Disposition::Expired,
            Disposition::Open,
            Disposition::Open,
            Disposition::Open
        ]
    );
}
#[test]
fn edits_split_dtstart_and_override() {
    for (start, rule) in [
        (
            "2026-01-01T08:00:00",
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE;COUNT=12",
        ),
        (
            "2026-01-01T08:00:00",
            "FREQ=MONTHLY;INTERVAL=2;BYDAY=2TU,-1FR;COUNT=12",
        ),
        ("2026-01-31T08:00:00", "FREQ=MONTHLY;BYMONTHDAY=31;COUNT=12"),
        ("2026-01-01T08:00:00", "FREQ=YEARLY;BYDAY=-1FR;COUNT=4"),
    ] {
        let spec = calendar(start, rule);
        let original = expand(&spec, l("2026-01-01T00:00:00"), l("2040-01-01T00:00:00")).unwrap();
        let (head, tail) = this_and_future(&spec, 2).unwrap();
        let head = head.unwrap();
        let mut combined =
            expand(&head, l("2026-01-01T00:00:00"), l("2040-01-01T00:00:00")).unwrap();
        assert!(tail.calendar_cursor().is_none());
        let restored = SeriesSpec::new(tail.input().clone()).unwrap();
        assert_eq!(tail, restored);
        let mut tail_rows = expand(
            &restored,
            l("2026-01-01T00:00:00"),
            l("2040-01-01T00:00:00"),
        )
        .unwrap();
        for row in &mut tail_rows {
            row.seq += 2;
        }
        combined.extend(tail_rows);
        assert_eq!(combined, original, "{rule}");
        assert_eq!(this_and_future(&spec, 0).unwrap(), (None, spec.clone()));
        assert_eq!(all(&spec), spec);
        let replacement = OccurrenceAnchor {
            local_due: Some(l("2026-01-05T09:00:00")),
            ..Default::default()
        };
        let edit = this_occurrence(&spec, 1, &replacement).unwrap();
        assert_eq!(edit.seq, 1);
        assert_eq!(edit.override_local_due, replacement.local_due);
    }
}
#[test]
fn split_at_gap_keeps_original_local_anchor_and_duration() {
    let spec = SeriesSpec::new(SeriesInput {
        kind: Kind::Appointment,
        tzid: Some("America/New_York".into()),
        starts_at: Some(u("2026-03-07T07:30:00Z")),
        ends_at: Some(u("2026-03-07T08:30:00Z")),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        rrule: Some("FREQ=DAILY;COUNT=3".into()),
        ..Default::default()
    })
    .unwrap();
    let a = l("2026-03-07T00:00:00");
    let c = l("2026-03-10T00:00:00");
    let original = expand(&spec, a, c).unwrap();
    let (head, tail) = this_and_future(&spec, 1).unwrap();
    let cursor = tail.calendar_cursor().unwrap();
    let restored = SeriesSpec::new(tail.input().clone())
        .unwrap()
        .with_calendar_cursor(cursor)
        .unwrap();
    let mut combined = expand(&head.unwrap(), a, c).unwrap();
    let mut rest = expand(&restored, a, c).unwrap();
    for row in &mut rest {
        row.seq += 1;
    }
    combined.extend(rest);
    assert_eq!(combined, original);
    assert_eq!(
        expand(&tail, a, c).unwrap()[1].local_end,
        Some(l("2026-03-09T03:30:00"))
    );
}
#[test]
fn pre_dtstart_until_is_validation_error() {
    let input = SeriesInput {
        kind: Kind::Deadline,
        time_mode: TimeMode::Floating,
        local_due: Some(l("2026-05-05T08:00:00")),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        rrule: Some("FREQ=DAILY;UNTIL=20260504".into()),
        ..Default::default()
    };
    assert_eq!(SeriesSpec::new(input).unwrap_err().field, "rrule");
}
// Rebuild through the same input + optional metadata path used by persistence.
fn restore_tail(tail: &SeriesSpec) -> SeriesSpec {
    SeriesSpec::new_with_calendar_cursor(tail.input().clone(), tail.calendar_cursor()).unwrap()
}
proptest! {
    #![proptest_config(ProptestConfig {
        cases: 80,
        failure_persistence: None,
        rng_seed: proptest::test_runner::RngSeed::Fixed(20261005),
        ..ProptestConfig::default()
    })]
    #[test]
    fn partitions_counts_and_splits(
        interval in 1u16..10, count in 2u32..60,
        split in 1u32..60, boundary in 1i64..200
    ) {
        let spec = calendar("2026-01-01T01:30:00", &format!("FREQ=DAILY;INTERVAL={interval};COUNT={count}"));
        let a = l("2026-01-01T00:00:00");
        let b = a + Duration::days(boundary);
        let c = a + Duration::days(600);
        let all_rows = expand(&spec, a, c).unwrap();
        let mut partition = expand(&spec, a, b).unwrap();
        partition.extend(expand(&spec, b, c).unwrap());
        prop_assert_eq!(&all_rows, &partition);
        prop_assert_eq!(all_rows.len(), count as usize);
        prop_assert!(all_rows.windows(2).all(|w| w[0].local_due < w[1].local_due));
        prop_assert!(all_rows.iter().all(|o| o.local_due.unwrap() >= a && o.local_due.unwrap() < c));

        let at = split.min(count - 1);
        let (head, tail) = this_and_future(&spec, at).unwrap();
        let mut recombined = expand(&head.unwrap(), a, c).unwrap();
        let mut tail_rows = expand(&restore_tail(&tail), a, c).unwrap();
        for row in &mut tail_rows { row.seq += at; }
        recombined.extend(tail_rows);
        prop_assert_eq!(all_rows, recombined);
    }
    #[test]
    fn fixed_resolution_obeys_shared_time(days in 1i64..370, hour in 0u32..24, minute in 0u32..60) {
        let zone = parse_tz("America/New_York").unwrap();
        let start = l("2026-01-01T00:00:00")
            + Duration::hours(i64::from(hour)) + Duration::minutes(i64::from(minute));
        let spec = SeriesSpec::new(SeriesInput {
            kind: Kind::Deadline,
            tzid: Some("America/New_York".into()),
            due_at: Some(resolve_local(start, zone)),
            recurrence_mode: Some(RecurrenceMode::Calendar),
            rrule: Some("FREQ=DAILY".into()),
            ..Default::default()
        }).unwrap();
        for row in expand(&spec, start, start + Duration::days(days)).unwrap() {
            prop_assert_eq!(row.due_at, Some(resolve_local(row.local_due.unwrap(), zone)));
        }
    }
}
