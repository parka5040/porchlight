//! Fractional Kanban/list keys using the crate's canonical string representation.
//!
//! Compare keys byte-wise (PostgreSQL `COLLATE "C"`), not using locale collation. Keys are
//! bounded to 64 ASCII bytes; on exhaustion the write layer must rebalance its ordered collection
//! in one transaction. These helpers do not provide concurrency control or uniqueness by themselves.
#![deny(missing_docs)]

use fractional_index::FractionalIndex;

/// Maximum ASCII byte length allowed by the database position CHECK.
pub const MAX_KEY_BYTES: usize = 64;

/// Invalid ordering inputs or a key that needs collection rebalancing.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum OrderingError {
    /// Not the canonical lowercase string emitted by `FractionalIndex::to_string`.
    #[error("invalid fractional ordering key")]
    InvalidKey,
    /// Lower and upper bounds are equal or reversed.
    #[error("fractional ordering bounds must be strictly increasing")]
    InvalidBounds,
    /// An input or generated key exceeds the SQL length limit; rebalance first.
    #[error("fractional ordering key exceeds 64 bytes; rebalance the collection")]
    TooLong,
}

fn parse(key: &str) -> Result<FractionalIndex, OrderingError> {
    if key.len() > MAX_KEY_BYTES {
        return Err(OrderingError::TooLong);
    }
    // The crate slices hex by byte pairs: Unicode can panic and an odd trailing nibble
    // is silently discarded ("801" becomes "80"). Guard both before calling it.
    if !key.len().is_multiple_of(2)
        || !key
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(OrderingError::InvalidKey);
    }
    let index = FractionalIndex::from_string(key).map_err(|_| OrderingError::InvalidKey)?;
    // Retain a canonical round-trip check so parsed order matches byte-wise database order.
    // Legacy "a0" is not a terminated crate index.
    if index.to_string() != key {
        return Err(OrderingError::InvalidKey);
    }
    Ok(index)
}

fn bounded(index: FractionalIndex) -> Result<String, OrderingError> {
    let key = index.to_string();
    if key.len() > MAX_KEY_BYTES {
        return Err(OrderingError::TooLong);
    }
    Ok(key)
}

/// The crate's default/first key (`"80"`), to use for SQL column defaults instead of legacy `"a0"`.
pub fn first_key() -> String {
    FractionalIndex::default().to_string()
}

/// A canonical key strictly before `key`, or an error (including 64-byte exhaustion).
pub fn key_before(key: &str) -> Result<String, OrderingError> {
    bounded(FractionalIndex::new_before(&parse(key)?))
}

/// A canonical key strictly after `key`, or an error (including 64-byte exhaustion).
pub fn key_after(key: &str) -> Result<String, OrderingError> {
    bounded(FractionalIndex::new_after(&parse(key)?))
}

/// A canonical key between optional bounds; neither bound means [`first_key`].
///
/// With one bound, generates before/after it; with two, requires strictly increasing bounds.
/// Returns [`OrderingError::TooLong`] rather than ever exceeding the database's 64-byte limit.
pub fn key_between(a: Option<&str>, b: Option<&str>) -> Result<String, OrderingError> {
    let a = a.map(parse).transpose()?;
    let b = b.map(parse).transpose()?;
    match (a, b) {
        (None, None) => Ok(first_key()),
        (Some(a), None) => bounded(FractionalIndex::new_after(&a)),
        (None, Some(b)) => bounded(FractionalIndex::new_before(&b)),
        (Some(a), Some(b)) => {
            if a >= b {
                return Err(OrderingError::InvalidBounds);
            }
            // fractional_index 2.0.2 subtracts 1 from each compared upper-bound byte;
            // equal leading 0x00 bytes would underflow in debug builds. Strip the common
            // non-terminating prefix, ask the crate for a suffix index, then restore it.
            // This is order-preserving and also keeps results valid for arbitrary parsed keys.
            let max_prefix = a.as_bytes().len().min(b.as_bytes().len()) - 1;
            let prefix_len = a
                .as_bytes()
                .iter()
                .zip(b.as_bytes())
                .take(max_prefix)
                .take_while(|(left, right)| left == right)
                .count();
            let a_suffix = FractionalIndex::from_bytes(a.as_bytes()[prefix_len..].to_vec())
                .expect("suffix retains the terminator");
            let b_suffix = FractionalIndex::from_bytes(b.as_bytes()[prefix_len..].to_vec())
                .expect("suffix retains the terminator");
            let suffix = FractionalIndex::new_between(&a_suffix, &b_suffix)
                .ok_or(OrderingError::InvalidBounds)?;
            let mut bytes = a.as_bytes()[..prefix_len].to_vec();
            bytes.extend_from_slice(suffix.as_bytes());
            bounded(
                FractionalIndex::from_bytes(bytes).expect("generated key retains the terminator"),
            )
        }
    }
}

/// Balanced fractional keys inside external bounds (S3's minimal-run spread algorithm).
pub fn spread(n: usize, lo: Option<&str>, hi: Option<&str>) -> Result<Vec<String>, OrderingError> {
    if n == 0 {
        return Ok(Vec::new());
    }
    let key = key_between(lo, hi)?;
    let mid = n / 2;
    let mut keys = spread(mid, lo, Some(&key))?;
    let rest = spread(n - mid - 1, Some(&key), hi)?;
    keys.push(key);
    keys.extend(rest);
    Ok(keys)
}

/// Minimal tied/exhausted run around an insertion gap. Returns start, exclusive end,
/// and keys for that run with the new element inserted at `gap - start`.
/// Expands only when the external bounds cannot accommodate the run.
pub fn insertion_run(
    rows: &[String],
    gap: usize,
) -> Result<(usize, usize, Vec<String>), OrderingError> {
    if gap > rows.len() {
        return Err(OrderingError::InvalidBounds);
    }
    let mut start = gap.saturating_sub(1);
    let mut end = (gap + 1).min(rows.len());
    loop {
        while start > 0 && rows[start - 1] == rows[start] {
            start -= 1;
        }
        while end < rows.len() && rows[end - 1] == rows[end] {
            end += 1;
        }
        if let Ok(keys) = spread(
            end - start + 1,
            start.checked_sub(1).map(|i| rows[i].as_str()),
            rows.get(end).map(String::as_str),
        ) {
            return Ok((start, end, keys));
        }
        if start == 0 && end == rows.len() {
            return Err(OrderingError::TooLong);
        }
        start = start.saturating_sub(1);
        end = (end + 1).min(rows.len());
    }
}

/// `n` sorted, evenly spaced keys, each a fixed-width 64-bit rank plus the crate terminator.
///
/// Ranks divide the interval `(0, u64::MAX)` into `n + 1` gaps differing by at most one.
/// Every key is 18 ASCII bytes with room for insertion on either side. The caller associates
/// these keys with the collection's existing order and persists the reassignment atomically.
pub fn rebalance(n: usize) -> Vec<String> {
    let denominator = n as u128 + 1;
    (0..n)
        .map(|i| {
            let rank = (((i as u128 + 1) * u128::from(u64::MAX)) / denominator) as u64;
            let mut bytes = rank.to_be_bytes().to_vec();
            bytes.push(128); // FractionalIndex's stable terminating byte.
            FractionalIndex::from_bytes(bytes)
                .expect("rank key has a terminator")
                .to_string()
        })
        .collect()
}
