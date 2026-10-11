use chrono::{Duration, NaiveDate, NaiveTime, Weekday};
use porchlight_core::schedule::*;
use uuid::Uuid;

fn second_precision_fold(busy: bool) {
    let date: NaiveDate = "2026-11-01".parse().unwrap();
    let now = "2026-11-01T04:00:00Z".parse().unwrap();
    let member = MemberSchedule {
        member_id: Uuid::from_u128(1),
        tz: chrono_tz::America::New_York,
        windows: vec![AvailabilityWindow {
            weekday: Weekday::Sun,
            start: NaiveTime::from_hms_opt(0, if busy { 0 } else { 15 }, if busy { 0 } else { 30 })
                .unwrap(),
            end: NaiveTime::from_hms_opt(2, 0, 0).unwrap(),
        }],
        busy_ranges: if busy {
            vec![TimeRange {
                start: now,
                end: "2026-11-01T04:15:30Z".parse().unwrap(),
            }]
        } else {
            vec![]
        },
    };
    let task = Task {
        id: Uuid::from_u128(2),
        member_id: member.member_id,
        est_minutes: 120,
        deadline: Some(Deadline::Date(date)),
        priority: 0,
    };
    let result = suggest_edf(&[task], &[member], now, now + Duration::days(1));
    assert_eq!(
        result.suggestions.len(),
        1,
        "second precision must not skip the tight fold slot: busy={busy}"
    );
    let range = result.suggestions[0].range;
    assert_eq!(
        range.start,
        "2026-11-01T05:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap()
    );
    assert_eq!(
        range.end,
        "2026-11-01T07:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap()
    );
    assert!(representable_override(range, chrono_tz::America::New_York));
}

#[test]
fn second_aligned_window_finds_exact_fold_boundary() {
    second_precision_fold(false);
}

#[test]
fn second_aligned_busy_end_finds_exact_fold_boundary() {
    second_precision_fold(true);
}
