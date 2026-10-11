use chrono::{DateTime, Utc};
use porchlight_core::{items::*, time::parse_tz};
#[test]
fn extreme_instants_return_errors_before_zone_conversion_can_overflow() {
    let zone = parse_tz("Pacific/Kiritimati").unwrap();
    assert!(
        SeriesSpec::new(SeriesInput {
            kind: Kind::Deadline,
            tzid: Some("Pacific/Kiritimati".into()),
            due_at: Some(DateTime::<Utc>::MAX_UTC),
            ..Default::default()
        })
        .is_err()
    );
    assert!(horizon_end(DateTime::<Utc>::MAX_UTC, zone).is_err());
    let spec = SeriesSpec::new(SeriesInput {
        kind: Kind::Deadline,
        tzid: Some("Pacific/Kiritimati".into()),
        due_at: Some("2026-01-01T00:00:00Z".parse().unwrap()),
        recurrence_mode: Some(RecurrenceMode::Completion),
        completion_interval: Some(PgInterval {
            days: 1,
            ..Default::default()
        }),
        ..Default::default()
    })
    .unwrap();
    assert!(next_after_completion(&spec, DateTime::<Utc>::MAX_UTC, zone).is_err());
}
