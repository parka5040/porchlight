//! Framework-free household PKI. All persistence and trust-store decisions belong to callers.

pub use std::time::SystemTime;
mod csr;
mod pinning;
use csr::validated_csr;
pub use csr::{generate_csr, validate_csr};
pub use pinning::{PinnedRootVerifier, enrollment_target};

use rcgen::PublicKeyData;
use std::{fmt, net::IpAddr};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
    SanType, SerialNumber,
};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use x509_parser::{
    pem::parse_x509_pem,
    prelude::{GeneralName, X509Certificate, X509Name},
};
use zeroize::Zeroizing;

const DAY_SECONDS: i64 = 86_400;
const CA_DAYS: u32 = 3650;
const CANONICAL_DNS: &str = "porchlight.lan";

/// Errors never contain input PEM, private keys, or untrusted subject strings.
#[derive(Debug, thiserror::Error)]
pub enum PkiError {
    #[error("certificate generation failed: {0}")]
    Rcgen(String),
    #[error("certificate parsing failed: {0}")]
    Parse(String),
    #[error("invalid CSR: {0}")]
    InvalidCsr(&'static str),
    #[error("only ECDSA P-256 with SHA-256 is supported")]
    UnsupportedKey,
    #[error("invalid SHA-256 fingerprint")]
    InvalidFingerprint,
    #[error("invalid household CA: {0}")]
    InvalidCa(&'static str),
    #[error("certificate validity is out of range")]
    InvalidValidity,
    #[error("invalid enrollment host")]
    InvalidEnrollmentTarget,
}

// Do not propagate rcgen's string-bearing errors: some embed untrusted ASN.1/PEM text.
fn rcgen_error(_: rcgen::Error) -> PkiError {
    PkiError::Rcgen("invalid parameters or cryptographic operation failed".into())
}

fn parse_error(reason: &str) -> PkiError {
    PkiError::Parse(reason.into())
}

/// SHA-256 of the certificate's complete DER encoding.
pub fn fingerprint(der: &[u8]) -> [u8; 32] {
    Sha256::digest(der).into()
}

/// Render a SHA-256 fingerprint as `sha256:<64 lowercase hex>`.
pub fn format_fingerprint(fp: &[u8; 32]) -> String {
    format!("sha256:{}", hex::encode(fp))
}

/// Parse the exact `sha256:` prefix and 64 case-insensitive hex digits.
pub fn parse_fingerprint(s: &str) -> Result<[u8; 32], PkiError> {
    let value = s
        .strip_prefix("sha256:")
        .ok_or(PkiError::InvalidFingerprint)?;
    if value.len() != 64 {
        return Err(PkiError::InvalidFingerprint);
    }
    let mut fp = [0; 32];
    hex::decode_to_slice(value, &mut fp).map_err(|_| PkiError::InvalidFingerprint)?;
    Ok(fp)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerSans {
    pub dns: Vec<String>,
    pub ips: Vec<IpAddr>,
}

/// A certificate and its public metadata (timestamps have X.509 second precision).
#[derive(Clone, Debug)]
pub struct IssuedCert {
    pub cert_pem: String,
    pub cert_der: Vec<u8>,
    pub fingerprint: [u8; 32],
    pub serial_hex: String,
    pub not_before: SystemTime,
    pub not_after: SystemTime,
}

/// Certificate plus a PKCS#8 private key. The PEM string is zeroized on drop.
pub struct CertifiedKey {
    pub cert: IssuedCert,
    pub key_pem: Zeroizing<String>,
}

impl fmt::Debug for CertifiedKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CertifiedKey")
            .field("cert", &self.cert)
            .field("key_pem", &"[REDACTED]")
            .finish()
    }
}

/// Household root and its signing key. No private material is exposed by Debug.
pub struct HouseholdCa {
    cert: IssuedCert,
    issuer: Issuer<'static, KeyPair>,
}

impl fmt::Debug for HouseholdCa {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HouseholdCa")
            .field("cert", &self.cert)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl HouseholdCa {
    /// Generate a self-signed P-256 root, valid for 3650 days with five minutes of skew.
    pub fn generate(household: &str, now: SystemTime) -> Result<Self, PkiError> {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(rcgen_error)?;
        let mut params = parameters(&format!("Porchlight {household} CA"), now, CA_DAYS)?;
        params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let cert = issued(params.self_signed(&key).map_err(rcgen_error)?)?;
        Ok(Self {
            cert,
            issuer: Issuer::new(params, key),
        })
    }

    /// Reload a self-signed P-256 CA and its matching key (without a wall-clock expiry check).
    pub fn from_pem(cert_pem: &str, key_pem: &str) -> Result<Self, PkiError> {
        let der = pem_der(cert_pem, "CERTIFICATE")?;
        let parsed = certificate(&der)?;
        let info = cert_info(&parsed, &der)?;
        if !info.is_ca {
            return Err(PkiError::InvalidCa("certificate is not a CA"));
        }
        let ku = parsed
            .key_usage()
            .map_err(|_| parse_error("invalid key usage"))?
            .ok_or(PkiError::InvalidCa("missing key usage"))?;
        if !ku.value.key_cert_sign() || !ku.value.crl_sign() {
            return Err(PkiError::InvalidCa("key usage does not permit CA signing"));
        }
        if parsed.subject() != parsed.issuer() || parsed.verify_signature(None).is_err() {
            return Err(PkiError::InvalidCa("certificate is not self-signed"));
        }
        let key = KeyPair::from_pem(key_pem).map_err(rcgen_error)?;
        if key.algorithm() != &PKCS_ECDSA_P256_SHA256 {
            return Err(PkiError::UnsupportedKey);
        }
        if parsed.public_key().raw != key.subject_public_key_info() {
            return Err(PkiError::InvalidCa("certificate and key do not match"));
        }
        let canonical = canonical_cert_pem(&der);
        let issuer = Issuer::from_ca_cert_pem(&canonical, key).map_err(rcgen_error)?;
        Ok(Self {
            cert: issued_from_parts(canonical, der, info),
            issuer,
        })
    }

    pub fn cert(&self) -> &IssuedCert {
        &self.cert
    }

    /// Verify direct issuance by this CA, without a validity check (expired leaves may renew).
    pub fn verify_server_signature(&self, cert_pem: &str) -> Result<(), PkiError> {
        let der = pem_der(cert_pem, "CERTIFICATE")?;
        let leaf = certificate(&der)?;
        let root = certificate(&self.cert.cert_der)?;
        if leaf.issuer() != root.subject()
            || leaf.verify_signature(Some(root.public_key())).is_err()
        {
            return Err(parse_error(
                "server certificate was not issued by household CA",
            ));
        }
        Ok(())
    }

    /// Export the CA key as PKCS#8 PEM, zeroized when the returned value is dropped.
    pub fn key_pem(&self) -> Zeroizing<String> {
        Zeroizing::new(self.issuer.key().serialize_pem())
    }

    /// Issue a server-only leaf, always including the canonical DNS SAN.
    pub fn issue_server(
        &self,
        sans: &ServerSans,
        now: SystemTime,
        days: u32,
    ) -> Result<CertifiedKey, PkiError> {
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(rcgen_error)?;
        let cert = self.server_for_key(&key, sans, now, days)?;
        Ok(CertifiedKey {
            cert,
            key_pem: Zeroizing::new(key.serialize_pem()),
        })
    }

    /// Re-issue a server leaf without changing its P-256 private key.
    pub fn reissue_server(
        &self,
        key_pem: &str,
        sans: &ServerSans,
        now: SystemTime,
        days: u32,
    ) -> Result<IssuedCert, PkiError> {
        let key = KeyPair::from_pem(key_pem).map_err(rcgen_error)?;
        if key.algorithm() != &PKCS_ECDSA_P256_SHA256 {
            return Err(PkiError::UnsupportedKey);
        }
        self.server_for_key(&key, sans, now, days)
    }

    fn server_for_key(
        &self,
        key: &KeyPair,
        sans: &ServerSans,
        now: SystemTime,
        days: u32,
    ) -> Result<IssuedCert, PkiError> {
        let mut params = parameters(CANONICAL_DNS, now, days)?;
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        for dns in &sans.dns {
            params.subject_alt_names.push(SanType::DnsName(
                dns.clone().try_into().map_err(rcgen_error)?,
            ));
        }
        if !sans
            .dns
            .iter()
            .any(|dns| dns.eq_ignore_ascii_case(CANONICAL_DNS))
        {
            params.subject_alt_names.push(SanType::DnsName(
                CANONICAL_DNS.try_into().map_err(rcgen_error)?,
            ));
        }
        params
            .subject_alt_names
            .extend(sans.ips.iter().copied().map(SanType::IpAddress));
        issued(params.signed_by(key, &self.issuer).map_err(rcgen_error)?)
    }

    /// Validate the CSR, then issue only our own subject and extensions, never its requests.
    pub fn issue_device(
        &self,
        csr_pem: &str,
        device_id: &str,
        now: SystemTime,
        days: u32,
    ) -> Result<IssuedCert, PkiError> {
        let csr = validated_csr(csr_pem)?;
        let mut params = parameters(device_id, now, days)?;
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        issued(
            params
                .signed_by(&csr.key, &self.issuer)
                .map_err(rcgen_error)?,
        )
    }
}

#[derive(Clone, Debug)]
pub struct CertInfo {
    pub fingerprint: [u8; 32],
    pub serial_hex: String,
    pub not_before: SystemTime,
    pub not_after: SystemTime,
    pub common_name: Option<String>,
    pub sans: ServerSans,
    pub is_ca: bool,
}

/// Parse a single certificate PEM, without asserting its trust or verifying its signature.
pub fn parse_cert_pem(pem: &str) -> Result<CertInfo, PkiError> {
    parse_cert_der(&pem_der(pem, "CERTIFICATE")?)
}

/// Parse one complete DER certificate, without asserting trust or verifying its signature.
pub fn parse_cert_der(der: &[u8]) -> Result<CertInfo, PkiError> {
    cert_info(&certificate(der)?, der)
}

fn subject(cn: &str) -> DistinguishedName {
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, cn);
    name.push(DnType::OrganizationName, "Porchlight");
    name
}

fn parameters(cn: &str, now: SystemTime, days: u32) -> Result<CertificateParams, PkiError> {
    let nanos = match now.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    };
    if days == 0 || OffsetDateTime::from_unix_timestamp_nanos(nanos).is_err() {
        return Err(PkiError::InvalidValidity);
    }
    let now = OffsetDateTime::from(now);
    let not_before = now
        .checked_sub(time::Duration::minutes(5))
        .ok_or(PkiError::InvalidValidity)?;
    let not_after = now
        .checked_add(time::Duration::seconds(i64::from(days) * DAY_SECONDS))
        .ok_or(PkiError::InvalidValidity)?;
    let mut params = CertificateParams::default();
    params.distinguished_name = subject(cn);
    params.not_before = not_before;
    params.not_after = not_after;
    params.serial_number = Some(random_serial()?);
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.use_authority_key_identifier_extension = true;
    Ok(params)
}

