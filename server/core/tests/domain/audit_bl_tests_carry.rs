use super::*;

#[test]
fn zero_overdue_invariant_rejects_protected_work_without_a_carrier() {
    for close_younger in [false, true] {
        let mut m = Model::new(&[0, 24], -1, Policy::CarryOver, false);
        assert_eq!(m.start().unwrap(), [OP, OP]);
        assert_eq!(m.step(Op::Move(0, 12)), [OP, OP]);
        assert_eq!(m.step(Op::Move(1, 48)), [OP, OP]);
        assert_eq!(m.step(Op::Advance(31)), [OV, OP]);
        if close_younger {
            assert_eq!(m.step(Op::Complete(1)), [OV, OP]);
        }
        // The older postponed row is protected even when the younger row closes.
        // Exercise the independent invariant, not the engine-versus-model equality.
        m.rows[0].persisted = OP;
        assert_eq!(
            m.invariants(None, None).unwrap_err(),
            "(1) missed 0 is uncovered and nothing is overdue"
        );
    }
}
