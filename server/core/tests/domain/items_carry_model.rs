//! R63 carry-over contract (progress.md Ruling R63; derivation-matrix.md, "R63 carry-over
//! contract"). Carry-over guarantees are invariants over current persisted state, plus one
//! documented deterministic choice of which instance carries.
//!
//! - `Model::rule` restates that choice independently, over hours and row indices.
//! - After every operation, `Model::derive` checks that the engine equals the rule, is
//!   idempotent and is independent of input order. `Model::invariants` then checks the five
//!   R63 invariants, using history the engine never sees (closures, the op, the previous
//!   carried row).
//! - The re-review probes P1–P8 are explicit regressions below.
//!
//! Round 3's oracle remembered ownership across history. R63 replaces it: identity follows the
//! rule, so histories with identical persisted inputs carry the same row
//! (`indistinguishable_histories_follow_the_documented_rule`).
use std::cmp::Reverse;

use chrono::{DateTime, Duration, Utc};
use porchlight_core::items::{
    Disposition as D, MissedPolicy as Policy, OccurrenceAnchor, OpenOccurrence, Status,
    dispositions,
};
use proptest::{prelude::*, test_runner::RngSeed};

const OV: D = D::Overdue;
const EX: D = D::Expired;
const OP: D = D::Open;

#[path = "audit_bl_tests_carry.rs"]
mod audit_bl_tests;

fn instant(hours: i64) -> DateTime<Utc> {
    "2026-01-04T13:00:00Z".parse::<DateTime<Utc>>().unwrap() + Duration::hours(hours)
}

#[derive(Clone, Debug)]
struct Row {
    generated: i64,
    effective: i64,
    detached: bool,
    status: Status,
    closed: Option<i64>,
    touched: bool,
    removed: bool,
    /// Last derived disposition. Closed rows keep the value they had when closed.
    persisted: D,
}

impl Row {
    fn open(&self) -> bool {
        !self.removed && self.status.is_open()
    }

    fn closed_at(&self) -> Option<i64> {
        self.closed
            .filter(|_| !self.removed && !self.status.is_open())
    }

    /// Due-since: the earlier of the generated and the effective boundary.
    fn since(&self) -> i64 {
        if self.detached {
            self.generated.min(self.effective)
        } else {
            self.effective
        }
    }

    /// Detached later than its generated boundary, which has passed by `t`.
    fn postponed_at(&self, t: i64) -> bool {
        self.detached && self.generated < t && self.effective > self.generated
    }
}

#[derive(Clone, Debug)]
enum Op {
    Advance(u8),
    Complete(usize),
    Reopen(usize),
    Move(usize, i16), // absolute hour; includes backward moves and sibling ties
    Doing(usize),
    Policy(Policy),
    Revert, // revert the most recent timing or policy edit
}

#[derive(Clone, Debug)]
enum Undo {
    Move(usize, i64, bool),
    Policy(Policy),
}

/// History only: the carried (overdue) row closed under carry_over and the peers it covered.
#[derive(Clone, Debug)]
struct Closure {
    row: usize,
    at: i64,
    peers: Vec<usize>,
}

#[derive(Clone, Debug)]
struct Model {
    rows: Vec<Row>,
    clock: i64,
    policy: Policy,
    completion: bool,
    undo: Vec<Undo>,
    closures: Vec<Closure>,
}

impl Model {
    fn new(times: &[i64], clock: i64, policy: Policy, completion: bool) -> Self {
        let mut m = Self {
            rows: Vec::new(),
            clock,
            policy,
            completion,
            undo: Vec::new(),
            closures: Vec::new(),
        };
        for &t in times {
            m.push(t);
        }
        m
    }

    fn push(&mut self, t: i64) {
        self.rows.push(Row {
            generated: t,
            effective: t,
            detached: false,
            status: Status::Todo,
            closed: None,
            touched: false,
            removed: false,
            persisted: D::Open,
        });
    }

    fn missed(&self, i: usize) -> bool {
        self.rows[i].open() && self.rows[i].effective < self.clock
    }

    /// Strict due-since order; the sequence (= index) breaks ties.
    fn older(&self, p: usize, w: usize) -> bool {
        (self.rows[p].since(), p) < (self.rows[w].since(), w)
    }

