//! Enrollment links; the CA fingerprint is parsed locally to keep core independent of PKI.

use std::fmt;
use url::Url;

#[derive(Clone, PartialEq, Eq)]
pub struct EnrollUri {
    pub host: String,
    pub token: String,
    pub ca_fingerprint: [u8; 32],
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum EnrollUriError {
    #[error("invalid enrollment URI")]
    InvalidUri,
    #[error("invalid enrollment authority")]
    InvalidAuthority,
    #[error("expected unique, nonempty host, token and ca enrollment parameters")]
    InvalidParameters,
    #[error("invalid CA fingerprint")]
    InvalidFingerprint,
}

impl fmt::Debug for EnrollUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnrollUri")
            .field("host", &self.host)
            .field("token", &"…redacted")
            .field(
                "ca_fingerprint",
                &format!("sha256:{}", hex::encode(self.ca_fingerprint)),
            )
            .finish()
    }
}

// Query values retain URI-safe punctuation (including IPv6 brackets and colons).
// Escape pair delimiters, literal '%' and '+' too: query_pairs uses form decoding.
fn encode_query_value(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~!$'()*,;:@[]/?".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    encoded
}

impl EnrollUri {
    pub fn to_uri(&self) -> String {
        let mut uri = Url::parse("porchlight://enroll").expect("static enrollment URI is valid");
        uri.set_query(Some(&format!(
            "host={}&token={}&ca=sha256:{}",
            encode_query_value(&self.host),
            encode_query_value(&self.token),
            hex::encode(self.ca_fingerprint),
        )));
        uri.into()
    }

