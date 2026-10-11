//! Direct MONTHLY/YEARLY candidates; empty periods still advance to the horizon.
use super::*;
use chrono::{Duration, Months, NaiveTime};
use std::collections::BTreeSet;

impl Rule {
    fn period_dates(
        &self,
        first: NaiveDate,
        next: NaiveDate,
        anchor: NaiveDate,
    ) -> BTreeSet<NaiveDate> {
        let mut dates = BTreeSet::new();
        if !self.by_month_day.is_empty() {
            let mut month = first;
            while month < next {
                let following = month.checked_add_months(Months::new(1)).unwrap();
                let length = (following - month).num_days();
                for &day in &self.by_month_day {
                    let offset = if day > 0 {
                        i64::from(day) - 1
                    } else {
                        length + i64::from(day)
                    };
                    if (0..length).contains(&offset) {
                        let date = month + Duration::days(offset);
                        if self.matches_date(date, anchor) {
                            dates.insert(date);
                        }
                    }
                }
                month = following;
            }
        } else if !self.by_day.is_empty() {
            let length = (next - first).num_days();
            for day in &self.by_day {
                let (ordinal, weekday) = day.split_at(day.len() - 2);
                let weekday = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"]
                    .iter()
                    .position(|d| *d == weekday)
                    .unwrap() as i64;
                let offset =
                    (weekday - i64::from(first.weekday().num_days_from_monday())).rem_euclid(7);
                if ordinal.is_empty() {
                    for offset in (offset..length).step_by(7) {
                        dates.insert(first + Duration::days(offset));
                    }
                } else {
                    let ordinal: i64 = ordinal.parse().expect("validated ordinal");
                    let offset = if ordinal > 0 {
                        offset + (ordinal - 1) * 7
                    } else {
                        offset + ((length - 1 - offset) / 7 + ordinal + 1) * 7
                    };
                    if (0..length).contains(&offset) {
                        dates.insert(first + Duration::days(offset));
                    }
                }
            }
        } else if let Some(date) = NaiveDate::from_ymd_opt(
            first.year(),
            if self.freq == "YEARLY" {
                anchor.month()
            } else {
                first.month()
            },
            anchor.day(),
        ) {
            dates.insert(date);
        }
        dates
    }
}

pub(super) fn visit(
    spec: &SeriesSpec,
    end: NaiveDateTime,
    limit: usize,
    mut callback: impl FnMut(u32, NaiveDateTime) -> Result<bool, ValidationError>,
) -> Result<bool, ValidationError> {
    let rule = spec.rule.as_ref().unwrap();
    let start = spec.start().unwrap();
    if !super::engine::within_until(spec, start) {
        return Ok(false);
    }
    if limit == 0 {
        return Ok(true);
    }
    if !callback(0, start)? {
        return Ok(false);
    }
    let mut index = 1;
    let yearly = rule.freq == "YEARLY";
    let mut first =
        NaiveDate::from_ymd_opt(start.year(), if yearly { 1 } else { start.month() }, 1).unwrap();
    let months = if yearly { 12 } else { 1 };
    let mut work = 1;
    let mut empty = 0;
    let cycle = if yearly { 400 } else { 4800 };
    while first.and_time(NaiveTime::MIN) < end && rule.count.is_none_or(|count| index < count) {
        // Charge periods as well as candidates: a sparse or empty rule cannot spin.
        if work >= limit {
            return Ok(true);
        }
        work += 1;
        let next = first
            .checked_add_months(Months::new(months))
            .ok_or_else(|| ValidationError::new("rrule", "date overflow"))?;
        empty += 1;
        for date in rule.period_dates(first, next, start.date()) {
            if work >= limit {
                return Ok(true);
            }
            work += 1;
            let local = date.and_time(start.time());
            if local <= start {
                continue;
            }
            if local >= end || !super::engine::within_until(spec, local) {
                return Ok(false);
            }
            empty = 0;
            if !callback(index, local)? {
                return Ok(false);
            }
            index = index
                .checked_add(1)
                .ok_or_else(|| ValidationError::new("seq", "sequence overflow"))?;
            if rule.count.is_some_and(|count| index >= count) {
                return Ok(false);
            }
        }
        // One extra period accounts for the initial partial period. Active periods
        // repeat within a Gregorian cycle, even when INTERVAL skips months/years.
        if empty > cycle + 1 {
            break;
        }
        first = first
            .checked_add_months(Months::new(months * u32::from(rule.interval)))
            .ok_or_else(|| ValidationError::new("rrule", "date overflow"))?;
    }
    Ok(false)
}