    /// The documented R63 rule, restated without engine code.
    fn rule(&self) -> Vec<D> {
        let r = &self.rows;
        let all = 0..r.len();
        // Older postponed or non-expired work is never superseded by younger postponed work.
        let protected = |p: usize, w: usize| {
            self.older(p, w) && (r[p].postponed_at(self.clock) || r[p].persisted != D::Expired)
        };
        let retained = |p: usize| self.missed(p) && r[p].persisted == D::Overdue;
        let closure_covered = |p: usize| {
            !retained(p)
                && r.iter().enumerate().any(|(c, row)| {
                    row.closed_at().is_some_and(|at| {
                        r[p].effective < at
                            && match row.persisted {
                                D::Overdue => true,
                                D::Open => row.postponed_at(at) && !protected(p, c),
                                D::Expired => false,
                            }
                    })
                })
        };
        let source = |w: usize| r[w].open() && r[w].postponed_at(self.clock) && !closure_covered(w);
        let covered = |p: usize| {
            closure_covered(p)
                || all.clone().any(|w| {
                    w != p && source(w) && r[p].effective < r[w].effective && !protected(p, w)
                })
        };
        let claim = |p: usize| retained(p) || r[p].postponed_at(self.clock);
        let eligible: Vec<usize> = all
            .clone()
            .filter(|&p| self.missed(p) && !covered(p))
            .collect();
        let latest = |p: &usize| (r[*p].effective, Reverse(*p));
        let carried = eligible
            .iter()
            .copied()
            .filter(|&p| claim(p))
            .min_by_key(|&p| (r[p].since(), p))
            .or_else(|| {
                eligible
                    .iter()
                    .copied()
                    .filter(|&p| r[p].persisted != D::Expired)
                    .max_by_key(latest)
            })
            .or_else(|| eligible.iter().copied().max_by_key(latest));
        all.filter(|&i| !r[i].removed)
            .map(|i| match self.policy {
                _ if !self.missed(i) => D::Open,
                Policy::Stack => D::Overdue,
                Policy::Skip => D::Expired,
                Policy::CarryOver if carried == Some(i) => D::Overdue,
                Policy::CarryOver => D::Expired,
            })
            .collect()
    }

