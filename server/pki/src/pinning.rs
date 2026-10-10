//! Enrollment trust bootstrap: a pin selects a root, never bypasses normal server validation.
use crate::{PkiError, fingerprint};
use rustls::{
    CertificateError, DigitallySignedStruct, Error, RootCertStore, SignatureScheme,
    client::{
        WebPkiServerVerifier,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    },
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
};
use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
use std::sync::Arc;
#[derive(Debug)]
pub struct PinnedRootVerifier {
    pin: [u8; 32],
    provider: Arc<CryptoProvider>,
}
impl PinnedRootVerifier {
    pub fn new(pin: [u8; 32]) -> Arc<Self> {
        Arc::new(Self {
            pin,
            provider: Arc::new(rustls::crypto::ring::default_provider()),
        })
    }
}
impl ServerCertVerifier for PinnedRootVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let root = std::iter::once(end_entity)
            .chain(intermediates)
            .find(|cert| fingerprint(cert.as_ref()) == self.pin)
            .ok_or(Error::InvalidCertificate(CertificateError::UnknownIssuer))?;
        // Pinning a leaf (even a signed one) is not the enrollment root protocol.
        if !crate::parse_cert_der(root.as_ref())
            .map_err(|_| Error::InvalidCertificate(CertificateError::BadEncoding))?
            .is_ca
        {
            return Err(Error::InvalidCertificate(CertificateError::UnknownIssuer));
        }
        let mut roots = RootCertStore::empty();
        roots.add(root.clone())?;
        let verifier =
            WebPkiServerVerifier::builder_with_provider(Arc::new(roots), self.provider.clone())
                .build()
                .map_err(|_| Error::InvalidCertificate(CertificateError::UnknownIssuer))?;
        // Exclude the selected anchor from intermediates; it is the sole trusted root.
        let chain: Vec<_> = intermediates
            .iter()
            .filter(|cert| fingerprint(cert.as_ref()) != self.pin)
            .cloned()
            .collect();
        verifier.verify_server_cert(end_entity, &chain, server_name, ocsp_response, now)
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
/// URI authority to (unbracketed TLS server name, bracketed socket target).
/// The enrollment listener uses 8444, not HTTPS's default 443.
pub fn enrollment_target(host: &str) -> Result<(String, String), PkiError> {
    let invalid = || PkiError::InvalidEnrollmentTarget;
    if host.is_empty()
        || host
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b"/@?#\\".contains(&b))
    {
        return Err(invalid());
    }
    let (name, port) = if let Some(rest) = host.strip_prefix('[') {
        let (name, suffix) = rest.split_once(']').ok_or_else(invalid)?;
        name.parse::<std::net::Ipv6Addr>().map_err(|_| invalid())?;
        let port = if suffix.is_empty() {
            None
        } else {
            Some(suffix.strip_prefix(':').ok_or_else(invalid)?)
        };
        (name, port)
    } else {
        let mut parts = host.split(':');
        let name = parts.next().ok_or_else(invalid)?;
        let port = parts.next();
        if parts.next().is_some() {
            return Err(invalid());
        }
        (name, port)
    };
    ServerName::try_from(name.to_owned()).map_err(|_| invalid())?;
    let port = match port {
        None => 8444,
        Some(s) => {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                return Err(invalid());
            }
            let p = s.parse::<u16>().map_err(|_| invalid())?;
            if p == 0 {
                return Err(invalid());
            }
            p
        }
    };
    let target = if name.contains(':') {
        format!("[{name}]:{port}")
    } else {
        format!("{name}:{port}")
    };
    Ok((name.to_owned(), target))
}
