//! Durable, private data-directory storage.
use anyhow::{Context, bail, ensure};
use porchlight_pki::{HouseholdCa, ServerSans, key_matches_cert, parse_cert_pem};
use sqlx::PgPool;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use zeroize::Zeroizing;

pub struct DataDir {
    pub root: PathBuf,
}
impl DataDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn ca_cert(&self) -> PathBuf {
        self.root.join("pki/ca.crt")
    }
    pub fn ca_key(&self) -> PathBuf {
        self.root.join("pki/ca.key")
    }
    pub fn server_cert(&self) -> PathBuf {
        self.root.join("pki/server.crt")
    }
    pub fn server_key(&self) -> PathBuf {
        self.root.join("pki/server.key")
    }
    pub fn idempotency_key(&self) -> PathBuf {
        self.root.join("secrets/idempotency.key")
    }
    pub fn backup_recipient(&self) -> PathBuf {
        self.root.join("backup-recipient.txt")
    }
    pub fn is_initialized(&self) -> bool {
        self.ca_cert().exists()
    }
}
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}
pub(crate) fn private_dir(dir: &Path) -> io::Result<()> {
    if !dir.exists() {
        if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty())
            && !parent.exists()
        {
            private_dir(parent)?;
        }
        match fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
            sync_dir(parent)?;
        }
    }
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(io::Error::other(
            "secret directory must be a real directory",
        ));
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    sync_dir(dir)
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
/// Atomic replacement, or atomic no-clobber publication with a hard link.
pub fn write_secret_file(path: &Path, bytes: &[u8], create_new: bool) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| io::Error::other("secret path needs a parent"))?;
    if create_new && path.symlink_metadata().is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "secret file already exists",
        ));
    }
    private_dir(parent)?;
    let tmp = Temporary(parent.join(format!(".write-{}", uuid::Uuid::now_v7())));
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp.0)?;
    f.set_permissions(fs::Permissions::from_mode(0o600))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    if create_new {
        fs::hard_link(&tmp.0, path)?;
        fs::remove_file(&tmp.0)?;
    } else {
        fs::rename(&tmp.0, path)?;
    }
    sync_dir(parent)
}
pub struct ServerPki {
    pub ca: HouseholdCa,
    pub server_cert_pem: String,
    pub server_key_pem: Zeroizing<String>,
}
impl std::fmt::Debug for ServerPki {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerPki")
            .field("ca", &self.ca)
            .field("server_key_pem", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}
pub(crate) fn read_secret_text(path: &Path) -> anyhow::Result<String> {
    fs::read_to_string(path).with_context(|| {
        format!(
            "reading {}; run `porchlight init` to initialize the data directory",
            path.display()
        )
    })
}
/// Read and authenticate the stored PKI without changing any files.
/// Serve must verify_install on this result before attempting renewal.
pub fn load(dir: &DataDir) -> anyhow::Result<ServerPki> {
    let ca_cert = read_secret_text(&dir.ca_cert())?;
    let ca_key = Zeroizing::new(read_secret_text(&dir.ca_key())?);
    let ca = HouseholdCa::from_pem(&ca_cert, &ca_key)?;
    let server_cert_pem = read_secret_text(&dir.server_cert())?;
    let server_key_pem = Zeroizing::new(read_secret_text(&dir.server_key())?);
    ensure!(
        key_matches_cert(&server_key_pem, &server_cert_pem)?,
        "server certificate and key do not match"
    );
    ca.verify_server_signature(&server_cert_pem)?;
    Ok(ServerPki {
        ca,
        server_cert_pem,
        server_key_pem,
    })
}
/// Apply the same DNS validation, canonical name and deduplication to init and reissue.
pub(crate) fn normalize_server_sans(sans: &mut ServerSans) -> anyhow::Result<()> {
    for dns in &mut sans.dns {
        ensure!(
            !dns.is_empty()
                && dns.len() <= 253
                && dns.split('.').all(|label| !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')),
            "invalid DNS SAN"
        );
        dns.make_ascii_lowercase();
    }
    sans.dns.push("porchlight.lan".into());
    sans.dns.sort();
    sans.dns.dedup();
    sans.ips.sort();
    sans.ips.dedup();
    Ok(())
}

#[derive(Clone, Debug)]
pub struct ReissuedCert {
    pub previous: ServerSans,
    pub current: ServerSans,
    pub not_after: SystemTime,
}

/// Replace only the server leaf, retaining its key and the household CA.
/// SANs replace the old set (with init's mandatory canonical name).
/// Callers with a database must verify_install before invoking this operation.
pub fn reissue_server_cert(
    dir: &DataDir,
    sans: &ServerSans,
    now: SystemTime,
) -> anyhow::Result<ReissuedCert> {
    ensure!(
        !sans.dns.is_empty() || !sans.ips.is_empty(),
        "at least one SAN is required"
    );
    let mut sans = sans.clone();
    normalize_server_sans(&mut sans)?;
    let pki = load(dir)?;
    let previous = parse_cert_pem(&pki.server_cert_pem)?.sans;
    let cert = pki
        .ca
        .reissue_server(&pki.server_key_pem, &sans, now, 730)?;
    let info = parse_cert_pem(&cert.cert_pem)?;
    write_secret_file(&dir.server_cert(), cert.cert_pem.as_bytes(), false)?;
    Ok(ReissuedCert {
        previous,
        current: info.sans,
        not_after: info.not_after,
    })
}

pub fn load_or_renew(dir: &DataDir, now: SystemTime) -> anyhow::Result<(ServerPki, bool)> {
    renew_loaded(dir, load(dir)?, now)
}
/// Renew an already loaded and install-verified PKI, avoiding a second filesystem read.
pub fn renew_loaded(
    dir: &DataDir,
    mut pki: ServerPki,
    now: SystemTime,
) -> anyhow::Result<(ServerPki, bool)> {
    let info = parse_cert_pem(&pki.server_cert_pem)?;
    let renew =
        info.not_after.duration_since(now).unwrap_or_default() < Duration::from_secs(60 * 86400);
    if renew {
        pki.server_cert_pem = pki
            .ca
            .reissue_server(&pki.server_key_pem, &info.sans, now, 730)?
            .cert_pem;
        write_secret_file(&dir.server_cert(), pki.server_cert_pem.as_bytes(), false)?;
    }
    Ok((pki, renew))
}
pub async fn verify_install(pool: &PgPool, pki: &ServerPki) -> anyhow::Result<()> {
    let stored =
        sqlx::query_scalar!("SELECT ca_fingerprint FROM porchlight.install_state WHERE id = true")
            .fetch_optional(pool)
            .await
            .map_err(crate::db::database_error)?;
    ensure!(
        stored.as_deref() == Some(pki.ca.cert().fingerprint.as_slice()),
        "database and data directory are not the same installation"
    );
    Ok(())
}
pub fn load_idempotency_key(dir: &DataDir) -> anyhow::Result<Zeroizing<[u8; 32]>> {
    let text = Zeroizing::new(read_secret_text(&dir.idempotency_key())?);
    if text.len() != 65
        || !text.ends_with('\n')
        || !text.as_bytes()[..64].iter().all(u8::is_ascii_hexdigit)
    {
        bail!("invalid idempotency key file");
    }
    let mut key = Zeroizing::new([0u8; 32]);
    hex::decode_to_slice(&text[..64], &mut *key)
        .map_err(|_| anyhow::anyhow!("invalid idempotency key file"))?;
    Ok(key)
}
