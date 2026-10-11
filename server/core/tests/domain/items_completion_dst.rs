use chrono::{DateTime, NaiveDateTime, Utc};
use porchlight_core::{items::*, time::parse_tz};
fn l(s: &str) -> NaiveDateTime {
    s.parse().unwrap()
}
fn u(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}
#[test]
fn day_based_successor_preserves_gap_wall_anchor_in_both_modes() {
    for mode in [TimeMode::Fixed, TimeMode::Floating] {
        let input = SeriesInput {
            kind: Kind::Deadline,
            time_mode: mode,
            tzid: (mode == TimeMode::Fixed).then(|| "America/New_York".into()),
            due_at: (mode == TimeMode::Fixed).then(|| u("2026-03-01T07:30:00Z")),
            local_due: (mode == TimeMode::Floating).then(|| l("2026-03-01T02:30:00")),
            recurrence_mode: Some(RecurrenceMode::Completion),
            completion_interval: Some(PgInterval {
                days: 1,
                ..Default::default()
            }),
            ..Default::default()
        };
        let next = next_after_completion(
            &SeriesSpec::new(input).unwrap(),
            u("2026-03-07T07:30:00Z"),
            parse_tz("America/New_York").unwrap(),
        )
        .unwrap();
        assert_eq!(next.local_due, Some(l("2026-03-08T02:30:00")));
        assert_eq!(
            next.due_at,
            if mode == TimeMode::Fixed {
                Some(u("2026-03-08T07:00:00Z"))
            } else {
                None
            }
        );
    }
}
#[test]
fn floating_completion_uses_completing_member_zone_and_all_day_stays_midnight() {
    let spec = SeriesSpec::new(SeriesInput {
        kind: Kind::Deadline,
        time_mode: TimeMode::Floating,
        local_due: Some(l("2026-01-01T08:00:00")),
        recurrence_mode: Some(RecurrenceMode::Completion),
        completion_interval: Some(PgInterval {
            days: 1,
            ..Default::default()
        }),
        ..Default::default()
    })
    .unwrap();
    let next = next_after_completion(
        &spec,
        u("2026-10-05T18:00:00Z"),
        parse_tz("Asia/Tokyo").unwrap(),
    )
    .unwrap();
    assert_eq!(next.local_due, Some(l("2026-10-07T03:00:00")));
    let spec = SeriesSpec::new(SeriesInput {
        kind: Kind::Appointment,
        all_day: true,
        time_mode: TimeMode::Floating,
        local_start: Some(l("2026-01-01T00:00:00")),
        local_end: Some(l("2026-01-03T00:00:00")),
        recurrence_mode: Some(RecurrenceMode::Completion),
        completion_interval: Some(PgInterval {
            days: 1,
            ..Default::default()
        }),
        ..Default::default()
    })
    .unwrap();
    let next = next_after_completion(
        &spec,
        u("2026-03-08T16:00:00Z"),
        parse_tz("America/New_York").unwrap(),
    )
    .unwrap();
    assert_eq!(next.local_start, Some(l("2026-03-09T00:00:00")));
    assert_eq!(next.local_end, Some(l("2026-03-11T00:00:00")));
}
#[test]
fn elapsed_completion_intervals_keep_known_instants_through_fold() {
    let input = SeriesInput {
        kind: Kind::Deadline,
        tzid: Some("America/New_York".into()),
        due_at: Some(u("2026-10-01T12:00:00Z")),
        recurrence_mode: Some(RecurrenceMode::Completion),
        completion_interval: Some(PgInterval {
            microseconds: 5_400_000_000,
            ..Default::default()
        }),
        ..Default::default()
    };
    let zone = parse_tz("America/New_York").unwrap();
    let spec = SeriesSpec::new(input.clone()).unwrap();
    let next = next_after_completion(&spec, u("2026-11-01T04:30:00Z"), zone).unwrap();
    assert_eq!(next.due_at, Some(u("2026-11-01T06:00:00Z")));
    assert_eq!(next.local_due, Some(l("2026-11-01T01:00:00")));
    let mut input = input;
    input.completion_interval = Some(PgInterval {
        microseconds: 900_000_000,
        ..Default::default()
    });
    let next = next_after_completion(
        &SeriesSpec::new(input.clone()).unwrap(),
        u("2026-11-01T06:15:00Z"),
        zone,
    )
    .unwrap();
    assert_eq!(next.due_at, Some(u("2026-11-01T06:30:00Z")));
    input.kind = Kind::Appointment;
    input.due_at = None;
    input.starts_at = Some(u("2026-10-01T12:00:00Z"));
    input.ends_at = Some(u("2026-10-01T12:15:00Z"));
    let next = next_after_completion(
        &SeriesSpec::new(input).unwrap(),
        u("2026-11-01T06:15:00Z"),
        zone,
    )
    .unwrap();
    assert_eq!(next.starts_at, Some(u("2026-11-01T06:30:00Z")));
    assert_eq!(next.ends_at, Some(u("2026-11-01T06:45:00Z")));
    assert_eq!(next.local_end, Some(l("2026-11-01T01:45:00")));
}
#[test]
fn fixed_appointment_collapsed_by_gap_preserves_wall_duration() {
    let spec = SeriesSpec::new(SeriesInput {
        kind: Kind::Appointment,
        tzid: Some("America/New_York".into()),
        starts_at: Some(u("2026-03-07T07:10:00Z")),
        ends_at: Some(u("2026-03-07T07:20:00Z")),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        rrule: Some("FREQ=DAILY;COUNT=2".into()),
        ..Default::default()
    })
    .unwrap();
    let rows = expand(&spec, l("2026-03-07T00:00:00"), l("2026-03-09T00:00:00")).unwrap();
    assert_eq!(rows[1].starts_at, Some(u("2026-03-08T07:00:00Z")));
    assert_eq!(rows[1].ends_at, Some(u("2026-03-08T07:10:00Z")));
    assert_eq!(rows[1].local_start, Some(l("2026-03-08T02:10:00")));
    assert_eq!(rows[1].local_end, Some(l("2026-03-08T02:20:00")));
}
