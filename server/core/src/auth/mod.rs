//! Framework-free authentication primitives shared by the server and CLI.

pub mod breach;
pub mod enroll_uri;
pub mod lockout;
pub mod password;
pub mod reset_code;
pub mod session;
pub mod token;

pub use breach::{breach_list_len, is_breached};
pub use enroll_uri::{EnrollUri, EnrollUriError};
pub use lockout::{LOCKOUT_THRESHOLD, locked_until};
pub use password::{
    MAX_PASSWORD_CHARS, MIN_PASSWORD_CHARS, PasswordHasher, PolicyError, check_policy,
};
pub use reset_code::{ResetCode, hash_reset_code, normalize_reset_code};
pub use session::{
    ENROLLMENT_TOKEN_MINUTES, Platform, RESET_CODE_MAX_FAILURES, RESET_CODE_MINUTES,
    SESSION_TOUCH_INTERVAL_SECS, STEP_UP_MINUTES, StepUpAction, session_lifetime,
};
pub use token::{Token, TokenKind, hash_token, hashes_equal, looks_like};
