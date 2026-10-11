use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use porchlight_core::{access::Role, agenda::*, time::parse_tz};
use uuid::Uuid;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
fn utc(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}
fn local(s: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").unwrap()
}
fn date(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}
fn fixed() -> Anchor {
    Anchor {
        starts_at: Some(utc("2026-10-05T14:00:00Z")),
        ends_at: Some(utc("2026-10-05T15:00:00Z")),
        ..Anchor::default()
    }
}
fn floating(start: &str, end: &str) -> Anchor {
    Anchor {
        local_start: Some(local(start)),
        local_end: Some(local(end)),
        ..Anchor::default()
    }
}

#[test]
fn agenda_members_union_and_fallback_do_not_add_an_owner_implicitly() {
    for role in [Role::User, Role::SuperAdmin, Role::Dependent] {
        assert_eq!(
            agenda_members(id(1), &[], Some((id(2), role))),
            [id(2)].into()
        );
        assert_eq!(
            agenda_members(id(1), &[id(3), id(3)], Some((id(2), role))),
            [id(2), id(3)].into()
        );
    }
    assert_eq!(agenda_members(id(1), &[], None), [id(1)].into());
    assert_eq!(
        agenda_members(id(1), &[id(2), id(3)], None),
        [id(2), id(3)].into()
    );
    assert_eq!(
        agenda_members(id(1), &[id(2)], Some((id(2), Role::Dependent))),
        [id(2)].into()
    );
}

#[test]
fn fixed_instants_pass_through_and_buffers_expand_only_busy() {
    for tz in ["America/New_York", "Europe/Berlin"] {
        let row = agenda_row(
            &fixed(),
            Kind::Appointment,
            TimeMode::Fixed,
            false,
            Buffers {
                before_min: 15,
                after_min: 30,
            },
            parse_tz(tz).unwrap(),
        )
        .unwrap();
        assert_eq!(row.starts_at, fixed().starts_at);
        assert_eq!(row.ends_at, fixed().ends_at);
        assert_eq!(
            row.busy,
            Some(BusyRange::new(utc("2026-10-05T13:45:00Z"), utc("2026-10-05T15:30:00Z")).unwrap())
        );
        assert_eq!(row.due_at, None);
        assert_eq!(row.due_date, None);
    }
}

#[test]
fn floating_appointments_resolve_in_each_members_zone() {
    let anchor = floating("2026-10-05T08:00:00", "2026-10-05T09:00:00");
    for (tz, expected) in [
        ("America/New_York", "2026-10-05T12:00:00Z"),
        ("Europe/Berlin", "2026-10-05T06:00:00Z"),
    ] {
        let row = agenda_row(
            &anchor,
            Kind::Appointment,
            TimeMode::Floating,
            false,
            Buffers::default(),
            parse_tz(tz).unwrap(),
        )
        .unwrap();
        assert_eq!(row.starts_at, Some(utc(expected)));
        assert_eq!(row.busy.unwrap().starts_at(), utc(expected));
    }
}

#[test]
fn dst_fold_and_gap_use_the_shared_time_policy() {
    let ny = parse_tz("America/New_York").unwrap();
    let fold = agenda_row(
        &floating("2026-11-01T01:30:00", "2026-11-01T02:30:00"),
        Kind::Appointment,
        TimeMode::Floating,
        false,
        Buffers::default(),
        ny,
    )
    .unwrap();
    assert_eq!(fold.starts_at, Some(utc("2026-11-01T05:30:00Z")));
    assert_eq!(fold.ends_at, Some(utc("2026-11-01T07:30:00Z")));
    let gap = agenda_row(
        &floating("2026-03-08T02:10:00", "2026-03-08T03:30:00"),
        Kind::Appointment,
        TimeMode::Floating,
        false,
        Buffers::default(),
        ny,
    )
    .unwrap();
    assert_eq!(gap.starts_at, Some(utc("2026-03-08T07:00:00Z")));
    assert_eq!(gap.ends_at, Some(utc("2026-03-08T07:30:00Z")));
    // Only the start falls in the gap: keep the independently resolved valid end,
    // rather than adding the 80-minute wall-clock duration to the shifted start.
    assert_eq!(gap.busy.unwrap().ends_at(), utc("2026-03-08T07:30:00Z"));
    let start_only_gap = agenda_row(
        &floating("2026-03-08T02:50:00", "2026-03-08T03:10:00"),
        Kind::Appointment,
        TimeMode::Floating,
        false,
        Buffers::default(),
        ny,
    )
    .unwrap();
    assert_eq!(start_only_gap.starts_at, Some(utc("2026-03-08T07:00:00Z")));
    assert_eq!(start_only_gap.ends_at, Some(utc("2026-03-08T07:10:00Z")));
    assert_eq!(
        start_only_gap.busy,
        Some(BusyRange::new(utc("2026-03-08T07:00:00Z"), utc("2026-03-08T07:10:00Z")).unwrap())
    );
}

