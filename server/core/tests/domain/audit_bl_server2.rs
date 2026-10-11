use chrono::{DateTime, NaiveDateTime, Utc};
use porchlight_core::items::*;

fn local(s: &str) -> NaiveDateTime {
    s.parse().unwrap()
}
fn utc(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

#[test]
fn disposition_rejects_invalid_effective_and_generated_spans() {
    let bad = OccurrenceAnchor {
        local_start: Some(local("2026-03-08T03:30:00")),
        local_end: Some(local("2026-03-08T01:30:00")),
        due_at: Some(utc("2026-03-01T00:00:00Z")),
        ..Default::default()
    };
    for generated in [false, true] {
        let row = OpenOccurrence {
            anchor: if generated {
                OccurrenceAnchor::default()
            } else {
                bad.clone()
            },
            generated_anchor: generated.then(|| bad.clone()),
            status: Status::Todo,
            disposition: Disposition::Open,
            closed_at: None,
        };
        let result = dispositions(
            &[row],
            utc("2026-03-10T00:00:00Z"),
            MissedPolicy::CarryOver,
            chrono_tz::America::New_York,
        );
        assert!(
            format!("{result:?}").starts_with("Err("),
            "invalid span silently accepted: {result:?}"
        );
    }
}

#[test]
fn fixed_all_day_skipped_midnight_keeps_nominal_dates() {
    let input = SeriesInput {
        kind: Kind::Appointment,
        all_day: true,
        tzid: Some("America/Sao_Paulo".into()),
        starts_at: Some(utc("2018-11-04T03:00:00Z")),
        ends_at: Some(utc("2018-11-05T02:00:00Z")),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        rrule: Some("FREQ=DAILY;COUNT=2".into()),
        ..Default::default()
    };
    let spec = SeriesSpec::new(input.clone()).expect("resolved midnight is a valid all-day anchor");
    let rows = expand(
        &spec,
        local("2018-11-04T00:00:00"),
        local("2018-11-06T00:00:00"),
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].local_start, Some(local("2018-11-04T00:00:00")));
    assert_eq!(rows[0].local_end, Some(local("2018-11-05T00:00:00")));
    assert_eq!(rows[0].starts_at, input.starts_at);
    assert_eq!(rows[1].starts_at, input.ends_at);
    assert_eq!(rows[1].local_end, Some(local("2018-11-06T00:00:00")));
    let mut invalid = input;
    invalid.starts_at = Some(utc("2018-11-04T03:01:00Z"));
    assert!(SeriesSpec::new(invalid).is_err());
}
