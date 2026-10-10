use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use porchlight_core::items::*;
use std::str::FromStr;

fn local(s: &str) -> NaiveDateTime {
    s.parse().unwrap()
}
fn utc(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}
fn date(s: &str) -> NaiveDate {
    s.parse().unwrap()
}
fn deadline(mode: TimeMode) -> SeriesInput {
    SeriesInput {
        kind: Kind::Deadline,
        time_mode: mode,
        tzid: (mode == TimeMode::Fixed).then(|| "America/New_York".into()),
        due_at: (mode == TimeMode::Fixed).then(|| utc("2026-03-07T07:30:00Z")),
        local_due: (mode == TimeMode::Floating).then(|| local("2026-03-07T02:30:00")),
        ..Default::default()
    }
}

#[test]
fn enum_contract() {
    macro_rules! check { ($t:ty, $($s:expr),+) => { $(let v = <$t>::from_str($s).unwrap(); assert_eq!(v.as_str(), $s); assert_eq!(serde_json::to_string(&v).unwrap(), format!("\"{}\"", $s)); assert_eq!(serde_json::from_str::<$t>(&format!("\"{}\"", $s)).unwrap(), v);)+ assert!(<$t>::from_str("WRONG").is_err()); }; }
    check!(Kind, "todo", "deadline", "appointment");
    check!(TimeMode, "fixed", "floating");
    check!(RecurrenceMode, "calendar", "completion");
    check!(MissedPolicy, "carry_over", "skip", "stack");
    check!(Status, "backlog", "todo", "doing", "done", "skipped");
    check!(Disposition, "open", "overdue", "expired");
}

#[test]
fn shape_validation_and_pg_interval() {
    assert!(SeriesSpec::new(SeriesInput::default()).is_ok());
    for mode in [TimeMode::Fixed, TimeMode::Floating] {
        let input = deadline(mode);
        assert!(SeriesSpec::new(input.clone()).is_ok());
        let mut bad = input.clone();
        bad.due_date = Some(date("2026-03-07"));
        assert_eq!(SeriesSpec::new(bad).unwrap_err().field, "due_date");
        let mut bad = input.clone();
        bad.time_mode = if mode == TimeMode::Fixed {
            TimeMode::Floating
        } else {
            TimeMode::Fixed
        };
        assert!(SeriesSpec::new(bad).is_err());
        let mut bad = input.clone();
        bad.recurrence_mode = Some(RecurrenceMode::Calendar);
        assert!(SeriesSpec::new(bad).is_err());
        let mut bad = input.clone();
        bad.completion_interval = Some(PgInterval {
            months: 0,
            days: 1,
            microseconds: 0,
        });
        assert!(SeriesSpec::new(bad).is_err());
    }
    let mut input = deadline(TimeMode::Fixed);
    input.recurrence_mode = Some(RecurrenceMode::Completion);
    input.completion_interval = Some(PgInterval {
        months: 1,
        days: -29,
        microseconds: 0,
    });
    assert!(SeriesSpec::new(input.clone()).is_ok()); // PG compares months as 30 days.
    input.completion_interval = Some(PgInterval {
        months: 1,
        days: -30,
        microseconds: 0,
    });
    assert!(SeriesSpec::new(input).is_err());
}

#[test]
fn rrule_corpus_and_canonicalization() {
    for line in include_str!("fixtures/rrule_corpus.txt")
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let (verdict, rule) = line.split_once(' ').unwrap();
        assert_eq!(Rule::from_str(rule).is_ok(), verdict == "accept", "{line}");
    }
    let rule: Rule = "FREQ=MONTHLY;COUNT=5;BYDAY=+2TU,-1FR;INTERVAL=1"
        .parse()
        .unwrap();
    assert_eq!(rule.to_string(), "FREQ=MONTHLY;BYDAY=-1FR,2TU;COUNT=5");
    assert_eq!(rule.to_string().parse::<Rule>().unwrap(), rule);
}

#[test]
fn calendar_gap_fold_and_partition() {
    let from = local("2026-03-07T00:00:00");
    let mid = local("2026-03-08T00:00:00");
    let until = local("2026-03-10T00:00:00");
    for mode in [TimeMode::Fixed, TimeMode::Floating] {
        let mut input = deadline(mode);
        input.recurrence_mode = Some(RecurrenceMode::Calendar);
        input.rrule = Some("FREQ=DAILY;COUNT=3".into());
        let spec = SeriesSpec::new(input).unwrap();
        let all = expand(&spec, from, until).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all.iter().map(|a| a.seq).collect::<Vec<_>>(), vec![0, 1, 2]);
        assert_eq!(all[1].local_due, Some(local("2026-03-08T02:30:00")));
        if mode == TimeMode::Fixed {
            assert_eq!(all[1].due_at, Some(utc("2026-03-08T07:00:00Z")));
            assert_eq!(all[2].due_at, Some(utc("2026-03-09T06:30:00Z")));
        } else {
            assert!(all.iter().all(|a| a.due_at.is_none()));
        }
        let mut parts = expand(&spec, from, mid).unwrap();
        parts.extend(expand(&spec, mid, until).unwrap());
        assert_eq!(all, parts);
    }
}