#[test]
fn dst_gap_collapse_preserves_wall_clock_duration_and_then_applies_buffers() {
    let ny = parse_tz("America/New_York").unwrap();
    for (buffers, busy_start, busy_end) in [
        (
            Buffers::default(),
            "2026-03-08T07:00:00Z",
            "2026-03-08T07:40:00Z",
        ),
        (
            Buffers {
                before_min: 15,
                after_min: 30,
            },
            "2026-03-08T06:45:00Z",
            "2026-03-08T08:10:00Z",
        ),
    ] {
        let collapsed = agenda_row(
            &floating("2026-03-08T02:10:00", "2026-03-08T02:50:00"),
            Kind::Appointment,
            TimeMode::Floating,
            false,
            buffers,
            ny,
        )
        .unwrap();
        assert_eq!(collapsed.starts_at, Some(utc("2026-03-08T07:00:00Z")));
        assert_eq!(collapsed.ends_at, Some(utc("2026-03-08T07:40:00Z")));
        assert_eq!(
            collapsed.busy,
            Some(BusyRange::new(utc(busy_start), utc(busy_end)).unwrap())
        );
        let new = MemberAgendaRow {
            member_id: id(1),
            occurrence_id: id(10),
            item_id: id(100),
            row: collapsed,
        };
        let existing = record(1, 11, 200, "2026-03-08T07:10:00Z", "2026-03-08T07:20:00Z");
        assert_eq!(
            hard_conflicts(id(1), &[new], &[existing]),
            vec![(id(10), id(11))]
        );
    }
}

#[test]
fn todos_deadlines_and_all_day_appointments_never_have_busy() {
    let ny = parse_tz("America/New_York").unwrap();
    let buffers = Buffers {
        before_min: 1440,
        after_min: 1440,
    };
    let todo = agenda_row(
        &Anchor {
            due_date: Some(date("2026-10-06")),
            ..Anchor::default()
        },
        Kind::Todo,
        TimeMode::Floating,
        false,
        buffers,
        ny,
    )
    .unwrap();
    assert_eq!(todo.due_date, Some(date("2026-10-06")));
    assert_eq!(todo.busy, None);
    assert_eq!(todo.starts_at, None);
    assert_eq!(todo.due_at, None);
    let untimed = agenda_row(
        &Anchor::default(),
        Kind::Todo,
        TimeMode::Fixed,
        false,
        buffers,
        ny,
    )
    .unwrap();
    assert_eq!(untimed, AgendaRow::default());
    for (mode, anchor) in [
        (
            TimeMode::Fixed,
            Anchor {
                due_at: Some(utc("2026-10-05T12:00:00Z")),
                ..Anchor::default()
            },
        ),
        (
            TimeMode::Floating,
            Anchor {
                local_due: Some(local("2026-10-05T08:00:00")),
                ..Anchor::default()
            },
        ),
    ] {
        let deadline = agenda_row(&anchor, Kind::Deadline, mode, false, buffers, ny).unwrap();
        assert_eq!(deadline.due_at, Some(utc("2026-10-05T12:00:00Z")));
        assert_eq!(deadline.due_date, None);
        assert_eq!(deadline.busy, None);
    }
    for (mode, anchor) in [
        (TimeMode::Fixed, fixed()),
        (
            TimeMode::Floating,
            floating("2026-10-05T00:00:00", "2026-10-06T00:00:00"),
        ),
    ] {
        let all_day = agenda_row(&anchor, Kind::Appointment, mode, true, buffers, ny).unwrap();
        assert!(all_day.starts_at.is_some());
        assert!(all_day.ends_at.is_some());
        assert_eq!(all_day.busy, None);
        assert_eq!(all_day.due_date, None);
    }
}

#[test]
fn malformed_anchors_buffers_and_timestamp_overflow_are_errors() {
    let tz = parse_tz("UTC").unwrap();
    for mode in [TimeMode::Fixed, TimeMode::Floating] {
        for kind in [Kind::Appointment, Kind::Deadline] {
            assert_eq!(
                agenda_row(
                    &Anchor::default(),
                    kind,
                    mode,
                    false,
                    Buffers::default(),
                    tz
                ),
                Err(AgendaError::MissingAnchor)
            );
        }
    }
    for buffers in [
        Buffers {
            before_min: -1,
            after_min: 0,
        },
        Buffers {
            before_min: 0,
            after_min: 1441,
        },
    ] {
        assert_eq!(
            agenda_row(
                &fixed(),
                Kind::Appointment,
                TimeMode::Fixed,
                false,
                buffers,
                tz
            ),
            Err(AgendaError::InvalidBuffers)
        );
    }
    let reversed = Anchor {
        starts_at: fixed().ends_at,
        ends_at: fixed().starts_at,
        ..Anchor::default()
    };
    assert_eq!(
        agenda_row(
            &reversed,
            Kind::Appointment,
            TimeMode::Fixed,
            false,
            Buffers::default(),
            tz
        ),
        Err(AgendaError::InvalidSpan)
    );
    assert_eq!(
        agenda_row(
            &floating("2026-10-05T09:00:00", "2026-10-05T08:00:00"),
            Kind::Appointment,
            TimeMode::Floating,
            false,
            Buffers::default(),
            tz
        ),
        Err(AgendaError::InvalidSpan)
    );
    let overflow = Anchor {
        starts_at: Some(DateTime::<Utc>::MIN_UTC),
        ends_at: Some(utc("2026-10-05T15:00:00Z")),
        ..Anchor::default()
    };
    assert_eq!(
        agenda_row(
            &overflow,
            Kind::Appointment,
            TimeMode::Fixed,
            false,
            Buffers {
                before_min: 1,
                after_min: 0
            },
            tz
        ),
        Err(AgendaError::TimestampOverflow)
    );
    assert!(BusyRange::new(utc("2026-10-05T15:00:00Z"), utc("2026-10-05T14:00:00Z")).is_err());
}

