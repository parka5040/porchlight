//! Authenticate CSR self-signatures and keep only the public P-256 key.
use super::{PkiError, common_name, pem_der, rcgen_error, subject};
use rcgen::{
    CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256, PublicKeyData, SignatureAlgorithm,
};
use x509_parser::{certification_request::X509CertificationRequest, prelude::FromDer};
use zeroize::Zeroizing;

/// Generate a fresh P-256 key and self-signed CSR with the informational requested CN.
pub fn generate_csr(common_name: &str) -> Result<(String, Zeroizing<String>), PkiError> {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(rcgen_error)?;
    let mut params = CertificateParams::default();
    params.distinguished_name = subject(common_name);
    let csr = params
        .serialize_request(&key)
        .map_err(rcgen_error)?
        .pem()
        .map_err(rcgen_error)?;
    Ok((csr, Zeroizing::new(key.serialize_pem())))
}

/// Validate the self-signature and exact P-256/SHA-256 key type; the CN is informational only.
pub fn validate_csr(csr_pem: &str) -> Result<Option<String>, PkiError> {
    Ok(validated_csr(csr_pem)?.common_name)
}

// Extract just the authenticated public key. rcgen's CSR parser also interprets requested
// extensions and rejects unknown ones; that would violate our policy of ignoring *all* requests.
pub(super) struct CsrKey(Vec<u8>);
impl PublicKeyData for CsrKey {
    fn der_bytes(&self) -> &[u8] {
        &self.0
    }
    fn algorithm(&self) -> &'static SignatureAlgorithm {
        &PKCS_ECDSA_P256_SHA256
    }
}

pub(super) struct ValidatedCsr {
    pub(super) key: CsrKey,
    pub(super) common_name: Option<String>,
}

pub(super) fn validated_csr(csr_pem: &str) -> Result<ValidatedCsr, PkiError> {
    let der =
        pem_der(csr_pem, "CERTIFICATE REQUEST").map_err(|_| PkiError::InvalidCsr("invalid PEM"))?;
    let (remaining, csr) = X509CertificationRequest::from_der(&der)
        .map_err(|_| PkiError::InvalidCsr("invalid DER"))?;
    if !remaining.is_empty() {
        return Err(PkiError::InvalidCsr("trailing DER data"));
    }
    let spki = &csr.certification_request_info.subject_pki;
    let curve = spki
        .algorithm
        .parameters
        .as_ref()
        .and_then(|p| p.as_oid().ok());
    if spki.algorithm.algorithm.to_id_string() != "1.2.840.10045.2.1"
        || curve.is_none_or(|oid| oid.to_id_string() != "1.2.840.10045.3.1.7")
        || csr.signature_algorithm.algorithm.to_id_string() != "1.2.840.10045.4.3.2"
    {
        return Err(PkiError::UnsupportedKey);
    }
    csr.verify_signature()
        .map_err(|_| PkiError::InvalidCsr("invalid self-signature"))?;
    Ok(ValidatedCsr {
        key: CsrKey(spki.subject_public_key.data.to_vec()),
        common_name: common_name(&csr.certification_request_info.subject),
    })
}
