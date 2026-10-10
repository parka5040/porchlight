use fractional_index::FractionalIndex;
use porchlight_core::ordering::*;
use proptest::{prelude::*, test_runner::RngSeed};

fn round_trip(s: &str) {
    assert!(s.len() <= 64);
    assert_eq!(FractionalIndex::from_string(s).unwrap().to_string(), s);
}

#[test]
fn first_key_is_the_crates_default_not_the_old_db_default() {
    assert_eq!(first_key(), "80");
    assert_eq!(first_key(), FractionalIndex::default().to_string());
    assert!(FractionalIndex::from_string("a0").is_err());
    assert_eq!(key_between(None, None).unwrap(), first_key());
}

#[test]
fn optional_bounds_and_before_after_are_strict_under_byte_order() {
    let first = first_key();
    let before = key_before(&first).unwrap();
    let after = key_after(&first).unwrap();
    assert!(before.as_bytes() < first.as_bytes());
    assert!(first.as_bytes() < after.as_bytes());
    assert_eq!(key_between(None, Some(&first)).unwrap(), before);
    assert_eq!(key_between(Some(&first), None).unwrap(), after);
    let between = key_between(Some(&before), Some(&after)).unwrap();
    assert!(before < between && between < after);
    for key in [first, before, after, between] {
        round_trip(&key);
    }
}

#[test]
fn invalid_noncanonical_and_unordered_bounds_are_errors() {
    for key in [
        "", "a0", "80ff", "xyz", "7F80", "8", "é", "aé", "8é0", "801",
    ] {
        assert_eq!(key_after(key), Err(OrderingError::InvalidKey));
        assert_eq!(key_before(key), Err(OrderingError::InvalidKey));
    }
    assert_eq!(
        key_between(Some("80"), Some("80")),
        Err(OrderingError::InvalidBounds)
    );
    assert_eq!(
        key_between(Some("8180"), Some("80")),
        Err(OrderingError::InvalidBounds)
    );
}

#[test]
fn arbitrary_valid_zero_byte_prefixes_do_not_panic_in_crate_between() {
    for (a, b) in [
        ("0080", "0180"),
        ("000080", "000180"),
        ("000080", "0080"),
        ("0080", "008180"),
    ] {
        let key = key_between(Some(a), Some(b)).unwrap();
        assert!(a.as_bytes() < key.as_bytes() && key.as_bytes() < b.as_bytes());
        round_trip(&key);
    }
}

#[test]
fn insertion_stops_before_the_database_length_limit_and_rebalance_recovers() {
    let mut upper = key_after(&first_key()).unwrap();
    let lower = first_key();
    let mut count = 0;
    loop {
        match key_between(Some(&lower), Some(&upper)) {
            Ok(key) => {
                assert!(lower < key && key < upper);
                round_trip(&key);
                upper = key;
                count += 1;
                assert!(
                    count < 10000,
                    "bounded-key insert must eventually ask for rebalance"
                );
            }
            Err(OrderingError::TooLong) => break,
            err => panic!("unexpected result: {err:?}"),
        }
    }
    assert!(count > 100);
    for key in rebalance(1000) {
        round_trip(&key);
    }
    let maximal = format!("{}80", "ff".repeat(31));
    assert_eq!(maximal.len(), 64);
    assert_eq!(key_after(&maximal), Err(OrderingError::TooLong));
    assert_eq!(
        key_before(&format!("{}80", "00".repeat(31))),
        Err(OrderingError::TooLong)
    );
    assert_eq!(
        key_before(&format!("{}80", "ff".repeat(32))),
        Err(OrderingError::TooLong)
    );
}

#[test]
fn rebalance_is_evenly_spaced_and_strictly_sorted() {
    assert!(rebalance(0).is_empty());
    for n in [1, 2, 3, 255, 1000] {
        let keys = rebalance(n);
        assert_eq!(keys.len(), n);
        assert!(keys.windows(2).all(|w| w[0].as_bytes() < w[1].as_bytes()));
        let ranks: Vec<u64> = keys
            .iter()
            .map(|key| {
                round_trip(key);
                let index = FractionalIndex::from_string(key).unwrap();
                u64::from_be_bytes(index.as_bytes()[..8].try_into().unwrap())
            })
            .collect();
        let mut boundaries = vec![0];
        boundaries.extend(ranks);
        boundaries.push(u64::MAX);
        let gaps: Vec<_> = boundaries.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.iter().max().unwrap() - gaps.iter().min().unwrap() <= 1);
        assert!(key_before(&keys[0]).is_ok());
        assert!(key_after(keys.last().unwrap()).is_ok());
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        rng_seed: RngSeed::Fixed(0x532b),
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    #[test]
    fn arbitrary_insertions_preserve_byte_sort_and_round_trip(positions in prop::collection::vec(any::<usize>(), 1..300)) {
        let mut keys: Vec<String> = vec![];
        for position in positions {
            let pos = position % (keys.len() + 1);
            let key = key_between(pos.checked_sub(1).map(|i| keys[i].as_str()), keys.get(pos).map(String::as_str)).unwrap();
            if pos > 0 { prop_assert!(keys[pos - 1].as_bytes() < key.as_bytes()); }
            if pos < keys.len() { prop_assert!(key.as_bytes() < keys[pos].as_bytes()); }
            round_trip(&key);
            keys.insert(pos, key);
        }
        prop_assert!(keys.windows(2).all(|w| w[0].as_bytes() < w[1].as_bytes()));
    }

    #[test]
    fn arbitrary_strings_never_panic(a in any::<String>(), b in any::<String>()) {
        prop_assert!(std::panic::catch_unwind(|| {
            let _ = key_before(&a);
            let _ = key_after(&a);
            let _ = key_before(&b);
            let _ = key_after(&b);
            let _ = key_between(Some(&a), Some(&b));
            let _ = key_between(Some(&b), Some(&a));
            let _ = key_between(Some(&a), None);
            let _ = key_between(None, Some(&b));
        }).is_ok(), "ordering helpers must not panic on arbitrary strings");
    }

    #[test]
    fn arbitrary_valid_bounds_are_supported(mut a in prop::collection::vec(any::<u8>(), 0..20), mut b in prop::collection::vec(any::<u8>(), 0..20)) {
        a.push(128);
        b.push(128);
        let a = FractionalIndex::from_bytes(a).unwrap().to_string();
        let b = FractionalIndex::from_bytes(b).unwrap().to_string();
        if a != b {
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            let key = key_between(Some(&lo), Some(&hi)).unwrap();
            prop_assert!(lo.as_bytes() < key.as_bytes() && key.as_bytes() < hi.as_bytes());
            round_trip(&key);
        }
    }
}
