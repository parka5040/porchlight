//! Verification follows the opened inode, never a pathname or range offset.
use crate::error::{ApiError, Problem};
use sha2::{Digest, Sha256};
use std::{collections::VecDeque, os::unix::fs::MetadataExt, sync::Mutex};
use std::{
    fs::File,
    io::{Read, Seek},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};
use uuid::Uuid;

// Bounded across households/publications. Metadata comes from the opened descriptor,
// including ctime so even a same-size rewrite with restored mtime must be rehashed.
#[derive(PartialEq, Eq)]
struct Identity {
    publication: Uuid,
    sha: String,
    inode: (u64, u64),
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl Identity {
    fn new(publication: Uuid, sha: &str, meta: &std::fs::Metadata) -> Self {
        Self {
            publication,
            sha: sha.to_owned(),
            inode: (meta.dev(), meta.ino()),
            size: meta.len(),
            modified: (meta.mtime(), meta.mtime_nsec()),
            changed: (meta.ctime(), meta.ctime_nsec()),
        }
    }
}
static VERIFIED: Mutex<VecDeque<Identity>> = Mutex::new(VecDeque::new());

pub(super) fn open(path: &Path, publication: Uuid, size: u64, sha: &str) -> Result<File, ApiError> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| ApiError::new(Problem::NotFound))?;
    verify(&mut file, publication, size, sha)?;
    Ok(file)
}
fn verify(file: &mut File, publication: Uuid, size: u64, sha: &str) -> Result<bool, ApiError> {
    let meta = file
        .metadata()
        .map_err(|_| ApiError::internal("backup_metadata"))?;
    if !meta.is_file() || meta.len() != size {
        return Err(ApiError::new(Problem::Conflict));
    }
    let identity = Identity::new(publication, sha, &meta);
    file.rewind()
        .map_err(|_| ApiError::internal("backup_seek"))?;
    if VERIFIED
        .lock()
        .map_err(|_| ApiError::internal("backup_cache"))?
        .contains(&identity)
    {
        return Ok(true);
    }
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|_| ApiError::internal("backup_read"))?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    if hex::encode(hash.finalize()) != sha {
        return Err(ApiError::new(Problem::Conflict));
    }
    let after = file
        .metadata()
        .map_err(|_| ApiError::internal("backup_metadata"))?;
    if Identity::new(publication, sha, &after) != identity {
        return Err(ApiError::new(Problem::Conflict));
    }
    let mut verified = VERIFIED
        .lock()
        .map_err(|_| ApiError::internal("backup_cache"))?;
    if verified.len() == 32 {
        verified.pop_front();
    }
    verified.push_back(identity);
    file.rewind()
        .map_err(|_| ApiError::internal("backup_seek"))?;
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn audit_bl_server2_reuses_only_unchanged_verified_publication() {
        let dir = tempfile::tempdir_in(concat!(env!("CARGO_MANIFEST_DIR"), "/../../var")).unwrap();
        let path = dir.path().join("bundle");
        std::fs::write(&path, b"verified").unwrap();
        let id = Uuid::now_v7();
        let sha = hex::encode(Sha256::digest(b"verified"));
        let mut first = File::open(&path).unwrap();
        assert!(!verify(&mut first, id, 8, &sha).unwrap());
        let mut resume = File::open(&path).unwrap();
        assert!(
            verify(&mut resume, id, 8, &sha).unwrap(),
            "unchanged range resume rehashed the bundle"
        );
        assert!(!verify(&mut resume, Uuid::now_v7(), 8, &sha).unwrap());
        let replacement = dir.path().join("replacement");
        std::fs::write(&replacement, b"tampered").unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert!(
            open(&path, id, 8, &sha).is_err(),
            "replacement inode reused verification"
        );
        assert!(
            verify(&mut first, id, 8, &sha).is_ok(),
            "held verified inode changed with pathname"
        );
        std::fs::write(&path, b"verified").unwrap();
        let mut same_inode = File::open(&path).unwrap();
        verify(&mut same_inode, id, 8, &sha).unwrap();
        let mut writer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        writer.write_all(b"tampered").unwrap();
        assert!(
            open(&path, id, 8, &sha).is_err(),
            "in-place change reused verification"
        );
        assert!(open(&path, id, 7, &sha).is_err());
    }
}
