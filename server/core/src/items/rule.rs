use super::ValidationError;
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt, str::FromStr};

/// Inclusive recurrence bound. Date bounds include the entire local day.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Until {
    /// Inclusive nominal local date.
    Date(NaiveDate),
    /// Inclusive UTC instant (synthetic UTC for floating/date-only items).
    Utc(DateTime<Utc>),
}

/// Validated RRULE subset, serialized as a canonical DB-compatible string.
/// Parts are ordered FREQ, INTERVAL, BYDAY, BYMONTHDAY, COUNT, UNTIL;
/// default INTERVAL=1 is omitted and filter lists are sorted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Rule {
    pub(super) freq: String,
    pub(super) interval: u16,
    pub(super) by_day: Vec<String>,
    pub(super) by_month_day: Vec<i8>,
    pub(super) count: Option<u32>,
    pub(super) until: Option<Until>,
}

impl Rule {
    /// Frequency keyword (DAILY, WEEKLY, MONTHLY or YEARLY).
    pub fn frequency(&self) -> &str {
        &self.freq
    }
    /// Frequency multiplier, 1..=999.
    pub fn interval(&self) -> u16 {
        self.interval
    }
    /// Total instance count including DTSTART, if bounded by COUNT.
    pub fn count(&self) -> Option<u32> {
        self.count
    }
    /// Inclusive bound, if bounded by UNTIL.
    pub fn until(&self) -> Option<&Until> {
        self.until.as_ref()
    }
    // Nonempty candidate superset: filter outside the library iterator so it can
    // stop at the horizon even if the selected active periods contain no matches.
    pub(super) fn scan_pattern(&self) -> String {
        let mut rule = self.clone();
        rule.count = None;
        rule.until = None;
        if rule.freq != "WEEKLY" {
            rule.by_day.clear();
            rule.by_month_day = if matches!(rule.freq.as_str(), "MONTHLY" | "YEARLY") {
                (1..=31).collect()
            } else {
                vec![]
            };
        }
        rule.to_string()
    }
    pub(super) fn matches_date(&self, date: NaiveDate, dtstart: NaiveDate) -> bool {
        let month_len = days_in_month(date);
        let month_day_matches = self.by_month_day.is_empty()
            || self.by_month_day.iter().any(|&d| {
                let wanted = if d > 0 {
                    i32::from(d)
                } else {
                    month_len + i32::from(d) + 1
                };
                date.day() as i32 == wanted
            });
        if !month_day_matches {
            return false;
        }
        if !self.by_day.is_empty() {
            return self.by_day.iter().any(|day| self.matches_by_day(day, date));
        }
        if !self.by_month_day.is_empty() {
            return true;
        }
        match self.freq.as_str() {
            "MONTHLY" => date.day() == dtstart.day(),
            "YEARLY" => date.month() == dtstart.month() && date.day() == dtstart.day(),
            _ => true, // DAILY and WEEKLY defaults already handled by the iterator.
        }
    }
    fn matches_by_day(&self, day: &str, date: NaiveDate) -> bool {
        let (ordinal, weekday) = day.split_at(day.len() - 2);
        let actual = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"]
            [date.weekday().num_days_from_monday() as usize];
        if weekday != actual {
            return false;
        }
        if ordinal.is_empty() {
            return true;
        }
        let ordinal: i32 = ordinal.parse().expect("validated ordinal");
        let (position, length) = if self.freq == "YEARLY" {
            (
                date.ordinal() as i32,
                NaiveDate::from_ymd_opt(date.year(), 12, 31)
                    .unwrap()
                    .ordinal() as i32,
            )
        } else {
            (date.day() as i32, days_in_month(date))
        };
        if ordinal > 0 {
            (position - 1) / 7 + 1 == ordinal
        } else {
            -((length - position) / 7 + 1) == ordinal
        }
    }
    fn ordinal_filters_satisfiable(&self) -> bool {
        if self.by_month_day.is_empty()
            || self.by_day.is_empty()
            || self.by_day.iter().any(|day| day.len() == 2)
        {
            return true;
        }
        // Gregorian weekdays/month lengths repeat every 400 years. Checking selected
        // month days over one cycle proves impossibility without rejecting valid unions,
        // negative month days, or YEARLY (whose ordinals are year-wide, not monthly).
        for year in 2000..2400 {
            for month in 1..=12 {
                let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
                let length = days_in_month(first);
                for &day in &self.by_month_day {
                    let d = if day > 0 {
                        i32::from(day)
                    } else {
                        length + i32::from(day) + 1
                    };
                    if d < 1 || d > length {
                        continue;
                    }
                    let date = NaiveDate::from_ymd_opt(year, month, d as u32).unwrap();
                    if self.by_day.iter().any(|day| self.matches_by_day(day, date)) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

fn days_in_month(date: NaiveDate) -> i32 {
    // No next-year conversion: remains valid in the final supported year.
    for day in (28..=31).rev() {
        if NaiveDate::from_ymd_opt(date.year(), date.month(), day).is_some() {
            return day as i32;
        }
    }
    unreachable!("every month has at least 28 days")
}

fn error(reason: impl Into<String>) -> ValidationError {
    ValidationError::new("rrule", reason)
}
fn positive(s: &str, digits: usize) -> bool {
    !s.is_empty()
        && s.len() <= digits
        && !s.starts_with('0')
        && s.bytes().all(|b| b.is_ascii_digit())
}

impl FromStr for Rule {
    type Err = ValidationError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split(';');
        let first = parts.next().ok_or_else(|| error("missing FREQ"))?;
        let freq = first
            .strip_prefix("FREQ=")
            .filter(|f| matches!(*f, "DAILY" | "WEEKLY" | "MONTHLY" | "YEARLY"))
            .ok_or_else(|| error("FREQ must be first and DAILY, WEEKLY, MONTHLY or YEARLY"))?;
        let mut rule = Rule {
            freq: freq.into(),
            interval: 1,
            by_day: vec![],
            by_month_day: vec![],
            count: None,
            until: None,
        };
        let mut seen = BTreeSet::new();
        seen.insert("FREQ");
        for part in parts {
            let (key, value) = part
                .split_once('=')
                .ok_or_else(|| error("malformed part"))?;
            if !seen.insert(key) {
                return Err(error(format!("duplicate {key}")));
            }
            match key {
                "INTERVAL" if positive(value, 3) => {
                    rule.interval = value.parse().map_err(|_| error("invalid INTERVAL"))?
                }
                "COUNT" if positive(value, 4) => {
                    rule.count = Some(value.parse().map_err(|_| error("invalid COUNT"))?)
                }
                "BYDAY" => {
                    let mut days = BTreeSet::new();
                    for day in value.split(',') {
                        if !day.is_ascii() || day.len() < 2 {
                            return Err(error("invalid BYDAY"));
                        }
                        let (ordinal, weekday) = day.split_at(day.len() - 2);
                        if !matches!(weekday, "MO" | "TU" | "WE" | "TH" | "FR" | "SA" | "SU") {
                            return Err(error("invalid weekday"));
                        }
                        let normalized = if ordinal.is_empty() {
                            weekday.to_owned()
                        } else {
                            let number = ordinal.strip_prefix(['+', '-']).unwrap_or(ordinal);
                            if number.len() != 1
                                || !matches!(number, "1" | "2" | "3" | "4" | "5")
                                || !matches!(freq, "MONTHLY" | "YEARLY")
                            {
                                return Err(error(
                                    "BYDAY ordinals require MONTHLY/YEARLY and +/-1..5",
                                ));
                            }
                            format!(
                                "{}{}{}",
                                if ordinal.starts_with('-') { "-" } else { "" },
                                number,
                                weekday
                            )
                        };
                        if !days.insert(normalized) {
                            return Err(error("duplicate BYDAY value"));
                        }
                    }
                    rule.by_day = days.into_iter().collect();
                }
                "BYMONTHDAY" => {
                    if !matches!(freq, "MONTHLY" | "YEARLY") {
                        return Err(error("BYMONTHDAY requires MONTHLY/YEARLY"));
                    }
                    let mut days = BTreeSet::new();
                    for day in value.split(',') {
                        let abs = day.strip_prefix('-').unwrap_or(day);
                        if !positive(abs, 2) {
                            return Err(error("BYMONTHDAY must be -31..-1 or 1..31"));
                        }
                        let num: i8 = day.parse().map_err(|_| error("invalid BYMONTHDAY"))?;
                        if num == 0 || num.abs() > 31 || !days.insert(num) {
                            return Err(error("invalid or duplicate BYMONTHDAY value"));
                        }
                    }
                    rule.by_month_day = days.into_iter().collect();
                }
                "UNTIL" => {
                    if !value.is_ascii() {
                        return Err(error("invalid UNTIL"));
                    }
                    let bound = match value.len() {
                        8 if value.bytes().all(|c| c.is_ascii_digit()) => {
                            let d = NaiveDate::parse_from_str(value, "%Y%m%d")
                                .map_err(|_| error("invalid UNTIL date"))?;
                            if d.year() < 1 {
                                return Err(error("UNTIL year must be 0001..9999"));
                            }
                            Until::Date(d)
                        }
                        16 if value.as_bytes()[8] == b'T'
                            && value.ends_with('Z')
                            && value[..8]
                                .bytes()
                                .chain(value[9..15].bytes())
                                .all(|c| c.is_ascii_digit()) =>
                        {
                            let t = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ")
                                .map_err(|_| error("invalid UNTIL UTC datetime"))?;
                            if t.year() < 1 || t.nanosecond() != 0 {
                                return Err(error("invalid UNTIL year or leap second"));
                            }
                            Until::Utc(t.and_utc())
                        }
                        _ => return Err(error("UNTIL must be YYYYMMDD or YYYYMMDDTHHMMSSZ")),
                    };
                    rule.until = Some(bound);
                }
                _ => return Err(error(format!("unsupported or invalid {key}"))),
            }
        }
        if rule.count.is_some() && rule.until.is_some() {
            return Err(error("COUNT and UNTIL are mutually exclusive"));
        }
        if !rule.ordinal_filters_satisfiable() {
            return Err(error("ordinal BYDAY and BYMONTHDAY cannot select any date"));
        }
        Ok(rule)
    }
}
impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FREQ={}", self.freq)?;
        if self.interval != 1 {
            write!(f, ";INTERVAL={}", self.interval)?;
        }
        if !self.by_day.is_empty() {
            write!(f, ";BYDAY={}", self.by_day.join(","))?;
        }
        if !self.by_month_day.is_empty() {
            write!(
                f,
                ";BYMONTHDAY={}",
                self.by_month_day
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )?;
        }
        if let Some(count) = self.count {
            write!(f, ";COUNT={count}")?;
        }
        if let Some(until) = &self.until {
            match until {
                Until::Date(d) => write!(f, ";UNTIL={}", d.format("%Y%m%d"))?,
                Until::Utc(t) => write!(f, ";UNTIL={}", t.format("%Y%m%dT%H%M%SZ"))?,
            }
        }
        Ok(())
    }
}
impl TryFrom<String> for Rule {
    type Error = ValidationError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}
impl From<Rule> for String {
    fn from(rule: Rule) -> Self {
        rule.to_string()
    }
}