fn random_serial() -> Result<SerialNumber, PkiError> {
    // No new randomness dependency: rcgen generates a fresh OS-random P-256 key with ring.
    // Hash its public encoding and take 16 bytes; never derive from the CSR (renewals must differ).
    // Clear the sign bit and reject a leading zero to retain a 16-byte positive DER integer.
    loop {
        let entropy = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(rcgen_error)?;
        let mut serial = fingerprint(&entropy.subject_public_key_info())[..16].to_vec();
        serial[0] &= 0x7f;
        if serial[0] != 0 {
            return Ok(SerialNumber::from_slice(&serial));
        }
    }
}

fn pem_der(pem: &str, label: &str) -> Result<Vec<u8>, PkiError> {
    let (remaining, parsed) =
        parse_x509_pem(pem.as_bytes()).map_err(|_| parse_error("invalid PEM"))?;
    if parsed.label != label || !remaining.iter().all(u8::is_ascii_whitespace) {
        return Err(parse_error("unexpected PEM label or trailing data"));
    }
    Ok(parsed.contents)
}

/// Compare a PKCS#8 key with a certificate's complete SubjectPublicKeyInfo.
pub fn key_matches_cert(key_pem: &str, cert_pem: &str) -> Result<bool, PkiError> {
    let key = KeyPair::from_pem(key_pem).map_err(rcgen_error)?;
    let der = pem_der(cert_pem, "CERTIFICATE")?;
    Ok(certificate(&der)?.public_key().raw == key.subject_public_key_info())
}