fn record(member: u128, occurrence: u128, item: u128, start: &str, end: &str) -> MemberAgendaRow {
    MemberAgendaRow {
        member_id: id(member),
        occurrence_id: id(occurrence),
        item_id: id(item),
        row: AgendaRow {
            busy: Some(BusyRange::new(utc(start), utc(end)).unwrap()),
            ..AgendaRow::default()
        },
    }
}

#[test]
fn conflicts_are_half_open_member_scoped_and_exclude_only_the_same_occurrence() {
    let new = record(1, 10, 100, "2026-10-05T14:00:00Z", "2026-10-05T15:00:00Z");
    let existing = vec![
        new.clone(), // self is never a conflict
        record(1, 11, 100, "2026-10-05T14:30:00Z", "2026-10-05T15:30:00Z"), // same item, different occurrence
        record(1, 12, 200, "2026-10-05T13:00:00Z", "2026-10-05T14:00:00Z"),
        record(1, 13, 200, "2026-10-05T15:00:00Z", "2026-10-05T16:00:00Z"),
        record(2, 14, 200, "2026-10-05T14:00:00Z", "2026-10-05T15:00:00Z"),
        record(1, 15, 200, "2026-10-05T14:30:00Z", "2026-10-05T14:30:00Z"), // empty
        MemberAgendaRow {
            occurrence_id: id(16),
            row: AgendaRow::default(),
            ..new.clone()
        },
        record(1, 11, 100, "2026-10-05T14:30:00Z", "2026-10-05T15:30:00Z"), // duplicate input
    ];
    assert_eq!(
        hard_conflicts(id(1), std::slice::from_ref(&new), &existing),
        vec![(id(10), id(11))]
    );
    assert!(hard_conflicts(id(2), &[new], &existing).is_empty());
}

#[test]
fn buffer_only_overlap_counts_and_conflict_output_is_deterministic() {
    let tz = parse_tz("UTC").unwrap();
    let row = agenda_row(
        &fixed(),
        Kind::Appointment,
        TimeMode::Fixed,
        false,
        Buffers {
            before_min: 15,
            after_min: 0,
        },
        tz,
    )
    .unwrap();
    let new = MemberAgendaRow {
        member_id: id(1),
        occurrence_id: id(10),
        item_id: id(100),
        row,
    };
    let mut existing = vec![
        record(1, 12, 200, "2026-10-05T13:00:00Z", "2026-10-05T14:00:00Z"),
        record(1, 11, 100, "2026-10-05T14:00:00Z", "2026-10-05T15:00:00Z"),
    ];
    let expected = vec![(id(10), id(11)), (id(10), id(12))];
    assert_eq!(
        hard_conflicts(id(1), std::slice::from_ref(&new), &existing),
        expected
    );
    existing.reverse();
    assert_eq!(hard_conflicts(id(1), &[new], &existing), expected);
}

#[test]
fn agenda_db_strings_match_items_schema() {
    for (text, kind) in [
        ("todo", Kind::Todo),
        ("deadline", Kind::Deadline),
        ("appointment", Kind::Appointment),
    ] {
        assert_eq!(text.parse::<Kind>().unwrap(), kind);
        assert_eq!(kind.as_str(), text);
    }
    for (text, mode) in [("fixed", TimeMode::Fixed), ("floating", TimeMode::Floating)] {
        assert_eq!(text.parse::<TimeMode>().unwrap(), mode);
        assert_eq!(mode.as_str(), text);
    }
    assert!("task".parse::<Kind>().is_err());
    assert!("local".parse::<TimeMode>().is_err());
}

#[test]
fn a_new_series_conflicts_with_its_own_overlapping_new_occurrences() {
    let first = record(1, 10, 100, "2026-10-05T14:00:00Z", "2026-10-05T15:00:00Z");
    let second = record(1, 11, 100, "2026-10-05T14:30:00Z", "2026-10-05T15:30:00Z");
    let rows = [first, second];
    let expected = vec![(id(10), id(11)), (id(11), id(10))];
    assert_eq!(hard_conflicts(id(1), &rows, &[]), expected);
    // Passing new rows among already-persisted existing rows does not duplicate warnings.
    assert_eq!(hard_conflicts(id(1), &rows, &rows), expected);
}
