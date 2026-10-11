use porchlight_core::items::*;

#[test]
fn audit_limits_expansion_rejects_ancient_daily_history() {
    let spec = SeriesSpec::new(SeriesInput {
        due_date: Some("0001-01-01".parse().unwrap()),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        rrule: Some("FREQ=DAILY".into()),
        ..Default::default()
    })
    .unwrap();
    let result = expand(
        &spec,
        "0001-01-01T00:00:00".parse().unwrap(),
        "2028-04-11T00:00:00".parse().unwrap(),
    );
    assert!(
        result.is_err(),
        "ancient daily history must exceed the materialization budget"
    );
}

#[test]
fn audit_limits_legacy_derivation_prefix_has_stable_sequences() {
    let spec = SeriesSpec::new(SeriesInput {
        due_date: Some("0001-01-01".parse().unwrap()),
        recurrence_mode: Some(RecurrenceMode::Calendar),
        rrule: Some("FREQ=DAILY".into()),
        ..Default::default()
    })
    .unwrap();
    let until = "2028-04-11T00:00:00".parse().unwrap();
    assert_eq!(
        bounded_occurrence_count(&spec, until, MAX_MATERIALIZED_OCCURRENCES).unwrap(),
        10001
    );
    let (anchors, exhausted) =
        expand_capped(&spec, "0001-01-01T00:00:00".parse().unwrap(), until).unwrap();
    assert!(exhausted);
    assert_eq!(anchors.len(), MAX_MATERIALIZED_OCCURRENCES);
    for (seq, anchor) in anchors.iter().enumerate() {
        assert_eq!(anchor.seq as usize, seq);
    }
}
