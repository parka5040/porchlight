use chrono::{Datelike, Days, NaiveDate, Weekday};
use porchlight_core::items::{Kind, RecurrenceMode, SeriesInput, SeriesSpec, TimeMode, expand};

#[test]
fn monthly_byday_matches_an_independent_date_walk() {
    let start = NaiveDate::from_ymd_opt(2024, 1, 1)
        .unwrap()
        .and_hms_opt(8, 0, 0)
        .unwrap();
    let end = NaiveDate::from_ymd_opt(2028, 1, 1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap();
    for rule in ["FREQ=MONTHLY;BYDAY=MO,2TU", "FREQ=MONTHLY;BYDAY=-5SA"] {
        let mut expected = vec![start]; // DTSTART belongs to the recurrence set too.
        let mut date = start.date().succ_opt().unwrap();
        while date < end.date() {
            let selected = if rule.ends_with("MO,2TU") {
                date.weekday() == Weekday::Mon
                    || (date.weekday() == Weekday::Tue && (8..=14).contains(&date.day()))
            } else {
                // Fifth Saturday from the end: four later Saturdays in this month,
                // but not five. No recurrence library or engine selector is used.
                date.weekday() == Weekday::Sat
                    && date.checked_add_days(Days::new(28)).unwrap().month() == date.month()
                    && date.checked_add_days(Days::new(35)).unwrap().month() != date.month()
            };
            if selected {
                expected.push(date.and_hms_opt(8, 0, 0).unwrap());
            }
            date = date.succ_opt().unwrap();
        }
        assert!(
            expected.len() > 10,
            "oracle must include non-DTSTART candidates"
        );
        let spec = SeriesSpec::new(SeriesInput {
            kind: Kind::Deadline,
            time_mode: TimeMode::Floating,
            local_due: Some(start),
            recurrence_mode: Some(RecurrenceMode::Calendar),
            rrule: Some(rule.into()),
            ..Default::default()
        })
        .unwrap();
        let actual: Vec<_> = expand(&spec, start, end)
            .unwrap()
            .into_iter()
            .map(|anchor| anchor.local_due.unwrap())
            .collect();
        assert_eq!(actual, expected, "{rule}");
    }
}
