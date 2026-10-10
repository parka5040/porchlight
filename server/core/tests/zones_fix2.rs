use porchlight_core::{
    ics::{self, IcsTime, ParseOptions},
    time,
};

#[test]
fn ics_legacy_tzids_retain_offsets() {
    for (zone, expected) in [
        ("US/Eastern", "2026-07-01T16:00:00Z"),
        ("Asia/Calcutta", "2026-07-01T06:30:00Z"),
        ("CET", "2026-07-01T10:00:00Z"),
    ] {
        let text = format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:legacy\r\nDTSTART;TZID={zone}:20260701T120000\r\nDTEND;TZID={zone}:20260701T130000\r\nSUMMARY:Legacy\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
        let cal = ics::parse(
            &text,
            ParseOptions {
                fallback_tz: chrono_tz::UTC,
                horizon_start: "2026-01-01T00:00:00Z".parse().unwrap(),
                horizon_end: "2027-01-01T00:00:00Z".parse().unwrap(),
            },
        );
        assert!(cal.warnings.is_empty(), "{zone}: {:?}", cal.warnings);
        assert_eq!(
            cal.events[0].start,
            IcsTime::Fixed(expected.parse().unwrap())
        );
        assert!(time::parse_tz(zone).is_some());
    }
}

#[test]
fn storage_links_have_identical_offsets_2025_through_2037() {
    use chrono::{Offset, TimeZone};
    let start = chrono::Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
    let end = chrono::Utc.with_ymd_and_hms(2038, 1, 1, 0, 0, 0).unwrap();
    for &(link, target) in time::STORAGE_ZONE_LINKS {
        let (zone, name) = time::storage_zone(link).unwrap();
        assert_eq!(name, target, "{link}");
        let source = time::parse_tz(link).unwrap_or_else(|| {
            assert_eq!(link, "Z");
            chrono_tz::UTC
        });
        assert_eq!(time::parse_tz(target), Some(zone));
        let mut instant = start;
        while instant < end {
            assert_eq!(
                instant.with_timezone(&source).offset().fix(),
                instant.with_timezone(&zone).offset().fix(),
                "{link} -> {target} at {instant}"
            );
            instant += chrono::Duration::hours(6);
        }
    }
    for zone in chrono_tz::TZ_VARIANTS {
        let name = zone.name();
        assert_eq!(time::parse_tz(name), Some(zone));
        if let Ok((_, canonical)) = time::storage_zone(name) {
            assert!(canonical == "UTC" || canonical.contains('/'), "{name}");
        } else {
            assert_eq!(name, "Factory", "unexpected storage exception");
        }
    }
}
