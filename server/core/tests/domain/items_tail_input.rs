use chrono::{DateTime, NaiveDateTime, Utc};
use porchlight_core::{
    items::*,
    time::{parse_tz, resolve_local},
};

fn l(s: &str) -> NaiveDateTime {
    s.parse().unwrap()
}
#[test]
fn input_plus_optional_anchor_validates_nominal_all_day_and_until_before_round_trip() {
    for (zone_name, start, until, split) in [
        ("Pacific/Apia", "2011-12-28T00:00:00", "20111230", 2),
        ("America/Santiago", "2026-09-04T00:00:00", "20260907", 2),
    ] {
        let zone = parse_tz(zone_name).unwrap();
        let start = l(start);
        let end = start + chrono::Duration::days(1);
        let spec = SeriesSpec::new(SeriesInput {
            kind: Kind::Appointment,
            all_day: true,
            tzid: Some(zone_name.into()),
            starts_at: Some(resolve_local(start, zone)),
            ends_at: Some(resolve_local(end, zone)),
            recurrence_mode: Some(RecurrenceMode::Calendar),
            rrule: Some(format!("FREQ=DAILY;UNTIL={until}")),
            ..Default::default()
        })
        .unwrap();
        let c = start + chrono::Duration::days(6);
        let original = expand(&spec, start, c).unwrap();
        let (_, tail) = this_and_future(&spec, split).unwrap();
        let restored =
            SeriesSpec::new_with_calendar_cursor(tail.input().clone(), tail.calendar_cursor())
                .unwrap();
        let mut rest = expand(&restored, start, c).unwrap();
        for row in &mut rest {
            row.seq += split;
        }
        assert_eq!(rest, original[split as usize..]);
        assert!(tail.calendar_cursor().is_some());
        let _known_start: DateTime<Utc> = tail.input().starts_at.unwrap();
    }
}
