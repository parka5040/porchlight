//! Real TLS enrollment: verify the URI's sole root pin before transmitting a password or token.
use crate::client::{self, DEADLINE, Error, Result};
use porchlight_core::auth::enroll_uri::EnrollUri;
use porchlight_pki::{PinnedRootVerifier, enrollment_target};
use rustls_pki_types::ServerName;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use zeroize::Zeroizing;

#[derive(Deserialize)]
pub struct Session {
    #[serde(deserialize_with = "secret_string")]
    pub token: Zeroizing<String>,
    pub expires_at: String,
}
#[derive(Deserialize)]
pub struct Enrollment {
    pub device_id: String,
    pub certificate: String,
    pub ca_certificate: String,
    pub session: Session,
    pub endpoints: Value,
}
impl std::fmt::Debug for Enrollment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Enrollment")
            .field("device_id", &self.device_id)
            .field("session", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Session([REDACTED])")
    }
}

fn secret_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Zeroizing<String>, D::Error> {
    String::deserialize(deserializer).map(Zeroizing::new)
}

fn tls_error(error: std::io::Error) -> Error {
    // Do not display an arbitrary IO error; only rustls's secret-free error kind.
    match error
        .get_ref()
        .and_then(|e| e.downcast_ref::<rustls::Error>())
    {
        Some(kind) => Error::Message(format!("TLS handshake failed: {kind}")),
        None => Error::Local("TLS handshake failed (transport error)"),
    }
}

pub fn target(uri: &EnrollUri, override_addr: Option<&str>) -> Result<(String, String)> {
    let (name, addr) =
        enrollment_target(&uri.host).map_err(|_| Error::Local("Invalid enrollment host"))?;
    Ok((name, override_addr.unwrap_or(&addr).to_owned()))
}

pub async fn connect(
    uri: &EnrollUri,
    override_addr: Option<&str>,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let (name, addr) = target(uri, override_addr)?;
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|_| Error::Local("TLS configuration failed"))?
    .dangerous()
    .with_custom_certificate_verifier(PinnedRootVerifier::new(uri.ca_fingerprint))
    .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config.resumption = rustls::client::Resumption::disabled();
    let tcp = tokio::time::timeout(DEADLINE, TcpStream::connect(addr))
        .await
        .map_err(|_| Error::Local("Enrollment connection timed out"))?
        .map_err(|_| Error::Local("Could not connect to enrollment listener"))?;
    let name = ServerName::try_from(name).map_err(|_| Error::Local("Invalid TLS server name"))?;
    let tls = tokio::time::timeout(
        DEADLINE,
        TlsConnector::from(Arc::new(config)).connect(name, tcp),
    )
    .await
    .map_err(|_| Error::Local("Enrollment TLS handshake timed out"))?
    .map_err(tls_error)?;
    Ok(tls)
}

pub async fn complete(
    tls: tokio_rustls::client::TlsStream<TcpStream>,
    uri: &EnrollUri,
    pending: &crate::pending_enrollment::Pending,
    password: &str,
) -> Result<Enrollment> {
    // Pending identity is durable, and pinned TLS succeeded before constructing application data.
    let mut request = client::request(
        "POST",
        "/v1/enrollment/complete",
        pending.body(uri, password),
        None,
        &uri.host,
    )?;
    request.headers_mut().insert(
        "idempotency-key",
        pending
            .idempotency_key
            .parse()
            .map_err(|_| Error::Local("Invalid pending enrollment identity"))?,
    );
    let response = client::send(tls, request).await?;
    serde_json::from_value(response).map_err(|_| Error::Local("Invalid enrollment response"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use porchlight_pki::generate_csr;
    use serde_json::json;
    #[test]
    fn session_token_is_zeroizing_and_debug_redacted() {
        let session: Session =
            serde_json::from_value(json!({"token":"not-for-display", "expires_at":"test expiry"}))
                .unwrap();
        let _: &Zeroizing<String> = &session.token;
        assert!(session.token.as_str() == "not-for-display");
        assert!(!format!("{session:?}").contains("not-for-display"));
        let (_, key) = generate_csr("test").unwrap();
        let _: &Zeroizing<String> = &key;
    }

    #[test]
    fn tls_errors_include_rustls_kind_not_arbitrary_io_details() {
        let error = tls_error(std::io::Error::other(rustls::Error::InvalidCertificate(
            rustls::CertificateError::UnknownIssuer,
        )));
        let Error::Message(message) = error else {
            panic!("Missing rustls certificate diagnostic")
        };
        assert!(message.contains("invalid peer certificate"));
        assert!(message.contains("UnknownIssuer"));
        let transport = tls_error(std::io::Error::other("never-show-transport-details"));
        assert!(matches!(
            transport,
            Error::Local("TLS handshake failed (transport error)")
        ));
    }

    #[test]
    fn enrollment_target_default_port() {
        for (host, name, addr) in [
            ("localhost", "localhost", "localhost:8444"),
            ("localhost:12345", "localhost", "localhost:12345"),
            ("[::1]", "::1", "[::1]:8444"),
        ] {
            let uri = EnrollUri {
                host: host.into(),
                token: "redacted".into(),
                ca_fingerprint: [0; 32],
            };
            assert_eq!(target(&uri, None).unwrap(), (name.into(), addr.into()));
            assert_eq!(
                target(&uri, Some("127.0.0.1:9000")).unwrap(),
                (name.into(), "127.0.0.1:9000".into())
            );
        }
    }
}