    fn inputs(&self) -> Vec<OpenOccurrence> {
        let anchor = |seq: usize, hours: i64| OccurrenceAnchor {
            seq: seq as u32,
            due_at: Some(instant(hours)),
            ..Default::default()
        };
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.removed)
            .map(|(i, r)| OpenOccurrence {
                anchor: anchor(i, r.effective),
                generated_anchor: r.detached.then(|| anchor(i, r.generated)),
                status: r.status,
                disposition: r.persisted,
                closed_at: r.closed.map(instant),
            })
            .collect()
    }

    fn overdue(&self) -> Vec<usize> {
        (0..self.rows.len())
            .filter(|&i| self.rows[i].open() && self.rows[i].persisted == D::Overdue)
            .collect()
    }

    /// Derive as production does after every write and clock refresh: the engine must equal
    /// the rule (5), ignore input order, and be idempotent. Persists the open rows' output.
    fn derive(&mut self) -> Result<Vec<D>, String> {
        let at = instant(self.clock);
        let input = self.inputs();
        let actual = dispositions(&input, at, self.policy, chrono_tz::UTC).unwrap();
        let expected = self.rule();
        if actual != expected {
            return Err(format!(
                "(5) clock={} policy={:?}: engine={actual:?} rule={expected:?}",
                self.clock, self.policy
            ));
        }
        let mut reversed = input;
        reversed.reverse();
        let mut back = dispositions(&reversed, at, self.policy, chrono_tz::UTC).unwrap();
        back.reverse();
        if back != actual {
            return Err(format!("input order: {back:?} != {actual:?}"));
        }
        let live: Vec<usize> = (0..self.rows.len())
            .filter(|&i| !self.rows[i].removed)
            .collect();
        for (&i, &d) in live.iter().zip(&actual) {
            if self.rows[i].open() {
                self.rows[i].persisted = d;
            }
        }
        let again = dispositions(&self.inputs(), at, self.policy, chrono_tz::UTC).unwrap();
        if again != actual {
            return Err(format!("idempotence: {again:?} after {actual:?}"));
        }
        if self.policy == Policy::Stack {
            // Stack shows every missed row overdue; returning to carry_over retains one of them.
            self.closures.clear();
        }
        Ok(actual)
    }

    /// R63 invariants under carry_over after an operation. `edited` is the row an explicit
    /// edit (move, revert, reopen) touched; `before` the row overdue before the operation.
    fn invariants(&self, edited: Option<usize>, before: Option<usize>) -> Result<(), String> {
        if self.policy != Policy::CarryOver {
            return Ok(());
        }
        let r = &self.rows;
        let overdue = self.overdue();
        // (1), (3): never a second overdue instance, including after moving work back.
        if overdue.len() > 1 {
            return Err(format!("(1) several overdue: {overdue:?}"));
        }
        // (2): closed carried work never resurrects the peers it covered.
        for c in &self.closures {
            for &p in &c.peers {
                if r[p].open() && r[p].effective < c.at && r[p].persisted == D::Overdue {
                    return Err(format!("(2) closing {} at {} resurrected {p}", c.row, c.at));
                }
            }
        }
        // (1), (3), (4): nothing is overdue only while all missed work is covered by carry
        // evidence: closed carried work, or postponed work whose generated boundary passed.
        if overdue.is_empty() {
            for p in (0..r.len()).filter(|&p| self.missed(p)) {
                let protected = |w| {
                    self.older(p, w)
                        && (r[p].postponed_at(self.clock) || r[p].persisted != D::Expired)
                };
                let closure = r.iter().enumerate().any(|(w, c)| {
                    c.closed_at().is_some_and(|at| {
                        r[p].effective < at
                            && (c.persisted == D::Overdue
                                || (c.persisted == D::Open && c.postponed_at(at) && !protected(w)))
                    })
                });
                let postponed = (0..r.len()).any(|w| {
                    w != p
                        && r[w].open()
                        && r[w].postponed_at(self.clock)
                        && r[p].effective < r[w].effective
                        && !protected(w)
                });
                if !closure && !postponed {
                    return Err(format!(
                        "(1) missed {p} is uncovered and nothing is overdue"
                    ));
                }
            }
        }
        // (3), (5): carried work survives time, Doing, closure of other rows and edits of
        // younger rows. Only an explicit edit of itself, or one that lets older work carry,
        // hands the overdue state to another row.
        if let Some(x) = before.filter(|&x| self.missed(x) && r[x].persisted != D::Overdue) {
            let older_carrier = match overdue.first() {
                Some(&y) => self.older(y, x),
                None => (0..r.len()).any(|w| {
                    r[w].open()
                        && r[w].postponed_at(self.clock)
                        && r[x].effective < r[w].effective
                        && self.older(w, x)
                }),
            };
            if !edited.is_some_and(|i| i == x || older_carrier) {
                return Err(format!(
                    "(3) carried {x} lost its overdue state to {overdue:?}"
                ));
            }
        }
        Ok(())
    }

    /// Applies one operation; returns the row an explicit edit touched.
    fn operation(&mut self, op: &Op) -> Option<usize> {
        let index = |i: usize| i % self.rows.len();
        match *op {
            Op::Advance(n) => {
                self.clock += i64::from(n);
                None
            }
            Op::Complete(raw) => {
                let i = index(raw);
                if !self.rows[i].open() {
                    return None;
                }
                if self.policy == Policy::CarryOver && self.rows[i].persisted == D::Overdue {
                    let peers = (0..self.rows.len())
                        .filter(|&p| p != i && self.missed(p))
                        .collect();
                    self.closures.push(Closure {
                        row: i,
                        at: self.clock,
                        peers,
                    });
                }
                self.rows[i].status = Status::Done;
                self.rows[i].closed = Some(self.clock);
                if self.completion && !self.rows.iter().any(Row::open) {
                    self.push(self.clock + 24);
                }
                None
            }
            Op::Reopen(raw) => {
                let i = index(raw);
                if self.rows[i].removed || self.rows[i].status != Status::Done {
                    return None;
                }
                // R31: only the latest closure removes an untouched successor.
                let latest = self
                    .rows
                    .iter()
                    .enumerate()
                    .filter_map(|(j, r)| r.closed.map(|t| (t, j)))
                    .max();
                if self.completion && latest == self.rows[i].closed.map(|t| (t, i)) {
                    for row in &mut self.rows {
                        if row.open() && !row.touched {
                            row.removed = true;
                        }
                    }
                }
                self.rows[i].status = Status::Todo;
                self.closures.retain(|c| c.row != i);
                Some(i)
            }
            Op::Move(raw, to) => {
                let i = index(raw);
                if !self.rows[i].open() {
                    return None;
                }
                let r = &mut self.rows[i];
                self.undo.push(Undo::Move(i, r.effective, r.detached));
                r.effective = i64::from(to);
                r.detached = true;
                r.touched = true;
                self.forget_moved(i);
                Some(i)
            }
            Op::Doing(raw) => {
                let i = index(raw);
                if self.rows[i].open() {
                    self.rows[i].status = Status::Doing;
                    self.rows[i].touched = true;
                }
                None
            }
            Op::Policy(p) => {
                self.undo.push(Undo::Policy(self.policy));
                self.policy = p;
                None
            }
            Op::Revert => match self.undo.pop() {
                Some(Undo::Policy(p)) => {
                    self.policy = p;
                    None
                }
                Some(Undo::Move(i, t, detached)) if self.rows[i].open() => {
                    self.rows[i].effective = t;
                    self.rows[i].detached = detached;
                    self.forget_moved(i);
                    Some(i)
                }
                _ => None,
            },
        }
    }

    /// Judged by its effective boundary: work moved past a closure is no longer its peer.
    fn forget_moved(&mut self, i: usize) {
        let t = self.rows[i].effective;
        for c in &mut self.closures {
            if t >= c.at {
                c.peers.retain(|&p| p != i);
            }
        }
    }

    fn start(&mut self) -> Result<Vec<D>, String> {
        let actual = self.derive()?;
        self.invariants(None, None)?;
        Ok(actual)
    }

    fn try_step(&mut self, op: &Op) -> Result<Vec<D>, String> {
        let before = if self.policy == Policy::CarryOver {
            self.overdue().first().copied()
        } else {
            None
        };
        let edited = self.operation(op);
        let actual = self.derive()?;
        self.invariants(edited, before)?;
        Ok(actual)
    }

    fn step(&mut self, op: Op) -> Vec<D> {
        self.try_step(&op)
            .unwrap_or_else(|e| panic!("after {op:?}: {e}"))
    }
}

