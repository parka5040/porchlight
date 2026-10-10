use super::*;
fn escape(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || matches!(c, '\t' | '\r' | '\n'))
        .collect::<String>()
        .replace('\\', "\\\\")
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\\n")
        .replace(';', "\\;")
        .replace(',', "\\,")
}
fn time_line(name: &str, t: &IcsTime, tz: Option<&str>) -> String {
    match t {
        IcsTime::Date(d) => format!("{name};VALUE=DATE:{}", d.format("%Y%m%d")),
        IcsTime::Floating(d) => format!("{name}:{}", d.format("%Y%m%dT%H%M%S")),
        IcsTime::Fixed(d) => match tz.and_then(parse_tz) {
            Some(zone) if zone != chrono_tz::UTC => format!(
                "{name};TZID={zone}:{}",
                to_local(*d, zone).format("%Y%m%dT%H%M%S")
            ),
            _ => format!("{name}:{}", d.format("%Y%m%dT%H%M%SZ")),
        },
    }
}
/// RFC5545 serialization, UTF-8-safe folding at 75 octets, CRLF, escaped TEXT.
/// Events from parse satisfy shape invariants. IANA TZID references intentionally
/// omit VTIMEZONE. `dtstamp` is supplied by O2 as the export time.
/// A fixed start in the second pass of a fall-back hour is emitted in UTC to retain
/// its identity. Third-party clients may expand a recurring UTC master in UTC and
/// shift its local clock across DST; O2 must include this in its export notes.
/// RECURRENCE-ID is emitted only for Overrides of a Series in this export.
/// Expanded/orphan-override standalone events get distinct standard UIDs, with
/// X-PORCHLIGHT-{UID,INSTANCE,ORIGIN} preserving typed source identity on re-import.
/// Other clients can ignore these extensions without seeing orphan overrides.
pub fn write(events: &[ImportedEvent], calendar_name: &str, dtstamp: DateTime<Utc>) -> String {
    let mut lines = vec![
        "BEGIN:VCALENDAR".to_owned(),
        "VERSION:2.0".to_owned(),
        "PRODID:-//Porchlight//EN".to_owned(),
        format!("X-WR-CALNAME:{}", escape(calendar_name)),
    ];
    let series: BTreeSet<_> = events
        .iter()
        .filter(|e| {
            e.origin == EventOrigin::Series && e.rrule.is_some() && e.recurrence_id.is_none()
        })
        .map(|e| e.uid.as_str())
        .collect();
    for e in events {
        let real_override = e.origin == EventOrigin::Override && series.contains(e.uid.as_str());
        let standalone_id = if real_override {
            None
        } else {
            match &e.origin {
                EventOrigin::Expanded {
                    rule_instance_start,
                } => Some(rule_instance_start),
                EventOrigin::Override => e.recurrence_id.as_ref(),
                _ => None,
            }
        };
        // Standalone events need distinct standard UIDs, never orphan RECURRENCE-ID.
        // Extensions preserve Porchlight's typed source identity on re-import.
        let uid = standalone_id.map_or_else(
            || e.uid.clone(),
            |id| format!("{}-porchlight-{}", e.uid, time_line("instance", id, None)),
        );
        lines.extend([
            "BEGIN:VEVENT".to_owned(),
            format!("UID:{}", escape(&uid)),
            format!("DTSTAMP:{}", dtstamp.format("%Y%m%dT%H%M%SZ")),
            format!("SEQUENCE:{}", e.sequence),
            format!("SUMMARY:{}", escape(&e.summary)),
            format!("DESCRIPTION:{}", escape(&e.description)),
            format!("LOCATION:{}", escape(&e.location)),
        ]);
        if let Some(id) = standalone_id {
            lines.push(format!("X-PORCHLIGHT-UID;VALUE=TEXT:{}", escape(&e.uid)));
            lines.push(time_line("X-PORCHLIGHT-INSTANCE", id, None));
            lines.push(format!(
                "X-PORCHLIGHT-ORIGIN:{}",
                if e.origin == EventOrigin::Override {
                    "OVERRIDE"
                } else {
                    "EXPANDED"
                }
            ));
        }
        let tz = e.tzid.as_deref();
        if let (IcsTime::Fixed(d), Some(zone)) = (&e.start, tz.and_then(parse_tz))
            && resolve_local(to_local(*d, zone), zone) != *d
        {
            lines.push(format!("X-PORCHLIGHT-TZID:{zone}"));
        }
        for (name, t) in [("DTSTART", &e.start), ("DTEND", &e.end)] {
            lines.push(safe_time_line(name, t, tz));
        }
        if real_override && let Some(t) = &e.recurrence_id {
            lines.push(safe_time_line("RECURRENCE-ID", t, tz));
        }
        for t in &e.exdates {
            lines.push(safe_time_line("EXDATE", t, tz));
        }
        if let Some(rule) = &e.rrule {
            lines.push(format!("RRULE:{}", rule.replace(['\r', '\n'], "")));
        }
        lines.push(format!(
            "STATUS:{}",
            match e.status {
                EventStatus::Confirmed => "CONFIRMED",
                EventStatus::Tentative => "TENTATIVE",
                EventStatus::Cancelled => "CANCELLED",
            }
        ));
        lines.push("END:VEVENT".to_owned());
    }
    lines.push("END:VCALENDAR".to_owned());
    let mut out = String::new();
    for line in lines {
        let mut bytes = 0;
        for c in line.chars() {
            if bytes + c.len_utf8() > 75 {
                out.push_str("\r\n ");
                bytes = 1;
            }
            out.push(c);
            bytes += c.len_utf8();
        }
        out.push_str("\r\n");
    }
    out
}
fn safe_time_line(name: &str, t: &IcsTime, tz: Option<&str>) -> String {
    if let (IcsTime::Fixed(d), Some(zone)) = (t, tz.and_then(parse_tz))
        && resolve_local(to_local(*d, zone), zone) != *d
    {
        return time_line(name, t, None);
    }
    time_line(name, t, tz)
}