    pub fn parse(s: &str) -> Result<EnrollUri, EnrollUriError> {
        let uri = Url::parse(s).map_err(|_| EnrollUriError::InvalidUri)?;
        if uri.scheme() != "porchlight"
            || uri.host_str() != Some("enroll")
            || !uri.username().is_empty()
            || uri.password().is_some()
            || uri.port().is_some()
            || !matches!(uri.path(), "" | "/")
            || uri.fragment().is_some()
        {
            return Err(EnrollUriError::InvalidAuthority);
        }
        let (mut host, mut token, mut ca) = (None, None, None);
        for (key, value) in uri.query_pairs() {
            let slot = match key.as_ref() {
                "host" => &mut host,
                "token" => &mut token,
                "ca" => &mut ca,
                _ => continue,
            };
            if slot.is_some() || value.is_empty() {
                return Err(EnrollUriError::InvalidParameters);
            }
            *slot = Some(value.into_owned());
        }
        let host = host.ok_or(EnrollUriError::InvalidParameters)?;
        let token = token.ok_or(EnrollUriError::InvalidParameters)?;
        let ca = ca.ok_or(EnrollUriError::InvalidParameters)?;
        let (prefix, hex) = ca
            .split_once(':')
            .ok_or(EnrollUriError::InvalidFingerprint)?;
        if !prefix.eq_ignore_ascii_case("sha256") || hex.len() != 64 {
            return Err(EnrollUriError::InvalidFingerprint);
        }
        let mut ca_fingerprint = [0u8; 32];
        hex::decode_to_slice(hex, &mut ca_fingerprint)
            .map_err(|_| EnrollUriError::InvalidFingerprint)?;
        Ok(Self {
            host,
            token,
            ca_fingerprint,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enroll_uri_roundtrip() {
        for host in ["porchlight.lan", "10.0.2.2:8444", "name:9444"] {
            let uri = EnrollUri {
                host: host.into(),
                token: "ple_abc_123-xyz".into(),
                ca_fingerprint: [0xab; 32],
            };
            let text = uri.to_uri();
            assert!(text.starts_with("porchlight://enroll?host="));
            assert_eq!(EnrollUri::parse(&text).unwrap(), uri);
            assert!(!format!("{uri:?}").contains(&uri.token));
            assert!(format!("{uri:?}").contains("…redacted"));
            assert!(EnrollUri::parse(&text.replace("porchlight://", "https://")).is_err());
            assert!(EnrollUri::parse(&text.replace("//enroll?", "//other?")).is_err());
        }
        for invalid in [
            "porchlight://enroll?host=lan&token=ple_test",
            "porchlight://enroll?token=ple_test&ca=sha256:bad",
            "porchlight://enroll?host=lan&ca=sha256:bad",
            "porchlight://enroll?host=lan&token=ple_test&ca=sha256:bad",
            "this is not a URI",
        ] {
            assert!(EnrollUri::parse(invalid).is_err());
        }
    }

    #[test]
    fn to_uri_emits_literal_port_and_fingerprint() {
        let uri = EnrollUri {
            host: "10.0.2.2:8444".into(),
            token: "ple_abc_123-xyz".into(),
            ca_fingerprint: [0xab; 32],
        };
        assert_eq!(
            uri.to_uri(),
            "porchlight://enroll?host=10.0.2.2:8444&token=ple_abc_123-xyz&ca=sha256:abababababababababababababababababababababababababababababababab"
        );
    }

    #[test]
    fn to_uri_emits_literal_ipv6_host() {
        let uri = EnrollUri {
            host: "[fd00::1]:9000".into(),
            token: "ple_abc_123-xyz".into(),
            ca_fingerprint: [0xab; 32],
        };
        assert_eq!(
            uri.to_uri(),
            "porchlight://enroll?host=[fd00::1]:9000&token=ple_abc_123-xyz&ca=sha256:abababababababababababababababababababababababababababababababab"
        );
    }

    #[test]
    fn enroll_uri_accepts_root_path() {
        let valid = format!(
            "porchlight://enroll?host=lan&token=test&ca=sha256:{}",
            "ab".repeat(32)
        );
        assert_eq!(
            EnrollUri::parse(&valid).unwrap(),
            EnrollUri::parse(&valid.replace("enroll?", "enroll/?")).unwrap()
        );
    }

    #[test]
    fn enroll_uri_ignores_unknown_parameters() {
        let valid = format!(
            "porchlight://enroll?host=lan&token=test&ca=sha256:{}",
            "ab".repeat(32)
        );
        assert_eq!(
            EnrollUri::parse(&valid).unwrap(),
            EnrollUri::parse(&format!("{valid}&extra=value&extra=other&future=")).unwrap()
        );
    }

    #[test]
    fn enroll_uri_encodes_values_and_accepts_uppercase_fingerprint() {
        let uri = EnrollUri {
            host: "[::1]:9444&=+#% é".into(),
            token: "ple_a_-&=? +é#%\r\n".into(),
            ca_fingerprint: [0xab; 32],
        };
        assert_eq!(EnrollUri::parse(&uri.to_uri()).unwrap(), uri);
        let upper = format!(
            "porchlight://enroll?host=lan&token=test&ca=SHA256:{}",
            "AB".repeat(32)
        );
        assert_eq!(EnrollUri::parse(&upper).unwrap().ca_fingerprint, [0xab; 32]);
    }

    #[test]
    fn enroll_uri_rejects_duplicate_or_malformed_parts_without_leaking() {
        let secret = "ple_do_not_leak";
        let valid = format!(
            "porchlight://enroll?host=lan&token={secret}&ca=sha256:{}",
            "ab".repeat(32)
        );
        for invalid in [
            valid.replace("host=lan", "host=lan&host=lan"),
            format!("{valid}&host=other"),
            format!("{valid}&token=other"),
            format!("{valid}&ca=sha256:{}", "ab".repeat(32)),
            valid.replace("host=lan", "host="),
            valid.replace(&format!("token={secret}"), "token="),
            valid.replace("sha256:", "sha512:"),
            valid.replace("ab", "gg"),
            valid.replace("//enroll?", "//user@enroll?"),
            valid.replace("//enroll?", "//enroll:8444?"),
            valid.replace("//enroll?", "//enroll/path?"),
            format!("{valid}#fragment"),
        ] {
            let error = EnrollUri::parse(&invalid).unwrap_err();
            assert!(!format!("{error:?}").contains(secret));
            assert!(!format!("{error}").contains(secret));
        }
    }
}
