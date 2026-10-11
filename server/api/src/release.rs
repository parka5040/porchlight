//! Verified, read-only client release. No path from a request is joined to the filesystem.
use crate::{outbound::MAX_BYTES, protocol::ClientVersion, state::AppState};
use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{Read, Seek},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Deserialize)]
pub(crate) struct DigestInfo {
    pub size: u64,
    pub sha256: String,
}
#[derive(Deserialize)]
pub(crate) struct Manifest {
    format_version: u32,
    pub version: String,
    files: BTreeMap<String, DigestInfo>,
}
pub(crate) fn verified_manifest(bytes: &[u8], signature: &str, key: &str) -> Result<Manifest> {
    let key = PublicKey::from_base64(key).map_err(|_| "public_key")?;
    let signature = Signature::decode(signature).map_err(|_| "signature")?;
    key.verify(bytes, &signature, false)
        .map_err(|_| "signature")?;
    let manifest: Manifest = serde_json::from_slice(bytes).map_err(|_| "manifest")?;
    if manifest.format_version != 1 || manifest.files.is_empty() {
        return Err("manifest");
    }
    manifest
        .version
        .parse::<ClientVersion>()
        .map_err(|_| "version")?;
    for (name, info) in &manifest.files {
        if !safe_path(name)
            || !hex_digest(&info.sha256)
            || matches!(name.as_str(), "manifest.json" | "manifest.json.minisig")
        {
            return Err("manifest");
        }
    }
    Ok(manifest)
}
fn hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn safe_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['\\', '\0'])
        && !value.chars().any(char::is_control)
        && value
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}

