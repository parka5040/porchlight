//! Opaque, typed bearer tokens. Persist only their SHA-256 hashes.

use argon2::password_hash::phc::Output;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::fmt;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Session,
    Pat,
    Enrollment,
    StepUp,
}

impl TokenKind {
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Session => "pls_",
            Self::Pat => "plp_",
            Self::Enrollment => "ple_",
            Self::StepUp => "plu_",
        }
    }
}

/// A secret token; formatting reveals only the type prefix.
pub struct Token(Zeroizing<String>);

impl Token {
    /// Panics if the OS random source fails; never falls back to weaker entropy.
    pub fn generate(kind: TokenKind) -> Token {
        let mut bytes = Zeroizing::new([0u8; 32]);
        getrandom::fill(&mut *bytes).expect("OS random source failed");
        let mut value = Zeroizing::new(String::with_capacity(kind.prefix().len() + 43));
        value.push_str(kind.prefix());
        URL_SAFE_NO_PAD.encode_string(bytes.as_slice(), &mut value);
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn hash(&self) -> [u8; 32] {
        hash_token(self.expose())
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}…redacted", &self.0[..4])
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// SHA-256 of the full UTF-8 token string, including its prefix.
pub fn hash_token(presented: &str) -> [u8; 32] {
    Sha256::digest(presented.as_bytes()).into()
}

/// Shape validation only: the prefix and 43 unpadded base64url characters.
pub fn looks_like(kind: TokenKind, presented: &str) -> bool {
    let Some(body) = presented.strip_prefix(kind.prefix()) else {
        return false;
    };
    body.len() == 43
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Constant-time equality for fixed-size hashes.
pub fn hashes_equal(a: &[u8; 32], b: &[u8; 32]) -> bool {
    // RustCrypto's PHC Output explicitly guarantees constant-time PartialEq.
    // Reuse that implementation through argon2's public reexport, rather than
    // hand-roll a comparison that the optimizer could turn into early-exit code.
    Output::new(a).expect("32-byte hash fits PHC output")
        == Output::new(b).expect("32-byte hash fits PHC output")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::collections::HashSet;

    #[test]
    fn tokens_have_prefix_and_entropy() {
        for (kind, prefix) in [
            (TokenKind::Session, "pls_"),
            (TokenKind::Pat, "plp_"),
            (TokenKind::Enrollment, "ple_"),
            (TokenKind::StepUp, "plu_"),
        ] {
            assert_eq!(kind.prefix(), prefix);
            let mut tokens = HashSet::new();
            for _ in 0..1000 {
                let token = Token::generate(kind);
                let value = token.expose();
                assert!(value.starts_with(prefix));
                assert_eq!(value.len(), prefix.len() + 43);
                assert!(looks_like(kind, value));
                assert_eq!(format!("{token:?}"), format!("{prefix}…redacted"));
                assert_eq!(format!("{token}"), format!("{prefix}…redacted"));
                assert!(!format!("{token:?}").contains(&value[prefix.len()..]));
                assert!(tokens.insert(value.to_owned()));
            }
            assert!(!looks_like(kind, &format!("bad_{}", "A".repeat(43))));
            assert!(!looks_like(kind, &format!("{prefix}{}", "A".repeat(42))));
            assert!(!looks_like(kind, &format!("{prefix}{}", "A".repeat(44))));
            for invalid in ["+", "/", "=", " ", "é"] {
                assert!(!looks_like(
                    kind,
                    &format!("{prefix}{}{invalid}", "A".repeat(42))
                ));
            }
            assert!(looks_like(kind, &format!("{prefix}{}_-", "A".repeat(41))));
        }
    }

    #[test]
    fn hash_token_matches_token_hash() {
        let token = Token::generate(TokenKind::Session);
        assert_eq!(hash_token(token.expose()), token.hash());
        assert_ne!(token.hash(), Token::generate(TokenKind::Session).hash());
        let expected: [u8; 32] = Sha256::digest(b"pls_x").into();
        assert_eq!(hash_token("pls_x"), expected);
    }

    #[test]
    fn hash_equality_checks_every_byte() {
        let a = [42; 32];
        assert!(hashes_equal(&a, &a));
        for index in 0..32 {
            let mut b = a;
            b[index] ^= 1;
            assert!(!hashes_equal(&a, &b));
            assert!(!hashes_equal(&b, &a));
        }
    }
}
