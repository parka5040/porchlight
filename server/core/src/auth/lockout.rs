//! Exponential account lockout, capped at one day.

use chrono::{DateTime, Duration, Utc};

pub const LOCKOUT_THRESHOLD: i32 = 5;

/// Consecutive failures including this one; success resets the caller's counter.
pub fn locked_until(failures: i32, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if failures < LOCKOUT_THRESHOLD {
        return None;
    }
    // By exponent 11 the schedule already exceeds 24 hours. Clamp before
    // shifting or multiplying, even for i32::MAX failure counts.
    let exponent = (failures - LOCKOUT_THRESHOLD).min(11) as u32;
    let seconds = (60_i64 * (1_i64 << exponent)).min(86400);
    Some(
        now.checked_add_signed(Duration::seconds(seconds))
            .unwrap_or(DateTime::<Utc>::MAX_UTC),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Duration, Utc};

    #[test]
    fn lockout_schedule() {
        assert_eq!(LOCKOUT_THRESHOLD, 5);
        let now = DateTime::parse_from_rfc3339("2026-10-05T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        for failures in [i32::MIN, -1, 0, 1, 2, 3, 4] {
            assert_eq!(locked_until(failures, now), None);
        }
        for (failures, seconds) in [
            (5, 60),
            (6, 120),
            (7, 240),
            (15, 61440),
            (16, 86400),
            (20, 86400),
            (100, 86400),
            (i32::MAX, 86400),
        ] {
            assert_eq!(
                locked_until(failures, now),
                Some(now + Duration::seconds(seconds))
            );
        }
    }

    #[test]
    fn lockout_at_max_timestamp_does_not_overflow() {
        assert_eq!(
            locked_until(100, DateTime::<Utc>::MAX_UTC),
            Some(DateTime::<Utc>::MAX_UTC)
        );
    }
}
