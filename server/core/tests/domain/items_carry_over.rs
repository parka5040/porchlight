use chrono::{DateTime, Duration, Utc};
use porchlight_core::items::{
    Disposition, MissedPolicy, OccurrenceAnchor, OpenOccurrence, Status, dispositions,
};

fn now() -> DateTime<Utc> {
    "2026-01-04T13:00:00Z".parse().unwrap()
}
fn row(seq: u32, days: i64, disposition: Disposition) -> OpenOccurrence {
    OpenOccurrence {
        anchor: OccurrenceAnchor {
            seq,
            due_at: Some(now() + Duration::days(days)),
            ..Default::default()
        },
        status: Status::Todo,
        disposition,
        closed_at: None,
        generated_anchor: None,
    }
}
fn apply(rows: &mut [OpenOccurrence], clock: DateTime<Utc>) {
    let values = dispositions(rows, clock, MissedPolicy::CarryOver, chrono_tz::UTC).unwrap();
    for (r, d) in rows.iter_mut().zip(values) {
        if r.status.is_open() {
            r.disposition = d;
        }
    }
}

#[test]
fn carried_identity_and_superseded_expiry_survive_clock_completion_and_input_order() {
    for reverse in [false, true] {
        let mut rows = vec![
            row(0, -3, Disposition::Open),
            row(1, -2, Disposition::Open),
            row(2, -1, Disposition::Open),
            row(3, 1, Disposition::Open),
        ];
        if reverse {
            rows.reverse();
        }
        apply(&mut rows, now());
        let carried = rows.iter().find(|r| r.anchor.seq == 2).unwrap();
        assert_eq!(carried.disposition, Disposition::Overdue);
        apply(&mut rows, now() + Duration::days(2));
        assert!(rows.iter().all(|r| r.disposition
            == if r.anchor.seq == 2 {
                Disposition::Overdue
            } else {
                Disposition::Expired
            }));
        let closed = rows.iter_mut().find(|r| r.anchor.seq == 2).unwrap();
        closed.status = Status::Done;
        closed.closed_at = Some(now() + Duration::days(2));
        apply(&mut rows, now() + Duration::days(2));
        assert!(
            rows.iter()
                .filter(|r| r.status.is_open())
                .all(|r| r.disposition == Disposition::Expired)
        );
        rows.push(row(4, 3, Disposition::Open));
        apply(&mut rows, now() + Duration::days(4));
        assert_eq!(rows.last().unwrap().disposition, Disposition::Overdue);
        assert!(
            rows.iter()
                .filter(|r| r.anchor.seq < 4 && r.status.is_open())
                .all(|r| r.disposition == Disposition::Expired)
        );
    }
}

#[test]
fn explicit_future_reschedule_resets_expired_and_other_policies_remain_stateless() {
    let mut rows = vec![row(0, -1, Disposition::Expired)];
    assert_eq!(
        dispositions(&rows, now(), MissedPolicy::Stack, chrono_tz::UTC).unwrap(),
        [Disposition::Overdue]
    );
    assert_eq!(
        dispositions(&rows, now(), MissedPolicy::Skip, chrono_tz::UTC).unwrap(),
        [Disposition::Expired]
    );
    rows[0].anchor.due_at = Some(now() + Duration::days(1));
    apply(&mut rows, now());
    assert_eq!(rows[0].disposition, Disposition::Open);
    apply(&mut rows, now() + Duration::days(2));
    assert_eq!(rows[0].disposition, Disposition::Overdue);
}

#[test]
fn rescheduled_carried_row_covers_peers_until_its_new_boundary_and_closure() {
    for reverse in [false, true] {
        for refreshed in [false, true] {
            for transition in [false, true] {
                for close_early in [false, true] {
                    let mut rows = vec![
                        row(0, -3, Disposition::Overdue),
                        row(1, -2, Disposition::Open),
                        row(2, -1, Disposition::Open),
                    ];
                    if reverse {
                        rows.reverse();
                    }
                    if refreshed {
                        apply(&mut rows, now());
                    }
                    let carried = rows.iter_mut().find(|r| r.anchor.seq == 0).unwrap();
                    carried.generated_anchor = Some(carried.anchor.clone());
                    carried.anchor.due_at = Some(now() + Duration::days(2));
                    apply(&mut rows, now());
                    assert!(
                        rows.iter().all(|r| r.disposition
                            == if r.anchor.seq == 0 {
                                Disposition::Open
                            } else {
                                Disposition::Expired
                            }),
                        "rescheduled row must cover peers (refresh={refreshed})"
                    );
                    if transition {
                        let skipped =
                            dispositions(&rows, now(), MissedPolicy::Skip, chrono_tz::UTC).unwrap();
                        for (r, d) in rows.iter_mut().zip(skipped) {
                            r.disposition = d;
                        }
                        apply(&mut rows, now());
                        assert!(rows.iter().all(|r| r.disposition != Disposition::Overdue));
                    }
                    if close_early {
                        let carried = rows.iter_mut().find(|r| r.anchor.seq == 0).unwrap();
                        carried.status = Status::Done;
                        carried.closed_at = Some(now() + Duration::days(1));
                    }
                    apply(&mut rows, now() + Duration::days(3));
                    assert!(
                        rows.iter()
                            .filter(|r| r.status.is_open())
                            .all(|r| r.disposition
                                == if r.anchor.seq == 0 {
                                    Disposition::Overdue
                                } else {
                                    Disposition::Expired
                                }),
                        "moved row must become carried; closing it early must not resurrect peers"
                    );
                }
            }
        }
    }
}

#[test]
fn switching_skip_and_stack_into_carry_over_elects_one_overdue() {
    for previous in [MissedPolicy::Skip, MissedPolicy::Stack] {
        let mut rows = vec![row(0, -3, Disposition::Open), row(1, -1, Disposition::Open)];
        let initial = dispositions(&rows, now(), previous, chrono_tz::UTC).unwrap();
        for (r, d) in rows.iter_mut().zip(initial) {
            r.disposition = d;
        }
        apply(&mut rows, now());
        assert_eq!(
            rows.iter()
                .filter(|r| r.disposition == Disposition::Overdue)
                .count(),
            1
        );
        let skipped = dispositions(&rows, now(), MissedPolicy::Skip, chrono_tz::UTC).unwrap();
        for (r, d) in rows.iter_mut().zip(skipped) {
            r.disposition = d;
        }
        apply(&mut rows, now());
        assert_eq!(
            rows.iter()
                .filter(|r| r.disposition == Disposition::Overdue)
                .count(),
            1
        );
        assert_eq!(rows[1].disposition, Disposition::Overdue);
    }
}

#[test]
fn closing_carried_row_suppresses_peers_without_a_prior_refresh() {
    for refreshed in [false, true] {
        let mut rows = vec![
            row(0, -3, Disposition::Overdue),
            row(1, -1, Disposition::Open),
            row(2, 1, Disposition::Open),
        ];
        if refreshed {
            apply(&mut rows, now());
        }
        rows[0].status = Status::Done;
        rows[0].closed_at = Some(now());
        apply(&mut rows, now());
        assert_eq!(
            rows[1].disposition,
            Disposition::Expired,
            "refresh={refreshed}: boundary missed while prior row was carried"
        );
        apply(&mut rows, now() + Duration::days(2));
        assert_eq!(rows[1].disposition, Disposition::Expired);
        assert_eq!(rows[2].disposition, Disposition::Overdue);
    }
}