/// seq0 carried since clock -60; seq1/seq2 missed later and expired; seq3.. in the future.
fn carried() -> Model {
    let mut m = Model::new(
        &[-72, -48, -24, 24, 48, 72, 96],
        -60,
        Policy::CarryOver,
        false,
    );
    assert_eq!(m.start().unwrap(), [OV, OP, OP, OP, OP, OP, OP]);
    assert_eq!(m.step(Op::Advance(60)), [OV, EX, EX, OP, OP, OP, OP]);
    m
}

#[test]
fn p1_future_move_keeps_carried_doing_work() {
    let mut m = carried();
    assert_eq!(m.step(Op::Doing(0)), [OV, EX, EX, OP, OP, OP, OP]);
    assert_eq!(m.step(Op::Move(3, 36)), [OV, EX, EX, OP, OP, OP, OP]);
    // seq3's generated boundary passed: the younger postponed row does not cover seq0.
    assert_eq!(m.step(Op::Advance(30)), [OV, EX, EX, OP, OP, OP, OP]);
    // seq3's new boundary passed: it is younger missed work behind the carried row.
    assert_eq!(m.step(Op::Advance(10)), [OV, EX, EX, EX, OP, OP, OP]);
}

#[test]
fn p2_carried_forward_then_back_to_original() {
    let mut m = carried();
    assert_eq!(m.step(Op::Move(0, 12)), [OP, EX, EX, OP, OP, OP, OP]);
    // Moved back, seq0 is missed again and was not expired: it outranks expired peers.
    assert_eq!(m.step(Op::Move(0, -72)), [OV, EX, EX, OP, OP, OP, OP]);
}

#[test]
fn p3_timed_tie_keeps_the_moved_identity() {
    let mut m = carried();
    assert_eq!(m.step(Op::Move(0, 48)), [OP, EX, EX, OP, OP, OP, OP]);
    // seq0 and seq4 share the boundary; postponed work is a claim and outranks seq4.
    assert_eq!(m.step(Op::Advance(49)), [OV, EX, EX, EX, EX, OP, OP]);
}

#[test]
fn p3b_refresh_gap_keeps_the_moved_identity() {
    let mut m = carried();
    assert_eq!(m.step(Op::Move(0, 47)), [OP, EX, EX, OP, OP, OP, OP]);
    assert_eq!(m.step(Op::Advance(49)), [OV, EX, EX, EX, EX, OP, OP]);
}

