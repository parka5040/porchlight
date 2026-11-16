//! TLS policy shared by all TCP listeners. Revocations are monotonic and checked on every handshake.
use crate::pki_store::ServerPki;
use porchlight_pki::fingerprint;
use rustls::{
    CertificateError, DigitallySignedStruct, DistinguishedName, Error, RootCertStore, ServerConfig,
    SignatureScheme,
    client::danger::HandshakeSignatureValid,
    server::{
        WebPkiClientVerifier,
        danger::{ClientCertVerified, ClientCertVerifier},
    },
};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, UnixTime, pem::PemObject};
use std::{
    collections::HashSet,
    sync::{Arc, RwLock},
};

#[derive(Clone, Default, Debug)]
pub struct RevokedSet {
    fingerprints: Arc<RwLock<HashSet<[u8; 32]>>>,
    published_devices: Arc<RwLock<HashSet<uuid::Uuid>>>,
}
impl RevokedSet {
    /// Shared by the local publisher and watcher: either may win the post-commit race.
    pub(crate) fn claim_device(&self, id: uuid::Uuid) -> bool {
        self.published_devices
            .write()
            .expect("revocation lock poisoned")
            .insert(id)
    }

    pub fn extend(&self, fps: impl IntoIterator<Item = [u8; 32]>) {
        self.fingerprints
            .write()
            .expect("revocation lock poisoned")
            .extend(fps);
    }
    pub fn insert(&self, fp: [u8; 32]) {
        self.extend([fp]);
    }
    pub fn contains(&self, fp: &[u8; 32]) -> bool {
        self.fingerprints
            .read()
            .expect("revocation lock poisoned")
            .contains(fp)
    }
    pub fn len(&self) -> usize {
        self.fingerprints
            .read()
            .expect("revocation lock poisoned")
            .len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
#[derive(Debug)]
pub struct RevocationAwareVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    revoked: RevokedSet,
}
impl RevocationAwareVerifier {
    pub fn new(roots: Arc<RootCertStore>, revoked: RevokedSet) -> anyhow::Result<Self> {
        let inner = WebPkiClientVerifier::builder_with_provider(
            roots,
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()?;
        Ok(Self { inner, revoked })
    }
}
impl ClientCertVerifier for RevocationAwareVerifier {
    fn offer_client_auth(&self) -> bool {
        self.inner.offer_client_auth()
    }
    fn client_auth_mandatory(&self) -> bool {
        self.inner.client_auth_mandatory()
    }
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }
    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        if self.revoked.contains(&fingerprint(end_entity.as_ref())) {
            return Err(Error::InvalidCertificate(CertificateError::Revoked));
        }
        self.inner
            .verify_client_cert(end_entity, intermediates, now)
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
    fn requires_raw_public_keys(&self) -> bool {
        self.inner.requires_raw_public_keys()
    }
}
fn chain_and_key(
    pki: &ServerPki,
) -> anyhow::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    Ok((
        vec![
            CertificateDer::from_pem_slice(pki.server_cert_pem.as_bytes())?,
            CertificateDer::from(pki.ca.cert().cert_der.clone()),
        ],
        PrivateKeyDer::from_pem_slice(pki.server_key_pem.as_bytes())?,
    ))
}
fn harden(mut config: ServerConfig) -> Arc<ServerConfig> {
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    config.send_tls13_tickets = 0;
    Arc::new(config)
}
pub fn mtls_server_config(
    pki: &ServerPki,
    revoked: RevokedSet,
) -> anyhow::Result<Arc<ServerConfig>> {
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(pki.ca.cert().cert_der.clone()))?;
    let verifier = RevocationAwareVerifier::new(Arc::new(roots), revoked)?;
    let (chain, key) = chain_and_key(pki)?;
    Ok(harden(
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_client_cert_verifier(Arc::new(verifier))
            .with_single_cert(chain, key)?,
    ))
}
pub fn enroll_server_config(pki: &ServerPki) -> anyhow::Result<Arc<ServerConfig>> {
    let (chain, key) = chain_and_key(pki)?;
    Ok(harden(
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .with_no_client_auth()
            .with_single_cert(chain, key)?,
    ))
}

