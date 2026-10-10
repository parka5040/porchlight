//! Human-entered, 50-bit Crockford base32 password reset codes.

use super::token::hash_token;
use std::fmt;
use zeroize::Zeroizing;

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

pub struct ResetCode(Zeroizing<String>);

impl ResetCode {
    /// Panics if the OS random source fails.
    pub fn generate() -> ResetCode {
        let mut bytes = Zeroizing::new([0u8; 10]);
        getrandom::fill(&mut *bytes).expect("OS random source failed");
        let mut value = Zeroizing::new(String::with_capacity(11));
        for (index, byte) in bytes.iter().enumerate() {
            if index == 5 {
                value.push('-');
            }
            // 256 is divisible by 32: taking five bits introduces no bias.
            value.push(ALPHABET[(byte & 31) as usize] as char);
        }
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ResetCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ResetCode(…redacted)")
    }
}

/// Uppercase ASCII, remove hyphens/spaces, and fold Crockford aliases.
/// Reject anything other than exactly ten Crockford characters.
pub fn normalize_reset_code(input: &str) -> Option<String> {
    let mut normalized = Zeroizing::new(String::with_capacity(10));
    for byte in input.bytes() {
        if byte == b'-' || byte == b' ' {
            continue;
        }
        let canonical = match byte.to_ascii_uppercase() {
            b'O' => b'0',
            b'I' | b'L' => b'1',
            other => other,
        };
        if normalized.len() == 10 || !ALPHABET.contains(&canonical) {
            return None;
        }
        normalized.push(canonical as char);
    }
    (normalized.len() == 10).then(|| std::mem::take(&mut *normalized))
}

pub fn hash_reset_code(input: &str) -> Option<[u8; 32]> {
    let normalized = Zeroizing::new(normalize_reset_code(input)?);
    Some(hash_token(&normalized))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::collections::HashSet;

    #[test]
    fn reset_codes() {
        let alphabet = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        let mut codes = HashSet::new();
        for _ in 0..1000 {
            let code = ResetCode::generate();
            let value = code.expose();
            assert_eq!(value.len(), 11);
            assert_eq!(value.as_bytes()[5], b'-');
            assert!(
                value
                    .bytes()
                    .enumerate()
                    .all(|(i, b)| i == 5 || alphabet.contains(&b))
            );
            assert_eq!(format!("{code:?}"), "ResetCode(…redacted)");
            assert!(!format!("{code:?}").contains(value));
            assert!(codes.insert(value.to_owned()));
        }
        assert_eq!(
            normalize_reset_code("abcde fghjk"),
            Some("ABCDEFGHJK".into())
        );
        assert_eq!(
            normalize_reset_code("o1il0-xxxxx"),
            Some("01110XXXXX".into())
        );
        for invalid in [
            "123456789",
            "12345678901",
            "12345-6789U",
            "12345\t67890",
            "é234567890",
        ] {
            assert_eq!(normalize_reset_code(invalid), None);
            assert_eq!(hash_reset_code(invalid), None);
        }
        assert_eq!(
            hash_reset_code("ABCDE-FGHJK"),
            hash_reset_code("abcdefghjk")
        );
        assert_eq!(
            hash_reset_code("O1IL0-XXXXX"),
            hash_reset_code("01110xxxxx")
        );
        let expected: [u8; 32] = Sha256::digest(b"ABCDEFGHJK").into();
        assert_eq!(hash_reset_code("abcde fghjk"), Some(expected));
    }
}