#[test]
fn p3b_intermediate_refresh_control() {
    let mut m = carried();
    assert_eq!(m.step(Op::Move(0, 47)), [OP, EX, EX, OP, OP, OP, OP]);
    assert_eq!(m.step(Op::Advance(48)), [OV, EX, EX, EX, OP, OP, OP]);
    assert_eq!(m.step(Op::Advance(1)), [OV, EX, EX, EX, EX, OP, OP]);
}

#[test]
fn p3_date_only_midnight_tie_keeps_doing_identity() {
    let mut m = carried();
    m.step(Op::Doing(0));
    m.step(Op::Move(0, 48));
    let mut input = m.inputs();
    for r in &mut input {
        for a in std::iter::once(&mut r.anchor).chain(r.generated_anchor.as_mut()) {
            a.due_date = Some(a.due_at.take().unwrap().date_naive());
        }
    }
    let at = "2026-01-07T00:01:00Z".parse().unwrap();
    let actual = dispositions(&input, at, Policy::CarryOver, chrono_tz::UTC).unwrap();
    assert_eq!(actual, [OV, EX, EX, EX, EX, OP, OP]);
}

// The supplied re-reviews do not describe a P4. This is their separately documented
// early-closure control, labelled as a reconstruction.
#[test]
fn p4_reconstructed_early_closure_does_not_resurrect_peers() {
    let mut m = carried();
    assert_eq!(m.step(Op::Move(0, 36)), [OP, EX, EX, OP, OP, OP, OP]);
    assert_eq!(m.step(Op::Complete(0)), [OP, EX, EX, OP, OP, OP, OP]);
    assert_eq!(m.step(Op::Advance(12)), [OP, EX, EX, OP, OP, OP, OP]);
    // Work missed after the closure carries again.
    assert_eq!(m.step(Op::Advance(13)), [OP, EX, EX, OV, OP, OP, OP]);
}

#[test]
fn p5_carried_forward_then_back_between_old_peers() {
    let mut m = carried();
    assert_eq!(m.step(Op::Move(0, 12)), [OP, EX, EX, OP, OP, OP, OP]);
    assert_eq!(m.step(Op::Move(0, -36)), [OV, EX, EX, OP, OP, OP, OP]);
}

#[test]
fn p6_moving_superseded_peer_does_not_steal_carry() {
    let mut m = carried();
    assert_eq!(m.step(Op::Move(1, 36)), [OV, OP, EX, OP, OP, OP, OP]);
    // At its new boundary the younger postponed peer is still behind the carried row.
    assert_eq!(m.step(Op::Advance(37)), [OV, EX, EX, EX, OP, OP, OP]);
}

// R63 rewrite: round 3 expected seq1 (latest effective boundary). The stack era left both
// missed rows overdue, so no state records which one was carried. R63 (4) requires exactly
// one; the documented rule ranks retained overdue claims by due-since, so seq0 carries.
#[test]
fn p7_stack_to_carry_uses_effective_not_generated_time() {
    let mut m = Model::new(&[-48, -24, 24, 48, 72], 0, Policy::Stack, false);
    assert_eq!(m.start().unwrap(), [OV, OV, OP, OP, OP]);
    assert_eq!(m.step(Op::Move(2, 36)), [OV, OV, OP, OP, OP]);
    assert_eq!(m.step(Op::Advance(30)), [OV, OV, OP, OP, OP]);
    assert_eq!(m.step(Op::Policy(Policy::CarryOver)), [OV, EX, OP, OP, OP]);
    assert_eq!(m.step(Op::Advance(7)), [OV, EX, EX, OP, OP]);
}

#[test]
fn p8_two_forward_moves_keep_the_carried_row() {
    let mut m = carried();
    assert_eq!(m.step(Op::Move(0, 36)), [OP, EX, EX, OP, OP, OP, OP]);
    assert_eq!(m.step(Op::Move(3, 60)), [OP, EX, EX, OP, OP, OP, OP]);
    assert_eq!(m.step(Op::Advance(37)), [OV, EX, EX, OP, OP, OP, OP]);
    assert_eq!(m.step(Op::Advance(24)), [OV, EX, EX, EX, EX, OP, OP]);
}

#[test]
fn postponing_older_superseded_work_hands_it_the_carry() {
    // Documented takeover (indistinguishable from a postponed carried row whose younger peers
    // a stack era made overdue): older postponed work covers younger missed work.
    let mut m = Model::new(&[-72, -48, -24, 72], 0, Policy::CarryOver, false);
    assert_eq!(m.start().unwrap(), [EX, EX, OV, OP]);
    assert_eq!(m.step(Op::Move(0, 36)), [OP, EX, EX, OP]);
    assert_eq!(m.step(Op::Advance(37)), [OV, EX, EX, OP]);
}

