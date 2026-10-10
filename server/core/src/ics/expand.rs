use super::parse::{CalendarDuration, Context, add, add_duration, date, datetime};
use super::*;

fn local(t: &IcsTime, tz: Tz) -> NaiveDateTime {
    match t {
        IcsTime::Date(d) => d.and_hms_opt(0, 0, 0).expect("midnight"),
        IcsTime::Floating(d) => *d,
        IcsTime::Fixed(d) => to_local(*d, tz),
    }
}
fn instant(t: &IcsTime, tz: Tz) -> DateTime<Utc> {
    match t {
        IcsTime::Fixed(t) => *t,
        _ => resolve_local(local(t, tz), tz),
    }
}
/// DATE exclusions match the local date; floating exclusions match local wall time;
/// fixed exclusions match the actual instant (including explicit later-fold IDs).
pub(super) fn matches_instance(exclusion: &IcsTime, start: &IcsTime, tz: Tz) -> bool {
    match exclusion {
        IcsTime::Date(d) => *d == local(start, tz).date(),
        IcsTime::Floating(d) => *d == local(start, tz),
        IcsTime::Fixed(d) => *d == instant(start, tz),
    }
}

pub(super) fn expand(
    event: ImportedEvent,
    rule: &str,
    nominal: Option<CalendarDuration>,
    superseded_from: Option<&IcsTime>,
    ctx: &Context<'_>,
    out: &mut ImportState,
) {
    if out.scan_exhausted() {
        warn(
            out,
            Some(ctx.uid),
            WarningCode::ExpansionLimited,
            "Parse-wide RRULE scan budget exhausted; remaining unsupported rules omitted",
        );
        return;
    }
    let is_override = event.origin == EventOrigin::Override;
    let tz = event
        .tzid
        .as_deref()
        .and_then(parse_tz)
        .unwrap_or(ctx.options.fallback_tz);
    let anchor = local(&event.start, tz);
    // Synthetic UTC wall times keep core::time's shared fold/gap policy.
    let mut rule = rule.trim().to_ascii_uppercase();
    let instant_until = rule
        .split(';')
        .find_map(|s| {
            s.strip_prefix("UNTIL=")
                .and_then(|s| s.strip_suffix('Z'))
                .and_then(datetime)
        })
        .map(|d| d.and_utc());
    if let Some(until) = rule.split(';').find_map(|s| s.strip_prefix("UNTIL=")) {
        let replacement = if let Some(s) = until.strip_suffix('Z') {
            datetime(s).map(|d| {
                to_local(d.and_utc(), tz)
                    .checked_add_signed(Duration::days(2))
                    .unwrap_or(NaiveDateTime::MAX)
                    .format("%Y%m%dT%H%M%SZ")
                    .to_string()
            })
        } else if let Some(d) = date(until) {
            d.and_hms_opt(23, 59, 59)
                .map(|d| d.format("%Y%m%dT%H%M%SZ").to_string())
        } else {
            datetime(until).map(|d| d.format("%Y%m%dT%H%M%SZ").to_string())
        };
        if let Some(r) = replacement {
            rule = rule.replace(&format!("UNTIL={until}"), &format!("UNTIL={r}"));
        }
    }
    let source = |rule: &str| format!("DTSTART:{}Z\nRRULE:{rule}", anchor.format("%Y%m%dT%H%M%S"));
    let mut set = match source(&rule).parse::<rrule::RRuleSet>() {
        Ok(s) => s,
        Err(_) => {
            warn(
                out,
                Some(ctx.uid),
                WarningCode::InvalidRule,
                "RRULE cannot be expanded; event retained without recurrence",
            );
            if !is_override {
                out.events.push(event);
            }
            return;
        }
    };
    warn(
        out,
        Some(ctx.uid),
        WarningCode::ExpandedRule,
        "Unsupported or invalid series RRULE expanded to discrete events",
    );
    if ctx.options.horizon_start >= ctx.options.horizon_end {
        return;
    }
    // Validate UNTIL above, then enforce it ourselves so the iterator can yield
    // one date beyond the bound. Natural UNTIL exhaustion is then distinguishable
    // from rrule's private iteration-limit flag, without a collecting tail rescan.
    let until_wall = set
        .get_rrule()
        .first()
        .and_then(|r| r.get_until())
        .map(|t| t.naive_utc());
    if until_wall.is_some() {
        let without_until = rule
            .split(';')
            .filter(|part| !part.starts_with("UNTIL="))
            .collect::<Vec<_>>()
            .join(";");
        if let Ok(unbounded) = source(&without_until).parse() {
            set = unbounded;
        }
    }
    // Union DTSTART in one pass, counting it even if filters disagree. Do not
    // collect/restart just to probe the first date (or to inspect a limited tail).
    let count = set.get_rrule().first().and_then(|r| r.get_count());
    let delta = match (&event.start, &event.end) {
        (IcsTime::Fixed(a), IcsTime::Fixed(b)) => *b - *a,
        _ => local(&event.end, tz) - anchor,
    };
    // Local time is not monotone around folds. Bound conservatively in wall space,
    // then apply exact half-open instant bounds (and inclusive UTC UNTIL).
    let wall_bound = to_local(ctx.options.horizon_end, tz)
        .checked_add_signed(Duration::days(2))
        .unwrap_or(NaiveDateTime::MAX);
    let wall_bound = until_wall.map_or(wall_bound, |until| wall_bound.min(until));
    let set = set.limit();
    let mut iter = (&set).into_iter();
    let mut seen = BTreeSet::new();
    let mut emitted = 0;
    let mut rule_count = 1u32;
    let mut pending = Some(event.start.clone());
    let mut limited = false;
    loop {
        let start = if let Some(start) = pending.take() {
            start
        } else {
            if count.is_some_and(|count| rule_count >= count) {
                break;
            }
            let Some(candidate) = out.scan(|| iter.next()) else {
                limited = true;
                break;
            };
            let Some(occurrence) = candidate else {
                // rrule's WasLimited flag is private. Treat early exhaustion as
                // potentially incomplete rather than rescanning all history to
                // recover it. COUNT and UNTIL completion are checked separately.
                limited = true;
                break;
            };
            let wall = occurrence.naive_utc();
            if wall > wall_bound {
                break;
            }
            // The explicit DTSTART above owns this nominal anchor. Re-resolving
            // it would fabricate an earlier-fold duplicate of a known later instant.
            if wall == anchor {
                continue;
            }
            rule_count = rule_count.saturating_add(1);
            match event.start {
                IcsTime::Date(_) => IcsTime::Date(wall.date()),
                IcsTime::Floating(_) => IcsTime::Floating(wall),
                IcsTime::Fixed(_) => IcsTime::Fixed(resolve_local(wall, tz)),
            }
        };
        let at = instant(&start, tz);
        if superseded_from.is_some_and(|rid| match rid {
            IcsTime::Date(d) => local(&start, tz).date() >= *d,
            IcsTime::Floating(d) => local(&start, tz) >= *d,
            IcsTime::Fixed(d) => at >= *d,
        }) {
            break;
        }
        if is_override && start == event.start
            || at < ctx.options.horizon_start
            || at >= ctx.options.horizon_end
            || instant_until.is_some_and(|until| at > until)
            || event
                .exdates
                .iter()
                .any(|ex| matches_instance(ex, &start, tz))
            || !seen.insert(start.clone())
        {
            continue;
        }
        if emitted == 10_000 {
            limited = true;
            break;
        }
        let end = if start == event.start {
            Some(event.end.clone())
        } else {
            match nominal {
                Some(d) => add_duration(&start, Some(tz), d),
                None => add(&start, delta),
            }
        };
        if let Some(end) = end {
            let mut e = event.clone();
            e.start = start.clone();
            e.end = end;
            e.recurrence_id = Some(start.clone());
            e.origin = EventOrigin::Expanded {
                rule_instance_start: start,
            };
            e.rrule = None;
            e.exdates.clear();
            out.events.push(e);
            emitted += 1;
        } else {
            warn(
                out,
                Some(ctx.uid),
                WarningCode::InvalidEvent,
                "Expanded end overflowed",
            );
        }
    }
    if limited {
        warn(
            out,
            Some(ctx.uid),
            WarningCode::ExpansionLimited,
            "RRULE expansion reached in-window instance or iteration safety limit; incomplete",
        );
    }
}