/// Both TCP policies share a resolver. Renewal atomically replaces only the public leaf;
/// each subsequent handshake loads it with the original key and install-verified CA.
#[derive(Debug)]
struct CurrentCert {
    identity: Option<(std::time::SystemTime, u64, u64)>,
    key: Arc<rustls::sign::CertifiedKey>,
}
#[derive(Debug)]
pub struct ReloadingCert {
    pki: Arc<ServerPki>,
    path: std::path::PathBuf,
    current: RwLock<CurrentCert>,
}
impl ReloadingCert {
    pub fn new(pki: Arc<ServerPki>, path: std::path::PathBuf) -> anyhow::Result<Self> {
        let (chain, key) = chain_and_key(&pki)?;
        let key = rustls::sign::CertifiedKey::from_der(
            chain,
            key,
            &rustls::crypto::ring::default_provider(),
        )?;
        // Force a first read: the file may already have been renewed after AppState was built.
        Ok(Self {
            pki,
            path,
            current: RwLock::new(CurrentCert {
                identity: None,
                key: Arc::new(key),
            }),
        })
    }
    fn certificate(&self) -> anyhow::Result<Arc<rustls::sign::CertifiedKey>> {
        let metadata = match std::fs::metadata(&self.path) {
            Ok(value) => value,
            // In-memory test servers have no disk PKI. Production load() requires it before serving.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(self
                    .current
                    .read()
                    .map_err(|_| anyhow::anyhow!("certificate lock"))?
                    .key
                    .clone());
            }
            Err(_) => anyhow::bail!("certificate metadata"),
        };
        use std::os::unix::fs::MetadataExt;
        let modified = (metadata.modified()?, metadata.ino(), metadata.len());
        let mut current = self
            .current
            .write()
            .map_err(|_| anyhow::anyhow!("certificate lock"))?;
        if current.identity != Some(modified) {
            anyhow::ensure!(
                metadata.is_file() && metadata.len() <= 65536,
                "invalid server certificate file"
            );
            let pem = std::fs::read_to_string(&self.path)?;
            self.pki.ca.verify_server_signature(&pem)?;
            let chain = vec![
                CertificateDer::from_pem_slice(pem.as_bytes())?,
                CertificateDer::from(self.pki.ca.cert().cert_der.clone()),
            ];
            let key = PrivateKeyDer::from_pem_slice(self.pki.server_key_pem.as_bytes())?;
            let key = rustls::sign::CertifiedKey::from_der(
                chain,
                key,
                &rustls::crypto::ring::default_provider(),
            )?;
            *current = CurrentCert {
                identity: Some(modified),
                key: Arc::new(key),
            };
        }
        Ok(current.key.clone())
    }
}
impl rustls::server::ResolvesServerCert for ReloadingCert {
    fn resolve(
        &self,
        _: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        self.certificate().ok()
    }
}

pub fn reloading_server_configs(
    pki: Arc<ServerPki>,
    revoked: RevokedSet,
    path: std::path::PathBuf,
) -> anyhow::Result<(Arc<ServerConfig>, Arc<ServerConfig>)> {
    let resolver = Arc::new(ReloadingCert::new(pki.clone(), path)?);
    let mut mtls = (*mtls_server_config(&pki, revoked)?).clone();
    let mut enroll = (*enroll_server_config(&pki)?).clone();
    mtls.cert_resolver = resolver.clone();
    enroll.cert_resolver = resolver;
    Ok((Arc::new(mtls), Arc::new(enroll)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configs_disable_resumption_and_verifier_reports_revoked() {
        use porchlight_pki::{HouseholdCa, ServerSans, generate_csr};
        let now = std::time::SystemTime::now();
        let ca = HouseholdCa::generate("Test", now).unwrap();
        let server = ca
            .issue_server(
                &ServerSans {
                    dns: vec!["localhost".into()],
                    ips: vec![],
                },
                now,
                730,
            )
            .unwrap();
        let pki = ServerPki {
            ca,
            server_cert_pem: server.cert.cert_pem,
            server_key_pem: server.key_pem,
        };
        let revoked = RevokedSet::default();
        for config in [
            mtls_server_config(&pki, revoked.clone()).unwrap(),
            enroll_server_config(&pki).unwrap(),
        ] {
            assert_eq!(config.send_tls13_tickets, 0);
            assert!(!config.session_storage.can_cache());
            assert_eq!(config.alpn_protocols, [b"http/1.1".to_vec()]);
        }
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(pki.ca.cert().cert_der.clone()))
            .unwrap();
        let verifier = RevocationAwareVerifier::new(Arc::new(roots), revoked.clone()).unwrap();
        assert!(verifier.offer_client_auth());
        assert!(verifier.client_auth_mandatory());
        assert!(!verifier.root_hint_subjects().is_empty());
        assert!(!verifier.supported_verify_schemes().is_empty());
        let (csr, _) = generate_csr("device").unwrap();
        let cert = pki.ca.issue_device(&csr, "device", now, 365).unwrap();
        let der = CertificateDer::from(cert.cert_der);
        assert!(
            verifier
                .verify_client_cert(&der, &[], UnixTime::now())
                .is_ok()
        );
        revoked.insert(cert.fingerprint);
        assert!(matches!(
            verifier.verify_client_cert(&der, &[], UnixTime::now()),
            Err(Error::InvalidCertificate(CertificateError::Revoked))
        ));
    }
    #[test]
    fn revocations_only_grow() {
        let set = RevokedSet::default();
        let clone = set.clone();
        set.insert([1; 32]);
        clone.extend([[2; 32], [1; 32]]);
        set.extend([]);
        assert_eq!(set.len(), 2);
        assert!(clone.contains(&[1; 32]));
    }
}