#[test]
fn indistinguishable_histories_follow_the_documented_rule() {
    // Round 3's proof histories. A leaves seq0 at its slot. B postpones still-future seq0,
    // so under round 3's history identity B would carry seq1. Both later restore seq0, then
    // move seq0 and seq1 to +48, and every persisted engine input coincides. Under R63 the
    // rule decides: the claim with the oldest due-since (seq0) carries in both.
    let mut a = Model::new(&[-72, -48, 72], -80, Policy::CarryOver, false);
    a.start().unwrap();
    let mut b = a.clone();
    a.step(Op::Move(0, -72));
    b.step(Op::Move(0, -36));
    for m in [&mut a, &mut b] {
        for hours in [20, 20, 40] {
            m.step(Op::Advance(hours));
        }
        for op in [Op::Move(0, -72), Op::Move(0, 48), Op::Move(1, 48)] {
            m.step(op);
        }
    }
    assert_eq!(
        a.inputs(),
        b.inputs(),
        "all persisted engine inputs coincide"
    );
    assert_eq!(a.step(Op::Advance(49)), [OV, EX, OP]);
    assert_eq!(b.step(Op::Advance(49)), [OV, EX, OP]);
}

fn operations() -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec(
        prop_oneof![
            4 => (0u8..49).prop_map(Op::Advance),
            2 => (0usize..12).prop_map(Op::Complete),
            2 => (0usize..12).prop_map(Op::Reopen),
            4 => (0usize..12, -96i16..193).prop_map(|(i, t)| Op::Move(i, t)),
            2 => (0usize..12, -4i16..9).prop_map(|(i, t)| Op::Move(i, t * 24)),
            2 => (0usize..12).prop_map(Op::Doing),
            2 => policies().prop_map(Op::Policy),
            2 => Just(Op::Revert),
        ],
        1..97,
    )
}

fn policies() -> impl Strategy<Value = Policy> {
    prop::sample::select(vec![Policy::CarryOver, Policy::Skip, Policy::Stack])
}

fn run(completion: bool, clock: i64, policy: Policy, ops: &[Op]) -> Result<(), TestCaseError> {
    let times: Vec<_> = if completion {
        vec![-72]
    } else {
        (0..12).map(|i| -72 + 24 * i).collect()
    };
    let mut m = Model::new(&times, clock, policy, completion);
    m.start().map_err(TestCaseError::fail)?;
    for (step, op) in ops.iter().enumerate() {
        m.try_step(op)
            .map_err(|e| TestCaseError::fail(format!("step {step}, {op:?}: {e}")))?;
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256, rng_seed: RngSeed::Fixed(2026100604), failure_persistence: None,
        max_shrink_iters: 4096, ..ProptestConfig::default()
    })]

    #[test]
    fn r63_contract_over_operation_sequences_fixed_seed(
        completion in any::<bool>(), clock in -80i64..1, policy in policies(), ops in operations()
    ) {
        run(completion, clock, policy, &ops)?;
    }
}

// Fast CI stays reproducible. Opt-in exploration prints its concrete seed so
// even a random failure can be replayed with PORCHLIGHT_MODEL_SEED=<seed>.
fn exploration_seed() -> RngSeed {
    let Ok(value) = std::env::var("PORCHLIGHT_MODEL_SEED") else {
        return RngSeed::Fixed(2026100701);
    };
    let seed = if value == "random" {
        use std::io::Read;
        let mut bytes = [0; 8];
        std::fs::File::open("/dev/urandom")
            .unwrap()
            .read_exact(&mut bytes)
            .unwrap();
        u64::from_ne_bytes(bytes)
    } else {
        value
            .parse()
            .expect("PORCHLIGHT_MODEL_SEED must be random or a u64")
    };
    eprintln!("items_carry_model exploration seed: {seed}; replay PORCHLIGHT_MODEL_SEED={seed}");
    RngSeed::Fixed(seed)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256, rng_seed: exploration_seed(), failure_persistence: None,
        max_shrink_iters: 4096, ..ProptestConfig::default()
    })]

    #[test]
    fn r63_contract_over_operation_sequences_second_fixed_seed(
        completion in any::<bool>(), clock in -80i64..1, policy in policies(), ops in operations()
    ) {
        run(completion, clock, policy, &ops)?;
    }
}
