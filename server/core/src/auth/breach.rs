//! Case-insensitive, exact-match lookup in the owner's compiled-in breach list.

use std::sync::OnceLock;
use zeroize::Zeroizing;

static BREACHED: OnceLock<Vec<&'static str>> = OnceLock::new();

fn entries() -> &'static [&'static str] {
    BREACHED.get_or_init(|| {
        include_str!(concat!(env!("OUT_DIR"), "/breached.txt"))
            .lines()
            .collect()
    })
}

pub fn is_breached(password: &str) -> bool {
    let lowered = Zeroizing::new(password.to_lowercase());
    entries().binary_search(&lowered.as_str()).is_ok()
}

pub fn breach_list_len() -> usize {
    entries().len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn breach_matching_is_exact_and_case_insensitive() {
        assert!(is_breached("password1234"));
        assert!(is_breached("PASSWORD1234"));
        assert!(!is_breached(" password1234"));
        assert!(!is_breached("password1234 "));
        assert!(!is_breached("different orchard password"));
        assert!(breach_list_len() >= 40);
    }

    #[test]
    fn requested_seeds_are_breached_case_insensitively() {
        let seed = include_str!("../../data/breached-passwords.txt");
        for password in ["iloveyou1234", "qwertyuiopas", "letmein12345"] {
            assert!(seed.lines().any(|line| line == password));
            assert_eq!(password.chars().count(), 12);
            for form in [password.to_owned(), password.to_uppercase()] {
                assert!(is_breached(&form));
                assert_eq!(
                    crate::auth::password::check_policy(&form),
                    Err(crate::auth::password::PolicyError::Breached)
                );
            }
        }
    }

    #[test]
    fn compiled_list_is_sorted_unique_lowercase_and_long_enough() {
        let entries: Vec<_> = include_str!(concat!(env!("OUT_DIR"), "/breached.txt"))
            .lines()
            .collect();
        assert!(entries.windows(2).all(|w| w[0] < w[1]));
        assert!(
            entries
                .iter()
                .all(|entry| entry.chars().count() >= 12 && *entry == entry.to_lowercase())
        );
        assert_eq!(breach_list_len(), entries.len());
    }
}