fn canonical_cert_pem(der: &[u8]) -> String {
    let encoded = STANDARD.encode(der);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in encoded.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    pem
}

fn certificate(der: &[u8]) -> Result<X509Certificate<'_>, PkiError> {
    let (remaining, cert) = x509_parser::parse_x509_certificate(der)
        .map_err(|_| parse_error("invalid certificate DER"))?;
    if !remaining.is_empty() {
        return Err(parse_error("trailing certificate data"));
    }
    Ok(cert)
}

fn common_name(name: &X509Name<'_>) -> Option<String> {
    name.iter_common_name()
        .next()
        .and_then(|cn| cn.as_str().ok())
        .map(str::to_owned)
}

fn cert_info(cert: &X509Certificate<'_>, der: &[u8]) -> Result<CertInfo, PkiError> {
    let mut sans = ServerSans {
        dns: Vec::new(),
        ips: Vec::new(),
    };
    if let Some(san) = cert
        .subject_alternative_name()
        .map_err(|_| parse_error("invalid subject alternative names"))?
    {
        for name in &san.value.general_names {
            match name {
                GeneralName::DNSName(dns) => sans.dns.push((*dns).to_owned()),
                GeneralName::IPAddress(bytes) => {
                    let ip = match bytes.len() {
                        4 => IpAddr::from(
                            <[u8; 4]>::try_from(*bytes)
                                .map_err(|_| parse_error("invalid IPv4 SAN"))?,
                        ),
                        16 => IpAddr::from(
                            <[u8; 16]>::try_from(*bytes)
                                .map_err(|_| parse_error("invalid IPv6 SAN"))?,
                        ),
                        _ => return Err(parse_error("invalid IP SAN length")),
                    };
                    sans.ips.push(ip);
                }
                _ => {}
            }
        }
    }
    let is_ca = cert
        .basic_constraints()
        .map_err(|_| parse_error("invalid basic constraints"))?
        .is_some_and(|bc| bc.value.ca);
    let serial = cert.raw_serial();
    if serial.is_empty() || serial[0] & 0x80 != 0 {
        return Err(parse_error("negative or empty serial"));
    }
    let first = serial
        .iter()
        .position(|b| *b != 0)
        .unwrap_or(serial.len() - 1);
    Ok(CertInfo {
        fingerprint: fingerprint(der),
        serial_hex: hex::encode(&serial[first..]),
        not_before: SystemTime::from(cert.validity().not_before.to_datetime()),
        not_after: SystemTime::from(cert.validity().not_after.to_datetime()),
        common_name: common_name(cert.subject()),
        sans,
        is_ca,
    })
}

fn issued(cert: Certificate) -> Result<IssuedCert, PkiError> {
    let cert_der = cert.der().to_vec();
    let info = parse_cert_der(&cert_der)?;
    Ok(issued_from_parts(cert.pem(), cert_der, info))
}

fn issued_from_parts(cert_pem: String, cert_der: Vec<u8>, info: CertInfo) -> IssuedCert {
    IssuedCert {
        cert_pem,
        cert_der,
        fingerprint: info.fingerprint,
        serial_hex: info.serial_hex,
        not_before: info.not_before,
        not_after: info.not_after,
    }
}
