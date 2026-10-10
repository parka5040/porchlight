//! Device platform names and authentication lifetimes.

use chrono::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Android,
    Desktop,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Android => "android",
            Self::Desktop => "desktop",
        }
    }

    pub fn parse(s: &str) -> Option<Platform> {
        match s {
            "android" => Some(Self::Android),
            "desktop" => Some(Self::Desktop),
            _ => None,
        }
    }
}

pub fn session_lifetime(p: Platform) -> Duration {
    match p {
        Platform::Android => Duration::days(180),
        Platform::Desktop => Duration::days(90),
    }
}

pub const SESSION_TOUCH_INTERVAL_SECS: i64 = 3600;
pub const ENROLLMENT_TOKEN_MINUTES: i64 = 15;
pub const RESET_CODE_MINUTES: i64 = 15;
pub const RESET_CODE_MAX_FAILURES: i32 = 5;
pub const STEP_UP_MINUTES: i64 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepUpAction {
    DeviceRevoke,
    HouseholdExport,
}

impl StepUpAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeviceRevoke => "device.revoke",
            Self::HouseholdExport => "household.export",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "device.revoke" => Some(Self::DeviceRevoke),
            "household.export" => Some(Self::HouseholdExport),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn session_lifetimes() {
        assert_eq!(session_lifetime(Platform::Android), Duration::days(180));
        assert_eq!(session_lifetime(Platform::Desktop), Duration::days(90));
        for (platform, name) in [
            (Platform::Android, "android"),
            (Platform::Desktop, "desktop"),
        ] {
            assert_eq!(platform.as_str(), name);
            assert_eq!(Platform::parse(name), Some(platform));
        }
        assert_eq!(Platform::parse("unknown"), None);
        assert_eq!(Platform::parse("Android"), None);
        assert_eq!(SESSION_TOUCH_INTERVAL_SECS, 3600);
        assert_eq!(ENROLLMENT_TOKEN_MINUTES, 15);
        assert_eq!(RESET_CODE_MINUTES, 15);
        assert_eq!(RESET_CODE_MAX_FAILURES, 5);
        assert_eq!(STEP_UP_MINUTES, 5);
    }

    #[test]
    fn step_up_actions() {
        assert_eq!(StepUpAction::DeviceRevoke.as_str(), "device.revoke");
        assert_eq!(
            StepUpAction::parse("device.revoke"),
            Some(StepUpAction::DeviceRevoke)
        );
        assert_eq!(StepUpAction::parse("unknown"), None);
    }
}
