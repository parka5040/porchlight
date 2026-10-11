use chrono::{DateTime, Duration, NaiveTime, Utc, Weekday};
use porchlight_core::schedule::{
    AvailabilityWindow, Deadline, MemberSchedule, Task, proposal_start, suggest_edf,
};
use uuid::Uuid;
fn utc(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}
#[test]
fn strictly_next_boundary_in_zone_including_seconds_and_dst() {
    for (now, zone, expected) in [
        (
            "2026-10-05T13:00:00Z",
            "America/New_York",
            "2026-10-05T13:15:00Z",
        ),
        (
            "2026-10-05T13:14:59.999999Z",
            "America/New_York",
            "2026-10-05T13:15:00Z",
        ),
        (
            "2026-10-05T13:15:00Z",
            "America/New_York",
            "2026-10-05T13:30:00Z",
        ),
        (
            "2026-10-05T13:16:07Z",
            "Asia/Kathmandu",
            "2026-10-05T13:30:00Z",
        ),
        (
            "2026-03-08T06:59:00Z",
            "America/New_York",
            "2026-03-08T07:00:00Z",
        ),
        (
            "2026-11-01T05:59:00Z",
            "America/New_York",
            "2026-11-01T07:00:00Z",
        ),
        // Shared fold policy chooses the earlier offset; skip past resolutions.
        (
            "2026-11-01T06:01:00Z",
            "America/New_York",
            "2026-11-01T07:00:00Z",
        ),
    ] {
        assert_eq!(
            proposal_start(utc(now), zone.parse().unwrap()),
            utc(expected)
        );
    }
}
#[test]
fn edf_search_begins_after_now_even_at_window_start() {
    let now = utc("2026-10-05T13:00:00Z");
    let member = MemberSchedule {
        member_id: Uuid::from_u128(1),
        tz: "America/New_York".parse().unwrap(),
        windows: vec![AvailabilityWindow {
            weekday: Weekday::Mon,
            start: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            end: NaiveTime::from_hms_opt(12, 0, 0).unwrap(),
        }],
        busy_ranges: vec![],
    };
    let task = Task {
        id: Uuid::from_u128(2),
        member_id: member.member_id,
        est_minutes: 30,
        deadline: Some(Deadline::Instant(now + Duration::hours(3))),
        priority: 0,
    };
    let result = suggest_edf(&[task], &[member], now, now + Duration::days(1));
    assert_eq!(result.suggestions.len(), 1);
    assert_eq!(
        result.suggestions[0].range.start,
        utc("2026-10-05T13:15:00Z")
    );
}
