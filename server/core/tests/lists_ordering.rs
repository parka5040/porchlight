use porchlight_core::ordering;
#[test]
fn minimal_run_keeps_external_bounds() {
    for rows in [
        vec!["2080".into(), "80".into(), "80".into(), "c080".into()],
        vec![
            "2080".into(),
            exhausted_gap().0,
            exhausted_gap().1,
            "c080".into(),
        ],
    ] {
        assert!(ordering::key_between(Some(&rows[1]), Some(&rows[2])).is_err());
        let plan = ordering::insertion_run(&rows, 2).unwrap();
        assert_eq!(
            (plan.0, plan.1),
            (1, 3),
            "plan: {plan:?}; spread: {:?}",
            ordering::spread(3, Some("2080"), Some("c080"))
        );
    }
}

fn exhausted_gap() -> (String, String) {
    let lo = "80".to_string();
    let mut hi = "8180".to_string();
    while let Ok(key) = porchlight_core::ordering::key_between(Some(&lo), Some(&hi)) {
        hi = key;
    }
    (lo, hi)
}
