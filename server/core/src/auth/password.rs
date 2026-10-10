//! Password policy and synchronous Argon2id hashing.
//!
//! API callers must use spawn_blocking and a semaphore permitting at most two
//! hashes/verifies at once. Core intentionally has no async runtime dependency.

use super::breach::is_breached;
use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{
        PasswordHasher as _, PasswordVerifier as _,
        phc::{Output, PasswordHash},
    },
};

// Test-only counting seam: each engine here is used for one Argon2 evaluation.
#[cfg(test)]
std::thread_local! {
    static ARGON2_EVALUATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub const MIN_PASSWORD_CHARS: usize = 12;
pub const MAX_PASSWORD_CHARS: usize = 1024;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    #[error("too short")]
    TooShort,
    #[error("too long")]
    TooLong,
    #[error("breached")]
    Breached,
}

pub fn check_policy(password: &str) -> Result<(), PolicyError> {
    let length = password.chars().count();
    if length < MIN_PASSWORD_CHARS {
        Err(PolicyError::TooShort)
    } else if length > MAX_PASSWORD_CHARS {
        Err(PolicyError::TooLong)
    } else if is_breached(password) {
        Err(PolicyError::Breached)
    } else {
        Ok(())
    }
}

pub struct PasswordHasher {
    params: Params,
}

impl PasswordHasher {
    pub fn production() -> Self {
        Self::with_params(65536, 3, 4)
    }

    /// Cheap parameters are for tests only. Production callers must use production().
    /// Panics if the provided Argon2 parameters are invalid.
    pub fn with_params(m_kib: u32, t: u32, p: u32) -> Self {
        Self {
            params: Params::new(m_kib, t, p, Some(32)).expect("invalid Argon2 parameters"),
        }
    }

    pub fn params(&self) -> (u32, u32, u32) {
        (
            self.params.m_cost(),
            self.params.t_cost(),
            self.params.p_cost(),
        )
    }

    fn argon2(&self) -> Argon2<'_> {
        #[cfg(test)]
        ARGON2_EVALUATIONS.with(|count| count.set(count.get() + 1));
        Argon2::new(Algorithm::Argon2id, Version::V0x13, self.params.clone())
    }

    /// Produce a PHC string with a random 16-byte salt and 32-byte output.
    /// Panics on OS random-source or hashing failure. Policy is checked separately.
    pub fn hash(&self, password: &str) -> String {
        let mut salt = [0u8; 16];
        getrandom::fill(&mut salt).expect("OS random source failed");
        let phc: PasswordHash = self
            .argon2()
            .hash_password_with_salt(password.as_bytes(), &salt)
            .expect("password hashing failed");
        phc.to_string()
    }

    /// Verification uses parameters embedded in the stored PHC, not this instance.
    /// Malformed or unsupported PHC strings return false.
    pub fn verify(&self, password: &str, phc: &str) -> bool {
        self.argon2()
            .verify_password(password.as_bytes(), phc)
            .is_ok()
    }

    /// Burn exactly one Argon2id evaluation using this instance's parameters,
    /// including on a freshly constructed hasher. No cache initialization or RNG.
    pub fn verify_dummy(&self, password: &str) {
        let phc: PasswordHash = self
            .argon2()
            .hash_password_with_salt(password.as_bytes(), &[0u8; 16])
            .expect("dummy password hashing failed");
        let expected = Output::new(&[0u8; 32]).expect("32-byte dummy hash fits PHC output");
        // Output's equality is constant-time; retain the otherwise unused comparison.
        std::hint::black_box(phc.hash.expect("Argon2 produces a hash") == expected);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::breach::breach_list_len;
    use argon2::password_hash::phc::PasswordHash;

    #[test]
    fn policy() {
        assert_eq!(MIN_PASSWORD_CHARS, 12);
        assert_eq!(MAX_PASSWORD_CHARS, 1024);
        assert_eq!(check_policy(&"a".repeat(11)), Err(PolicyError::TooShort));
        assert_eq!(check_policy("unique-stone"), Ok(()));
        assert_eq!(check_policy(&"a".repeat(1024)), Ok(()));
        assert_eq!(check_policy(&"a".repeat(1025)), Err(PolicyError::TooLong));
        assert_eq!(check_policy(&"é".repeat(11)), Err(PolicyError::TooShort));
        assert_eq!(check_policy(&"é".repeat(12)), Ok(()));
        assert_eq!(check_policy("password1234"), Err(PolicyError::Breached));
        assert_eq!(check_policy("PASSWORD1234"), Err(PolicyError::Breached));
        assert!(breach_list_len() >= 40);
    }

    #[test]
    fn hasher_production_params() {
        let hasher = PasswordHasher::production();
        assert_eq!(hasher.params(), (65536, 3, 4));
        let hash = hasher.hash("correct horse battery staple");
        assert!(hash.starts_with("$argon2id$v=19$m=65536,t=3,p=4$"));
        let phc = PasswordHash::new(&hash).unwrap();
        assert_eq!(phc.salt.unwrap().len(), 16);
        assert_eq!(phc.hash.unwrap().len(), 32);
    }

    #[test]
    fn hasher_roundtrip_cheap() {
        let hasher = PasswordHasher::with_params(8, 1, 1);
        assert_eq!(hasher.params(), (8, 1, 1));
        let hash = hasher.hash("correct horse battery staple");
        assert!(hasher.verify("correct horse battery staple", &hash));
        assert!(!hasher.verify("wrong password", &hash));
        for malformed in [
            "",
            "not a PHC",
            "$argon2id$v=19$m=bad$",
            "$argon2id$v=19$m=8,t=1,p=1",
        ] {
            assert!(!hasher.verify("correct horse battery staple", malformed));
        }
        assert_ne!(hash, hasher.hash("correct horse battery staple"));
        assert!(PasswordHasher::production().verify("correct horse battery staple", &hash));
    }

    #[test]
    fn dummy_verification_performs_one_evaluation_even_on_first_call() {
        for (m, t, p) in [(8, 1, 1), (32, 2, 2)] {
            let hasher = PasswordHasher::with_params(m, t, p);
            let hash = hasher.hash("some password");
            ARGON2_EVALUATIONS.with(|count| count.set(0));
            assert!(hasher.verify("some password", &hash));
            assert_eq!(ARGON2_EVALUATIONS.with(|count| count.get()), 1);

            // A fresh instance must not initialize a dummy hash by hashing first.
            let fresh = PasswordHasher::with_params(m, t, p);
            for password in ["some password", "another password", ""] {
                ARGON2_EVALUATIONS.with(|count| count.set(0));
                fresh.verify_dummy(password);
                assert_eq!(ARGON2_EVALUATIONS.with(|count| count.get()), 1);
            }
        }
    }
}
