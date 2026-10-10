use super::*;
/// Common Windows calendar identifiers. Canada shares US zones except Newfoundland
/// and Atlantic; mappings intentionally choose one canonical IANA representative.
pub const WINDOWS_ZONES: &[(&str, &str)] = &[
    ("Dateline Standard Time", "Etc/GMT+12"),
    ("UTC", "UTC"),
    ("Aleutian Standard Time", "America/Adak"),
    ("Yukon Standard Time", "America/Whitehorse"),
    ("Lord Howe Standard Time", "Australia/Lord_Howe"),
    ("Aus Central W. Standard Time", "Australia/Eucla"),
    ("Kaliningrad Standard Time", "Europe/Kaliningrad"),
    ("Belarus Standard Time", "Europe/Minsk"),
    ("Volgograd Standard Time", "Europe/Volgograd"),
    ("Hawaiian Standard Time", "Pacific/Honolulu"),
    ("Alaskan Standard Time", "America/Anchorage"),
    ("Pacific Standard Time", "America/Los_Angeles"),
    ("US Mountain Standard Time", "America/Phoenix"),
    ("Mountain Standard Time", "America/Denver"),
    ("Central Standard Time", "America/Chicago"),
    ("Canada Central Standard Time", "America/Regina"),
    ("Eastern Standard Time", "America/New_York"),
    ("US Eastern Standard Time", "America/Indianapolis"),
    ("Atlantic Standard Time", "America/Halifax"),
    ("Newfoundland Standard Time", "America/St_Johns"),
    ("GMT Standard Time", "Europe/London"),
    ("Greenwich Standard Time", "Atlantic/Reykjavik"),
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("Central Europe Standard Time", "Europe/Budapest"),
    ("Romance Standard Time", "Europe/Paris"),
    ("Central European Standard Time", "Europe/Warsaw"),
    ("W. Central Africa Standard Time", "Africa/Lagos"),
    ("GTB Standard Time", "Europe/Bucharest"),
    ("FLE Standard Time", "Europe/Kyiv"),
    ("E. Europe Standard Time", "Europe/Chisinau"),
    ("Turkey Standard Time", "Europe/Istanbul"),
    ("Russian Standard Time", "Europe/Moscow"),
    ("AUS Eastern Standard Time", "Australia/Sydney"),
    ("E. Australia Standard Time", "Australia/Brisbane"),
    ("Cen. Australia Standard Time", "Australia/Adelaide"),
    ("AUS Central Standard Time", "Australia/Darwin"),
    ("W. Australia Standard Time", "Australia/Perth"),
    ("Tasmania Standard Time", "Australia/Hobart"),
    ("India Standard Time", "Asia/Kolkata"),
    ("Tokyo Standard Time", "Asia/Tokyo"),
];
pub(super) fn known_zone(name: &str) -> Option<Tz> {
    parse_tz(name).or_else(|| {
        WINDOWS_ZONES
            .iter()
            .find(|(w, _)| w.eq_ignore_ascii_case(name))
            .and_then(|(_, i)| parse_tz(i))
    })
}
