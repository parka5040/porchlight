use super::*;

fn event(time: IcsTime, origin: EventOrigin) -> ImportedEvent {
    ImportedEvent {
        uid: "series".into(),
        recurrence_id: Some(time.clone()),
        origin,
        summary: String::new(),
        description: String::new(),
        location: String::new(),
        start: time.clone(),
        end: time,
        all_day: false,
        time_mode: TimeMode::Fixed,
        tzid: None,
        rrule: None,
        exdates: Vec::new(),
        sequence: 0,
        status: EventStatus::Confirmed,
    }
}

#[test]
fn duplicate_overrides_have_linear_reconciliation_comparisons() {
    let mut state = ImportState::default();
    let start: DateTime<Utc> = "2026-10-11T00:00:00Z".parse().unwrap();
    for hour in 0..100 {
        let time = IcsTime::Fixed(start + Duration::hours(hour));
        state.events.push(event(
            time.clone(),
            EventOrigin::Expanded {
                rule_instance_start: time,
            },
        ));
    }
    let outside = IcsTime::Date(NaiveDate::from_ymd_opt(2027, 1, 1).unwrap());
    for _ in 0..1000 {
        state
            .events
            .push(event(outside.clone(), EventOrigin::Override));
    }
    parse::reconcile(&mut state, chrono_tz::UTC);
    assert!(
        state.reconciliation_checks <= 300,
        "comparison budget exceeded: {}",
        state.reconciliation_checks
    );
    assert_eq!(state.events.len(), 101);
    assert!(state.source_work >= 1100, "reconciliation must charge work");
}

#[test]
fn reconciliation_exhaustion_is_globally_incomplete() {
    let mut state = ImportState {
        source_work: SOURCE_WORK,
        ..Default::default()
    };
    state.events.push(event(
        IcsTime::Date(NaiveDate::from_ymd_opt(2026, 10, 11).unwrap()),
        EventOrigin::Override,
    ));
    parse::reconcile(&mut state, chrono_tz::UTC);
    assert!(
        state
            .warnings
            .iter()
            .any(|w| w.uid.is_none() && w.code == WarningCode::ExpansionLimited),
        "missing global reconciliation budget warning"
    );
}
