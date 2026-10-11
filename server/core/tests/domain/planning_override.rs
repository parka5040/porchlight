use porchlight_core::{
    agenda::{self, Anchor, Buffers},
    items::{OccurrenceAnchor, SeriesInput, SeriesSpec, this_occurrence},
};
#[test]
fn scheduled_todo_preserves_deadline_and_generates_busy_span() {
    let input: SeriesInput = serde_json::from_value(
        serde_json::json!({"kind":"todo","time_mode":"floating","due_date":"2026-10-12"}),
    )
    .unwrap();
    let spec = SeriesSpec::new(input).unwrap();
    let replacement = OccurrenceAnchor {
        seq: 0,
        local_start: Some("2026-10-12T09:00:00".parse().unwrap()),
        local_end: Some("2026-10-12T10:00:00".parse().unwrap()),
        local_due: None,
        due_date: Some("2026-10-12".parse().unwrap()),
        starts_at: None,
        ends_at: None,
        due_at: None,
    };
    let edit = this_occurrence(&spec, 0, &replacement).unwrap();
    assert_eq!(edit.override_local_start, replacement.local_start);
    let row = agenda::agenda_row(
        &Anchor {
            local_start: replacement.local_start,
            local_end: replacement.local_end,
            local_due: None,
            due_date: replacement.due_date,
            starts_at: None,
            ends_at: None,
            due_at: None,
        },
        spec.input().kind,
        spec.input().time_mode,
        false,
        Buffers {
            before_min: 0,
            after_min: 0,
        },
        chrono_tz::America::New_York,
    )
    .unwrap();
    assert!(row.busy.is_some());
    assert_eq!(
        row.starts_at.unwrap().to_rfc3339(),
        "2026-10-12T13:00:00+00:00"
    );
    assert_eq!(row.due_date, replacement.due_date);
}
