use chrono::{Duration, NaiveDate, NaiveTime, Weekday};
use porchlight_core::{schedule::*, time};
use uuid::Uuid;

#[test]
fn edf_skips_unrepresentable_fold_endpoints() {
    for (tz, date, minutes) in [
        (chrono_tz::America::New_York, "2026-11-01", 120),
        (chrono_tz::Australia::Sydney, "2026-04-05", 180),
    ] {
        let date: NaiveDate = date.parse().unwrap();
        let member = MemberSchedule {
            member_id: Uuid::from_u128(1),
            tz,
            windows: vec![AvailabilityWindow {
                weekday: Weekday::Sun,
                start: NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
                end: NaiveTime::from_hms_opt(5, 0, 0).unwrap(),
            }],
            busy_ranges: vec![],
        };
        let task = Task {
            id: Uuid::from_u128(2),
            member_id: member.member_id,
            est_minutes: minutes,
            deadline: Some(Deadline::Date(date)),
            priority: 0,
        };
        let now = time::start_of_day(date, tz);
        let result = suggest_edf(&[task], &[member], now, now + Duration::days(1));
        assert_eq!(result.suggestions.len(), 1, "a later valid span exists");
        let range = result.suggestions[0].range;
        assert_eq!(
            range.end - range.start,
            Duration::minutes(i64::from(minutes))
        );
        let start = time::to_local(range.start, tz);
        let end = time::to_local(range.end, tz);
        assert!(start < end);
        assert_eq!(
            time::resolve_local(start, tz),
            range.start,
            "start round trip"
        );
        assert_eq!(time::resolve_local(end, tz), range.end, "end round trip");
    }
}
