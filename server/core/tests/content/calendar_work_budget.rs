use chrono::NaiveDate;
use porchlight_core::items::{
    Kind, RecurrenceMode, SeriesInput, SeriesSpec, bounded_occurrence_count, expand,
};

fn series(start: &str, rule: &str) -> SeriesSpec {
    let start: chrono::DateTime<chrono::Utc> = start.parse().unwrap();
    SeriesSpec::new(SeriesInput {
        kind: Kind::Appointment,
        starts_at: Some(start),
        ends_at: Some(start + chrono::Duration::hours(1)),
        tzid: Some("UTC".into()),
        rrule: Some(rule.into()),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn bounded_count_matches_materialization_and_stops_at_remaining_budget() {
    let from = NaiveDate::from_ymd_opt(1, 1, 1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap();
    let until = NaiveDate::from_ymd_opt(2028, 4, 6)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap();
    for rule in [
        "FREQ=DAILY",
        "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE",
        "FREQ=MONTHLY;BYMONTHDAY=31",
        "FREQ=YEARLY;COUNT=3",
    ] {
        let spec = series("2026-10-12T09:00:00Z", rule);
        let count = expand(&spec, from, until).unwrap().len();
        assert_eq!(
            bounded_occurrence_count(&spec, until, 10000).unwrap(),
            count
        );
        for limit in [0, 1, 10, 100] {
            assert_eq!(
                bounded_occurrence_count(&spec, until, limit).unwrap(),
                count.min(limit + 1)
            );
        }
    }
}

#[test]
fn count_includes_ancient_dtstart_history_not_only_the_parse_window() {
    let spec = series("2000-01-01T09:00:00Z", "FREQ=DAILY");
    let until = NaiveDate::from_ymd_opt(2028, 4, 6)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap();
    assert_eq!(bounded_occurrence_count(&spec, until, 5000).unwrap(), 5001);
}
