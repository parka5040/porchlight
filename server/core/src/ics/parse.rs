use super::*;
use super::{expand::expand, rules::keep_series, zones::known_zone};
use icalendar::parser::{Component, Property};
fn prop<'a>(props: &'a [Property<'a>], name: &str) -> Option<&'a Property<'a>> {
    props
        .iter()
        .find(|p| p.name.as_str().eq_ignore_ascii_case(name))
}
fn value<'a>(props: &'a [Property<'a>], name: &str) -> Option<&'a str> {
    prop(props, name).map(|p| p.val.as_str())
}
fn text(props: &[Property<'_>], name: &str) -> String {
    // The crate parser already decodes known TEXT properties exactly once.
    prop(props, name)
        .map(|p| p.val.as_str().to_owned())
        .unwrap_or_default()
}
fn param<'a>(p: &'a Property<'a>, name: &str) -> Option<&'a str> {
    p.params
        .iter()
        .find(|p| p.key.as_str().eq_ignore_ascii_case(name))
        .and_then(|p| p.val.as_ref())
        .map(|s| s.as_str().trim_matches('"'))
}
pub(super) fn date(s: &str) -> Option<NaiveDate> {
    if s.len() != 8 || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    NaiveDate::parse_from_str(s, "%Y%m%d").ok()
}
pub(super) fn datetime(s: &str) -> Option<NaiveDateTime> {
    if s.len() != 15
        || s.as_bytes()[8] != b'T'
        || !s
            .bytes()
            .enumerate()
            .all(|(i, c)| i == 8 || c.is_ascii_digit())
    {
        return None;
    }
    NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%S").ok()
}
pub(super) struct Context<'a> {
    pub(super) aliases: &'a BTreeMap<String, Tz>,
    pub(super) calendar_tz: Option<Tz>,
    pub(super) options: ParseOptions,
    pub(super) uid: &'a str,
}
fn read_time(
    p: &Property<'_>,
    raw: &str,
    ctx: &Context<'_>,
    out: &mut ImportState,
) -> Option<(IcsTime, Option<Tz>)> {
    if param(p, "VALUE").is_some_and(|v| v.eq_ignore_ascii_case("DATE")) || raw.len() == 8 {
        return date(raw).map(|d| (IcsTime::Date(d), None));
    }
    if let Some(s) = raw.strip_suffix('Z') {
        return datetime(s).map(|d| {
            (
                IcsTime::Fixed(d.and_utc()),
                Some(ctx.calendar_tz.unwrap_or(chrono_tz::UTC)),
            )
        });
    }
    let local = datetime(raw)?;
    if let Some(name) = param(p, "TZID") {
        let zone = known_zone(name)
            .or_else(|| ctx.aliases.get(name).copied())
            .unwrap_or_else(|| {
                warn(
                    out,
                    Some(ctx.uid),
                    WarningCode::UnknownTimezone,
                    format!("Unknown TZID {name}; used {}", ctx.options.fallback_tz),
                );
                ctx.options.fallback_tz
            });
        Some((IcsTime::Fixed(resolve_local(local, zone)), Some(zone)))
    } else {
        Some((IcsTime::Floating(local), None))
    }
}
pub(super) fn add(t: &IcsTime, duration: Duration) -> Option<IcsTime> {
    match t {
        IcsTime::Date(d) => d.checked_add_signed(duration).map(IcsTime::Date),
        IcsTime::Floating(d) => d.checked_add_signed(duration).map(IcsTime::Floating),
        IcsTime::Fixed(d) => d.checked_add_signed(duration).map(IcsTime::Fixed),
    }
}
// DURATION values are not typed by icalendar's parser: RFC5545 nonnegative weeks,
// days and hours/minutes/seconds, in decreasing unit order (no years/months).
#[derive(Clone, Copy)]
pub(super) struct CalendarDuration {
    days: i64,
    seconds: i64,
    time_part: bool,
}
fn duration(s: &str) -> Option<CalendarDuration> {
    let s = s.strip_prefix('+').unwrap_or(s).strip_prefix('P')?;
    let mut days = 0i64;
    let mut seconds = 0i64;
    let mut number = String::new();
    let mut in_time = false;
    let mut last = 0;
    let mut units = 0;
    for c in s.chars() {
        if c.is_ascii_digit() {
            number.push(c);
            continue;
        }
        if c == 'T' && !in_time && last != 1 && number.is_empty() {
            in_time = true;
            continue;
        }
        let n = number.parse::<i64>().ok()?;
        number.clear();
        let (order, mult) = match (in_time, c) {
            (false, 'W') => (1, 7),
            (false, 'D') => (2, 1),
            (true, 'H') => (3, 3600),
            (true, 'M') => (4, 60),
            (true, 'S') => (5, 1),
            _ => return None,
        };
        if order <= last || last == 1 {
            return None;
        }
        last = order;
        units += 1;
        let amount = n.checked_mul(mult)?;
        if in_time {
            seconds = seconds.checked_add(amount)?;
        } else {
            days = days.checked_add(amount)?;
        }
    }
    if units == 0 || !number.is_empty() || (in_time && last < 3) {
        return None;
    }
    Duration::try_days(days)?;
    Duration::try_seconds(seconds)?;
    Some(CalendarDuration {
        days,
        seconds,
        time_part: in_time,
    })
}
pub(super) fn add_duration(t: &IcsTime, zone: Option<Tz>, d: CalendarDuration) -> Option<IcsTime> {
    let nominal = Duration::try_days(d.days)?;
    let next = match (t, zone) {
        (IcsTime::Fixed(t), Some(tz)) if d.days != 0 => IcsTime::Fixed(resolve_local(
            to_local(*t, tz).checked_add_signed(nominal)?,
            tz,
        )),
        _ => add(t, nominal)?,
    };
    add(&next, Duration::try_seconds(d.seconds)?)
}

/// Parse a calendar atomically at the syntax level; semantic errors skip only the
/// bad event. Malformed syntax returns no events and a warning, never partial data.
/// Limit input to 8 MiB and component depth to 16 before invoking the crate parser.
/// Custom VTIMEZONE transitions are not synthesized: use IANA/Windows/X-LIC-LOCATION
/// or fallback with a warning. Cancelled overrides may use RECURRENCE-ID as DTSTART.
pub fn parse(input: &str, options: ParseOptions) -> ParsedCalendar {
    let mut out = ImportState::default();
    if input.len() > 8 * 1024 * 1024 {
        warn(
            &mut out,
            None,
            WarningCode::MalformedCalendar,
            "Calendar exceeds 8 MiB",
        );
        return out.finish();
    }
    let input = input.strip_prefix('﻿').unwrap_or(input);
    let unfolded = icalendar::parser::unfold(input.trim_end());
    // The crate tries take_until(CRLF) before LF for every property. An LF-only
    // large feed otherwise scans the entire remaining file on each property.
    let unfolded = unfolded.replace("\r\n", "\n").replace('\n', "\r\n");
    let mut depth = 0usize;
    for line in unfolded.lines() {
        // The crate can ignore a bare line inside a component. RFC content lines
        // require a colon; reject this syntax error atomically, not as a bad event.
        if !line.trim().is_empty() && !line.contains(':') {
            warn(
                &mut out,
                None,
                WarningCode::MalformedCalendar,
                "Content line missing colon",
            );
            return out.finish();
        }
        if line
            .get(..6)
            .is_some_and(|s| s.eq_ignore_ascii_case("BEGIN:"))
        {
            depth += 1;
            if depth > 16 {
                warn(
                    &mut out,
                    None,
                    WarningCode::MalformedCalendar,
                    "Component depth exceeds 16",
                );
                return out.finish();
            }
        } else if line
            .get(..4)
            .is_some_and(|s| s.eq_ignore_ascii_case("END:"))
        {
            depth = depth.saturating_sub(1);
        }
    }
    let roots = match icalendar::parser::read_calendar_simple(&unfolded) {
        Ok(r) => r,
        Err(_) => {
            warn(
                &mut out,
                None,
                WarningCode::MalformedCalendar,
                "Malformed calendar syntax",
            );
            return out.finish();
        }
    };
    // The crate accepts a valid prefix and ignores trailing garbage. Reject it
    // conservatively by requiring a single, closed VCALENDAR wrapper.
    let meaningful: Vec<_> = unfolded.lines().filter(|s| !s.trim().is_empty()).collect();
    if roots.len() != 1
        || !roots[0].name.as_str().eq_ignore_ascii_case("VCALENDAR")
        || depth != 0
        || !meaningful
            .last()
            .is_some_and(|s| s.trim_end().eq_ignore_ascii_case("END:VCALENDAR"))
    {
        warn(
            &mut out,
            None,
            WarningCode::MalformedCalendar,
            "Expected one complete VCALENDAR",
        );
        return out.finish();
    }
    let root = &roots[0];
    let mut aliases = BTreeMap::new();
    for c in &root.components {
        if c.name.as_str().eq_ignore_ascii_case("VTIMEZONE")
            && let (Some(id), Some(location)) = (
                value(&c.properties, "TZID"),
                value(&c.properties, "X-LIC-LOCATION"),
            )
            && let Some(tz) = known_zone(location)
        {
            aliases.insert(id.to_owned(), tz);
        }
    }
    let calendar_tz = value(&root.properties, "X-WR-TIMEZONE").and_then(|name| {
        let zone = known_zone(name).or_else(|| aliases.get(name).copied());
        if zone.is_none() {
            warn(
                &mut out,
                None,
                WarningCode::UnknownTimezone,
                "Invalid X-WR-TIMEZONE; UTC retained for Z times",
            );
        }
        zone
    });
    let mut masters = BTreeSet::new();
    let mut source_events = Vec::new();
    let mut future_replacements = BTreeMap::<String, IcsTime>::new();
    for c in &root.components {
        if !c.name.as_str().eq_ignore_ascii_case("VEVENT") {
            continue;
        }
        let has_standalone_identity = matches!(
            value(&c.properties, "X-PORCHLIGHT-ORIGIN"),
            Some("EXPANDED" | "OVERRIDE")
        ) && prop(&c.properties, "X-PORCHLIGHT-INSTANCE").is_some();
        let uid = if has_standalone_identity && prop(&c.properties, "X-PORCHLIGHT-UID").is_some() {
            text(&c.properties, "X-PORCHLIGHT-UID")
        } else {
            text(&c.properties, "UID")
        };
        // Extension emitted only for a later-fold UTC DTSTART. It preserves the
        // display zone without changing the unambiguous instant on the wire.
        let event_tz = value(&c.properties, "X-PORCHLIGHT-TZID")
            .and_then(known_zone)
            .or(calendar_tz);
        let ctx = Context {
            aliases: &aliases,
            calendar_tz: event_tz,
            options,
            uid: &uid,
        };
        match read_event(c, &ctx, &mut out) {
            Some(e) => {
                if e.recurrence_id.is_none() && !masters.insert(e.uid.clone()) {
                    warn(
                        &mut out,
                        Some(&uid),
                        WarningCode::InvalidEvent,
                        "Duplicate UID master; kept and expanded representations may both survive; importer must review",
                    );
                }
                if e.origin == EventOrigin::Override
                    && e.rrule.is_some()
                    && prop(&c.properties, "RECURRENCE-ID")
                        .and_then(|p| param(p, "RANGE"))
                        .is_some_and(|s| s.eq_ignore_ascii_case("THISANDFUTURE"))
                    && let Some(rid) = &e.recurrence_id
                {
                    future_replacements
                        .entry(uid.clone())
                        .and_modify(|old| {
                            if rid < old {
                                *old = rid.clone();
                            }
                        })
                        .or_insert_with(|| rid.clone());
                }
                let nominal = if prop(&c.properties, "DTEND").is_none() {
                    value(&c.properties, "DURATION").and_then(duration)
                } else {
                    None
                };
                source_events.push((e, nominal, event_tz));
            }
            None => warn(
                &mut out,
                Some(&uid),
                WarningCode::InvalidEvent,
                "Event missing UID/time, invalid duration/end/sequence, or mismatched time forms",
            ),
        }
    }
    // Decide RANGE replacements only after every source override is known.
    for (mut e, nominal, event_tz) in source_events {
        let uid = e.uid.clone();
        let ctx = Context {
            aliases: &aliases,
            calendar_tz: event_tz,
            options,
            uid: &uid,
        };
        if let Some(rule) = e.rrule.take() {
            if e.origin == EventOrigin::Override {
                warn(
                    &mut out,
                    Some(&uid),
                    WarningCode::UnsupportedProperty,
                    "Override RRULE: original RECURRENCE-ID retained; further rule starts expanded; RANGE=THISANDFUTURE supersedes the master's future",
                );
                expand(e.clone(), &rule, nominal, None, &ctx, &mut out);
                out.events.push(e);
            } else {
                let cutoff = future_replacements.get(&uid);
                if cutoff.is_none()
                    && let Some(normal) = keep_series(&e, &rule)
                {
                    e.rrule = Some(normal);
                    e.origin = EventOrigin::Series;
                    out.events.push(e);
                } else {
                    expand(e, &rule, nominal, cutoff, &ctx, &mut out);
                }
            }
        } else {
            out.events.push(e);
        }
    }
    reconcile(&mut out, options.fallback_tz);
    out.finish()
}
fn read_event(
    c: &Component<'_>,
    ctx: &Context<'_>,
    out: &mut ImportState,
) -> Option<ImportedEvent> {
    let p = &c.properties;
    if ctx.uid.is_empty() {
        return None;
    }
    let status = match value(p, "STATUS")
        .unwrap_or("CONFIRMED")
        .to_ascii_uppercase()
        .as_str()
    {
        "CANCELLED" => EventStatus::Cancelled,
        "TENTATIVE" => EventStatus::Tentative,
        _ => EventStatus::Confirmed,
    };
    let identity = prop(p, "RECURRENCE-ID").or_else(|| {
        matches!(
            value(p, "X-PORCHLIGHT-ORIGIN"),
            Some("EXPANDED" | "OVERRIDE")
        )
        .then(|| prop(p, "X-PORCHLIGHT-INSTANCE"))
        .flatten()
    });
    let rid = match identity {
        Some(p) => Some(read_time(p, p.val.as_str(), ctx, out)?.0),
        None => None,
    };
    let start_prop = prop(p, "DTSTART").or_else(|| {
        if status == EventStatus::Cancelled {
            prop(p, "RECURRENCE-ID")
        } else {
            None
        }
    })?;
    let (start, zone) = read_time(start_prop, start_prop.val.as_str(), ctx, out)?;
    let all_day = matches!(start, IcsTime::Date(_));
    let mut end = if let Some(p) = prop(p, "DTEND") {
        read_time(p, p.val.as_str(), ctx, out)?.0
    } else if let Some(s) = value(p, "DURATION") {
        let d = duration(s)?;
        if all_day && d.time_part {
            return None;
        }
        add_duration(&start, zone, d)?
    } else {
        add(&start, Duration::days(i64::from(all_day)))?
    };
    if all_day && end == start {
        end = add(&start, Duration::days(1))?;
        warn(
            out,
            Some(ctx.uid),
            WarningCode::InvalidEvent,
            "DATE end equals start; salvaged as one day",
        );
    }
    let valid = match (&start, &end) {
        (IcsTime::Date(a), IcsTime::Date(b)) => a < b,
        (IcsTime::Floating(a), IcsTime::Floating(b)) => a <= b,
        (IcsTime::Fixed(a), IcsTime::Fixed(b)) => a <= b,
        _ => false,
    };
    if !valid {
        return None;
    }
    let mut exdates = Vec::new();
    for p in p
        .iter()
        .filter(|p| p.name.as_str().eq_ignore_ascii_case("EXDATE"))
    {
        for raw in p.val.as_str().split(',') {
            if let Some((t, _)) = read_time(p, raw, ctx, out) {
                exdates.push(t);
            } else {
                warn(
                    out,
                    Some(ctx.uid),
                    WarningCode::InvalidEvent,
                    "Invalid EXDATE ignored",
                );
            }
        }
    }
    for name in ["RDATE", "EXRULE", "RANGE"] {
        if prop(p, name).is_some()
            || (name == "RANGE"
                && prop(p, "RECURRENCE-ID")
                    .and_then(|p| param(p, "RANGE"))
                    .is_some())
        {
            warn(
                out,
                Some(ctx.uid),
                WarningCode::UnsupportedProperty,
                format!("{name} is not represented; importer must review"),
            );
        }
    }
    let sequence = match value(p, "SEQUENCE") {
        None => 0,
        Some(s) => s.parse().unwrap_or_else(|_| {
            warn(
                out,
                Some(ctx.uid),
                WarningCode::InvalidEvent,
                "Invalid SEQUENCE; used 0",
            );
            0
        }),
    };
    let origin = if let Some(rid) = &rid {
        if value(p, "X-PORCHLIGHT-ORIGIN") == Some("EXPANDED") {
            EventOrigin::Expanded {
                rule_instance_start: rid.clone(),
            }
        } else {
            EventOrigin::Override
        }
    } else {
        EventOrigin::Single
    };
    Some(ImportedEvent {
        uid: ctx.uid.to_owned(),
        recurrence_id: rid,
        origin,
        summary: text(p, "SUMMARY"),
        description: text(p, "DESCRIPTION"),
        location: text(p, "LOCATION"),
        time_mode: if matches!(start, IcsTime::Fixed(_)) {
            TimeMode::Fixed
        } else {
            TimeMode::Floating
        },
        tzid: zone.map(|t| t.to_string()),
        start,
        end,
        all_day,
        rrule: value(p, "RRULE").map(str::to_owned),
        exdates,
        sequence,
        status,
    })
}
// Resolve overrides only after all source events are read: file order is irrelevant.
fn reconcile(out: &mut ImportState, fallback: Tz) {
    let overrides: Vec<_> = out
        .events
        .iter()
        .filter(|e| e.origin == EventOrigin::Override)
        .filter_map(|e| e.recurrence_id.clone().map(|rid| (e.uid.clone(), rid)))
        .collect();
    out.events.retain(|e| {
        let EventOrigin::Expanded {
            rule_instance_start,
        } = &e.origin
        else {
            return true;
        };
        let tz = e.tzid.as_deref().and_then(parse_tz).unwrap_or(fallback);
        !overrides.iter().any(|(uid, rid)| {
            uid == &e.uid && super::expand::matches_instance(rid, rule_instance_start, tz)
        })
    });
    // Same key cannot leave parse twice. Higher SEQUENCE wins; equal sequence keeps
    // the first source record. Warn so ambiguous source duplicates are not silent.
    let mut seen = BTreeMap::new();
    let mut events: Vec<ImportedEvent> = Vec::new();
    for e in std::mem::take(&mut out.events) {
        let key = (e.uid.clone(), e.recurrence_id.clone());
        if let Some(&index) = seen.get(&key) {
            let previous: &ImportedEvent = &events[index];
            warn(
                out,
                Some(&e.uid),
                WarningCode::InvalidEvent,
                "Duplicate UID/RECURRENCE-ID; highest SEQUENCE retained (first on ties)",
            );
            if e.sequence > previous.sequence {
                events[index] = e;
            }
        } else {
            seen.insert(key, events.len());
            events.push(e);
        }
    }
    out.events = events;
}