#[derive(Deserialize)]
struct ReleaseMetadata {
    format_version: u32,
    version: String,
    android: AndroidMetadata,
    desktop: DesktopMetadata,
}
#[derive(Deserialize)]
struct AndroidMetadata {
    apk: String,
    package: String,
    version_code: u64,
    signer_sha256: Vec<String>,
}
#[derive(Deserialize)]
struct DesktopMetadata {
    archive: String,
    files: Vec<String>,
}
#[derive(Clone, Serialize, utoipa::ToSchema)]
pub struct ReleaseFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Serialize, utoipa::ToSchema)]
pub struct AndroidRelease {
    pub file: String,
    pub size: u64,
    pub sha256: String,
    pub package: String,
    pub version_code: u64,
    pub signer_sha256: Vec<String>,
}
#[derive(Clone, Serialize, utoipa::ToSchema)]
pub struct DesktopRelease {
    pub archive: String,
    pub files: Vec<ReleaseFile>,
}
#[derive(Clone, Serialize, utoipa::ToSchema)]
pub struct ClientRelease {
    pub version: String,
    pub android: AndroidRelease,
    pub desktop: DesktopRelease,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileVersion {
    len: u64,
    inode: u64,
    device: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
impl FileVersion {
    fn of(file: &File) -> Result<Self> {
        let meta = file.metadata().map_err(|_| "metadata")?;
        Ok(Self {
            len: meta.len(),
            inode: meta.ino(),
            device: meta.dev(),
            mtime: (meta.mtime(), meta.mtime_nsec()),
            ctime: (meta.ctime(), meta.ctime_nsec()),
        })
    }
}
pub struct Store {
    root: PathBuf,
    verified: BTreeMap<String, Mutex<Option<FileVersion>>>,
    pub advertised: ClientRelease,
    files: BTreeMap<String, DigestInfo>,
    #[cfg(feature = "testing")]
    pub verification_starts: std::sync::atomic::AtomicUsize,
}
impl Store {
    /// The release mount is operator-owned and read-only, as for backup downloads.
    /// Reject symlinks in every component, and never follow the final component on open.
    fn open_member(root: &Path, name: &str) -> Result<File> {
        if !safe_path(name) {
            return Err("path");
        }
        let path = root.join(name);
        if path.ancestors().any(|p| p.is_symlink()) {
            return Err("path");
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| "open")?;
        if !file.metadata().map_err(|_| "metadata")?.is_file() {
            return Err("not_regular");
        }
        Ok(file)
    }
    fn read_text(root: &Path, name: &str) -> Result<Vec<u8>> {
        let file = Self::open_member(root, name)?;
        let mut bytes = Vec::new();
        file.take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "read")?;
        if bytes.len() > MAX_BYTES {
            return Err("too_large");
        }
        Ok(bytes)
    }
    fn digest(bytes: &[u8]) -> DigestInfo {
        DigestInfo {
            size: bytes.len() as u64,
            sha256: hex::encode(Sha256::digest(bytes)),
        }
    }
    fn check_bytes(bytes: &[u8], expected: &DigestInfo) -> Result<()> {
        let actual = Self::digest(bytes);
        if actual.size != expected.size || actual.sha256 != expected.sha256 {
            return Err("file_digest");
        }
        Ok(())
    }
    fn verify_file(file: &mut File, expected: &DigestInfo) -> Result<FileVersion> {
        let before = FileVersion::of(file)?;
        if before.len != expected.size {
            return Err("file_size");
        }
        let mut hash = Sha256::new();
        let mut buf = [0; 65536];
        loop {
            let n = file.read(&mut buf).map_err(|_| "read")?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        if hex::encode(hash.finalize()) != expected.sha256 {
            return Err("file_digest");
        }
        if FileVersion::of(file)? != before {
            return Err("file_changed");
        }
        file.rewind().map_err(|_| "seek")?;
        Ok(before)
    }
    fn load(path: &Path, public_key: &str) -> Result<Self> {
        if path.ancestors().any(|p| p.is_symlink()) || !path.is_dir() {
            return Err("directory");
        }
        let root = path.to_owned();
        let manifest_bytes = Self::read_text(&root, "manifest.json")?;
        let signature_bytes = Self::read_text(&root, "manifest.json.minisig")?;
        let signature = std::str::from_utf8(&signature_bytes).map_err(|_| "signature")?;
        let manifest = verified_manifest(&manifest_bytes, signature, public_key)?;
        let metadata_bytes = Self::read_text(&root, "release.json")?;
        Self::check_bytes(
            &metadata_bytes,
            manifest
                .files
                .get("release.json")
                .ok_or("release_metadata")?,
        )?;
        let metadata: ReleaseMetadata =
            serde_json::from_slice(&metadata_bytes).map_err(|_| "release_metadata")?;
        if metadata.format_version != 1
            || metadata.version != porchlight_core::VERSION
            || metadata.version != manifest.version
        {
            return Err("version");
        }
        let ClientVersion([major, minor, patch]) =
            metadata.version.parse().map_err(|_| "version")?;
        let code = major
            .checked_mul(10000)
            .and_then(|v| minor.checked_mul(100).and_then(|m| v.checked_add(m)))
            .and_then(|v| v.checked_add(patch));
        if minor > 99
            || patch > 99
            || code != Some(metadata.android.version_code)
            || !(1..=2_100_000_000).contains(&metadata.android.version_code)
            || metadata.android.package != "app.porchlight"
            || metadata.android.signer_sha256.is_empty()
            || !metadata.android.signer_sha256.iter().all(|s| hex_digest(s))
            || metadata
                .android
                .signer_sha256
                .windows(2)
                .any(|w| w[0] >= w[1])
        {
            return Err("android_metadata");
        }
        let mut names: BTreeSet<_> = metadata.desktop.files.iter().cloned().collect();
        if names.len() != metadata.desktop.files.len()
            || !names.contains(&metadata.desktop.archive)
            || !names.contains("release.json")
        {
            return Err("desktop_metadata");
        }
        names.insert(metadata.android.apk.clone());
        let mut files = BTreeMap::new();
        let mut verified = BTreeMap::new();
        for name in names {
            let info = manifest.files.get(&name).ok_or("unlisted_file")?;
            let mut file = Self::open_member(&root, &name)?;
            let version = Self::verify_file(&mut file, info)?;
            verified.insert(name.clone(), Mutex::new(Some(version)));
            files.insert(name, info.clone());
        }
        // These small signed metadata files are first verified on download against the bytes read above.
        verified.insert("manifest.json".into(), Mutex::new(None));
        verified.insert("manifest.json.minisig".into(), Mutex::new(None));
        files.insert("manifest.json".into(), Self::digest(&manifest_bytes));
        files.insert(
            "manifest.json.minisig".into(),
            Self::digest(&signature_bytes),
        );
        let mut desktop_names: BTreeSet<_> = metadata.desktop.files.into_iter().collect();
        desktop_names.extend(["manifest.json".into(), "manifest.json.minisig".into()]);
        let desktop = DesktopRelease {
            archive: metadata.desktop.archive,
            files: desktop_names
                .into_iter()
                .map(|path| {
                    let info = &files[&path];
                    ReleaseFile {
                        path,
                        size: info.size,
                        sha256: info.sha256.clone(),
                    }
                })
                .collect(),
        };
        let apk = &files[&metadata.android.apk];
        let android = AndroidRelease {
            file: metadata.android.apk,
            size: apk.size,
            sha256: apk.sha256.clone(),
            package: metadata.android.package,
            version_code: metadata.android.version_code,
            signer_sha256: metadata.android.signer_sha256,
        };
        Ok(Self {
            root,
            files,
            verified,
            #[cfg(feature = "testing")]
            verification_starts: std::sync::atomic::AtomicUsize::new(0),
            advertised: ClientRelease {
                version: metadata.version,
                android,
                desktop,
            },
        })
    }
    pub(crate) fn info(&self, name: &str) -> Result<&DigestInfo> {
        self.files.get(name).ok_or("unlisted_file")
    }
    pub(crate) fn open(&self, name: &str) -> Result<(File, u64, String)> {
        let info = self.info(name)?;
        let mut file = Self::open_member(&self.root, name)?;
        // One cache entry per allowlisted path; simultaneous opens share verification.
        let mut verified = self.verified[name].lock().map_err(|_| "cache_poisoned")?;
        let current = FileVersion::of(&file)?;
        if *verified != Some(current) {
            *verified = None;
            #[cfg(feature = "testing")]
            self.verification_starts
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            *verified = Some(Self::verify_file(&mut file, info)?);
        }
        Ok((file, info.size, info.sha256.clone()))
    }
}

/// Runs once, after trust/listener startup; hashing never blocks an async executor.
pub async fn initialize(state: &AppState) {
    state
        .client_release
        .get_or_init(|| async {
            let (Some(dir), Some(key)) = (
                &state.settings.release_dir,
                &state.settings.release_public_key,
            ) else {
                return None;
            };
            let (dir, key) = (dir.clone(), key.clone());
            let permit = state
                .release_hash_slots
                .clone()
                .acquire_owned()
                .await
                .ok()?;
            let result = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                Store::load(&dir, &key)
            })
            .await
            .unwrap_or(Err("worker"));
            match result {
                Ok(store) => Some(Arc::new(store)),
                Err(reason) => {
                    tracing::warn!(reason, "client release unavailable");
                    None
                }
            }
        })
        .await;
}
