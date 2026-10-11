use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc, Weekday};
use porchlight_core::schedule::*;
use proptest::prelude::*;
use std::collections::BTreeMap;
use uuid::Uuid;
fn utc(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}
fn day(s: &str) -> NaiveDate {
    s.parse().unwrap()
}
fn window(weekday: Weekday, start: &str, end: &str) -> AvailabilityWindow {
    AvailabilityWindow {
        weekday,
        start: NaiveTime::parse_from_str(start, "%H:%M").unwrap(),
        end: NaiveTime::parse_from_str(end, "%H:%M").unwrap(),
    }
}
fn range(a: &str, b: &str) -> TimeRange {
    TimeRange {
        start: utc(a),
        end: utc(b),
    }
}
fn member(id: u128) -> MemberSchedule {
    MemberSchedule {
        member_id: Uuid::from_u128(id),
        tz: chrono_tz::America::New_York,
        windows: vec![
            window(Weekday::Sun, "01:00", "04:00"),
            window(Weekday::Mon, "09:00", "12:00"),
        ],
        busy_ranges: vec![],
    }
}
fn task(id: u128, minutes: u32, deadline: Option<Deadline>, priority: i16) -> Task {
    Task {
        id: Uuid::from_u128(id),
        member_id: Uuid::from_u128(1),
        est_minutes: minutes,
        deadline,
        priority,
    }
}
#[test]
fn dst_windows_busy_subtraction_union_and_clipping() {
    let windows = vec![
        window(Weekday::Sun, "01:00", "04:00"),
        window(Weekday::Sun, "03:00", "05:00"),
        window(Weekday::Mon, "09:00", "12:00"),
    ];
    let busy = vec![
        range("2026-03-08T06:30:00Z", "2026-03-08T07:30:00Z"),
        range("2026-03-08T07:00:00Z", "2026-03-08T08:00:00Z"),
    ];
    assert_eq!(
        free_slots(
            &windows,
            &busy,
            utc("2026-03-08T06:00:00Z"),
            utc("2026-03-10T00:00:00Z"),
            chrono_tz::America::New_York
        ),
        vec![
            range("2026-03-08T06:00:00Z", "2026-03-08T06:30:00Z"),
            range("2026-03-08T08:00:00Z", "2026-03-08T09:00:00Z"),
            range("2026-03-09T13:00:00Z", "2026-03-09T16:00:00Z")
        ]
    );
    let fold = free_slots(
        &[window(Weekday::Sun, "01:00", "04:00")],
        &[],
        utc("2026-11-01T00:00:00Z"),
        utc("2026-11-02T00:00:00Z"),
        chrono_tz::America::New_York,
    );
    assert_eq!(
        fold,
        vec![range("2026-11-01T05:00:00Z", "2026-11-01T09:00:00Z")]
    );
}
#[test]
fn overnight_windows_are_anchored_to_start_weekday_and_equal_is_empty() {
    let windows = vec![
        window(Weekday::Sat, "23:00", "02:00"),
        window(Weekday::Sun, "09:00", "09:00"),
    ];
    assert_eq!(
        free_slots(
            &windows,
            &[],
            utc("2026-03-08T05:30:00Z"),
            utc("2026-03-08T08:00:00Z"),
            chrono_tz::America::New_York
        ),
        vec![range("2026-03-08T05:30:00Z", "2026-03-08T07:00:00Z")]
    );
}
#[test]
fn golden_edf_pressure_ties_and_unplaced_reasons() {
    let tasks = vec![
        task(7, 1, None, 0),
        task(4, 60, Some(Deadline::Date(day("2026-03-09"))), 1),
        task(
            3,
            30,
            Some(Deadline::Instant(utc("2026-03-08T07:00:00Z"))),
            2,
        ),
        task(
            2,
            30,
            Some(Deadline::Instant(utc("2026-03-08T07:00:00Z"))),
            2,
        ),
        task(
            1,
            30,
            Some(Deadline::Instant(utc("2026-03-08T07:00:00Z"))),
            1,
        ),
        task(5, 500, Some(Deadline::Date(day("2026-03-09"))), 0),
        task(6, 0, None, 0),
    ];
    let result = suggest_edf(
        &tasks,
        &[member(1)],
        utc("2026-03-08T00:00:00Z"),
        utc("2026-03-10T00:00:00Z"),
    );
    assert_eq!(
        result
            .suggestions
            .iter()
            .map(|s| s.task_id.as_u128())
            .collect::<Vec<_>>(),
        vec![2, 3, 4, 7]
    );
    assert_eq!(
        result.suggestions[0].range,
        range("2026-03-08T06:00:00Z", "2026-03-08T06:30:00Z")
    );
    assert_eq!(
        result.suggestions[2].range,
        range("2026-03-08T07:00:00Z", "2026-03-08T08:00:00Z")
    );
    assert_eq!(
        result
            .unplaced
            .iter()
            .map(|u| (u.task_id.as_u128(), u.reason))
            .collect::<Vec<_>>(),
        vec![
            (1, UnplacedReason::NoSlotBeforeDeadline),
            (5, UnplacedReason::NoSlotBeforeDeadline),
            (6, UnplacedReason::InvalidEstimate)
        ]
    );
    let mut reversed = tasks;
    reversed.reverse();
    assert_eq!(
        result,
        suggest_edf(
            &reversed,
            &[member(1)],
            utc("2026-03-08T00:00:00Z"),
            utc("2026-03-10T00:00:00Z")
        )
    );
}
#[test]
fn local_date_deadline_is_end_of_day_and_missing_member_reported() {
    let mut m = member(1);
    m.windows = vec![window(Weekday::Sun, "20:00", "23:59")];
    let tasks = vec![
        task(1, 60, Some(Deadline::Date(day("2026-03-08"))), 0),
        Task {
            member_id: Uuid::from_u128(99),
            ..task(2, 60, None, 0)
        },
    ];
    let result = suggest_edf(
        &tasks,
        &[m],
        utc("2026-03-08T00:00:00Z"),
        utc("2026-03-10T00:00:00Z"),
    );
    assert_eq!(
        result.suggestions[0].range,
        range("2026-03-09T00:00:00Z", "2026-03-09T01:00:00Z")
    );
    assert_eq!(result.unplaced[0].reason, UnplacedReason::UnknownMember);
}
#[test]
fn daily_load_counts_elapsed_minutes_merges_windows_and_includes_closed_dates() {
    let tasks = BTreeMap::from([
        (day("2026-03-08"), 150),
        (day("2026-03-09"), 30),
        (day("2026-03-10"), 20),
    ]);
    let result = daily_load(&tasks, &member(1).windows, chrono_tz::America::New_York);
    assert_eq!(
        result,
        vec![
            DailyLoad {
                date: day("2026-03-08"),
                planned_minutes: 150,
                available_minutes: 120,
                overload_minutes: 30
            },
            DailyLoad {
                date: day("2026-03-09"),
                planned_minutes: 30,
                available_minutes: 180,
                overload_minutes: 0
            },
            DailyLoad {
                date: day("2026-03-10"),
                planned_minutes: 20,
                available_minutes: 0,
                overload_minutes: 20
            }
        ]
    );
}
#[test]
fn dependent_ranks_free_inside_then_outside_then_conflicting_with_id_ties() {
    let appt = range("2026-03-09T14:00:00Z", "2026-03-09T15:00:00Z");
    let mut inside = member(3);
    inside.busy_ranges = vec![range("2026-03-09T15:00:00Z", "2026-03-09T16:00:00Z")];
    let mut outside = member(2);
    outside.windows.clear();
    let mut conflict = member(1);
    conflict.busy_ranges = vec![range("2026-03-09T14:30:00Z", "2026-03-09T15:30:00Z")];
    let candidates = dependent_candidates(appt, &[conflict, outside, inside, member(4)]);
    assert_eq!(
        candidates
            .iter()
            .map(|c| (c.member_id.as_u128(), c.reason))
            .collect::<Vec<_>>(),
        vec![
            (3, CandidateReason::FreeInsideAvailability),
            (4, CandidateReason::FreeInsideAvailability),
            (2, CandidateReason::FreeOutsideAvailability),
            (1, CandidateReason::Conflicting)
        ]
    );
}
#[test]
fn golden_week_across_dst_with_two_member_zones() {
    let weekdays = [
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
    ];
    let mut ny = member(1);
    let ny_clocks = [
        "09:00", "10:00", "01:30", "16:00", "08:00", "12:00", "18:00",
    ];
    let ny_ends = [
        "10:00", "11:00", "03:30", "17:00", "09:00", "13:00", "19:00",
    ];
    ny.windows = weekdays
        .iter()
        .enumerate()
        .map(|(i, d)| window(*d, ny_clocks[i], ny_ends[i]))
        .collect();
    ny.busy_ranges = vec![range("2026-03-07T15:00:00Z", "2026-03-07T16:00:00Z")];
    let mut berlin = member(2);
    berlin.tz = chrono_tz::Europe::Berlin;
    let berlin_clocks = [
        "11:00", "09:00", "15:00", "08:00", "17:00", "10:00", "14:00",
    ];
    let berlin_ends = [
        "12:00", "10:00", "16:00", "09:00", "18:00", "11:00", "15:00",
    ];
    berlin.windows = weekdays
        .iter()
        .enumerate()
        .map(|(i, d)| window(*d, berlin_clocks[i], berlin_ends[i]))
        .collect();
    let dates = (0..7)
        .map(|n| day("2026-03-06") + Duration::days(n))
        .collect::<Vec<_>>();
    let mut tasks = Vec::new();
    for (i, date) in dates.iter().enumerate() {
        tasks.push(task(i as u128 + 1, 60, Some(Deadline::Date(*date)), 0));
        tasks.push(Task {
            member_id: Uuid::from_u128(2),
            ..task(i as u128 + 101, 60, Some(Deadline::Date(*date)), 0)
        });
    }
    let result = suggest_edf(
        &tasks,
        &[ny, berlin],
        utc("2026-03-06T00:00:00Z"),
        utc("2026-03-13T00:00:00Z"),
    );
    assert_eq!(
        result.unplaced,
        vec![UnplacedTask {
            task_id: Uuid::from_u128(2),
            reason: UnplacedReason::NoSlotBeforeDeadline
        }]
    );
    assert_eq!(result.suggestions.len(), 13);
    for s in &result.suggestions {
        let n = if s.member_id == Uuid::from_u128(1) {
            s.task_id.as_u128() - 1
        } else {
            s.task_id.as_u128() - 101
        };
        let expected_utc_hours = if s.member_id == Uuid::from_u128(2) {
            [10, 8, 14, 7, 16, 9, 13]
        } else {
            [14, 15, 6, 20, 12, 16, 22]
        };
        let minute = if s.member_id == Uuid::from_u128(1) && n == 2 {
            30
        } else {
            0
        };
        assert_eq!(
            s.range.start,
            dates[n as usize]
                .and_hms_opt(expected_utc_hours[n as usize], minute, 0)
                .unwrap()
                .and_utc()
        );
        assert_eq!(s.range.end, s.range.start + Duration::hours(1));
    }
}
#[test]
fn duplicate_tasks_empty_intervals_and_invalid_candidates_have_explicit_rules() {
    let t = task(1, 30, None, 0);
    let result = suggest_edf(
        &[t.clone(), t],
        &[member(1)],
        utc("2026-03-09T13:00:00Z"),
        utc("2026-03-09T16:00:00Z"),
    );
    assert!(result.suggestions.is_empty());
    assert!(
        result
            .unplaced
            .iter()
            .all(|u| u.reason == UnplacedReason::DuplicateTask)
    );
    assert!(
        free_slots(
            &member(1).windows,
            &[],
            utc("2026-03-09T16:00:00Z"),
            utc("2026-03-09T13:00:00Z"),
            member(1).tz
        )
        .is_empty()
    );
    let candidates = dependent_candidates(
        range("2026-03-09T14:00:00Z", "2026-03-09T14:00:00Z"),
        &[member(1)],
    );
    assert_eq!(candidates[0].reason, CandidateReason::InvalidAppointment);
}
proptest! {
    #[test]
    fn multi_member_placements_preserve_member_slots(estimates in prop::collection::vec(1u32..120,0..60)) {
        let from=utc("2026-03-06T00:00:00Z"); let until=utc("2026-03-13T00:00:00Z");
        let mut b=member(2); b.tz=chrono_tz::Europe::Berlin;
        let members=[member(1),b];
        let tasks=estimates.iter().enumerate().map(|(i,n)|Task { member_id:Uuid::from_u128((i%2+1) as u128),..task(i as u128+1,*n,Some(Deadline::Instant(until)),(i%4) as i16) }).collect::<Vec<_>>();
        let result=suggest_edf(&tasks,&members,from,until);
        for s in &result.suggestions {
            let m=members.iter().find(|m|m.member_id==s.member_id).unwrap();
            prop_assert!(free_slots(&m.windows,&m.busy_ranges,from,until,m.tz).iter().any(|f|f.start<=s.range.start&&s.range.end<=f.end));
            prop_assert!(s.range.end<=until);
        }
        for (i,a) in result.suggestions.iter().enumerate() { for b in result.suggestions.iter().skip(i+1).filter(|b|b.member_id==a.member_id) { prop_assert!(!a.range.overlaps(b.range)); } }
    }
    #[test]
    fn placements_are_disjoint_inside_free_slots_and_before_deadline(estimates in prop::collection::vec(1u32..240,0..60),busy_start in 0i64..150,busy_length in 0i64..60) {
        let from=utc("2026-03-09T13:00:00Z"); let until=from+Duration::hours(3);
        let mut m=member(1); m.busy_ranges=vec![TimeRange{start:from+Duration::minutes(busy_start),end:from+Duration::minutes(busy_start+busy_length)}];
        let slots=free_slots(&m.windows,&m.busy_ranges,from,until,m.tz);
        let tasks=estimates.iter().enumerate().map(|(i,n)|task(i as u128+1,*n,Some(Deadline::Instant(until-Duration::minutes((i%3) as i64))), (i%4) as i16)).collect::<Vec<_>>();
        let result=suggest_edf(&tasks,&[m],from,until);
        for s in &result.suggestions {
            prop_assert!(slots.iter().any(|f|f.start<=s.range.start&&s.range.end<=f.end));
            let t=tasks.iter().find(|t|t.id==s.task_id).unwrap();
            if let Some(Deadline::Instant(d))=t.deadline { prop_assert!(s.range.end<=d); }
            prop_assert_eq!((s.range.end-s.range.start).num_minutes(),i64::from(t.est_minutes));
        }
        for (i,a) in result.suggestions.iter().enumerate() { for b in result.suggestions.iter().skip(i+1) { prop_assert!(a.range.end<=b.range.start || b.range.end<=a.range.start); } }
    }
}

#[test]
fn daily_load_clips_previous_and_current_overnight_windows() {
    let windows = [
        window(Weekday::Sat, "23:00", "03:00"),
        window(Weekday::Sun, "22:00", "02:00"),
    ];
    let tasks = BTreeMap::from([
        (day("2026-03-07"), 0),
        (day("2026-03-08"), 241),
        (day("2026-03-09"), 0),
    ]);
    let loads = daily_load(&tasks, &windows, chrono_tz::America::New_York);
    assert_eq!(
        loads
            .iter()
            .map(|l| l.available_minutes)
            .collect::<Vec<_>>(),
        vec![60, 240, 120]
    );
    assert_eq!(loads[1].overload_minutes, 1);
}
