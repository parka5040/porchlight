use super::filename_categories::excluded_filename_category;
use super::*;
pub fn sanitize_name(name: &str, mime: &str) -> String {
    let file = name.rsplit(['/', '\\']).next().unwrap_or("attachment");
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    let mut stem: String = stem
        .chars()
        .filter(|c| {
            (!c.is_ascii() && !excluded_filename_category(*c))
                || c.is_ascii_alphanumeric()
                || ['-', '_', ' ', '(', ')', '.'].contains(c)
        })
        .take(120)
        .collect();
    if stem.trim().is_empty() {
        stem = "attachment".into();
    }
    let ext = match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "application/pdf" => "pdf",
        _ => "txt",
    };
    format!("{}.{}", stem.trim(), ext)
}
pub fn blob_path(root: &Path, hash: &str) -> Result<PathBuf, ApiError> {
    Ok(root
        .join("blobs")
        .join(ingest::blob_relative_path(hash).map_err(ingest_error)?))
}
/// Write a same-directory temp, flush it, rename to the validated hash, flush directory.
/// Temps are never returned by downloads/backup and are removed on any ordinary error.
/// Standalone fixture helper; server writes use the shared `DiskBudget` method.
pub fn durable_write(root: &Path, hash: &str, bytes: &[u8]) -> Result<(), ApiError> {
    DiskBudget::default().durable_write(root, hash, bytes)
}
impl DiskBudget {
    pub(super) fn durable_write(
        &self,
        root: &Path,
        hash: &str,
        bytes: &[u8],
    ) -> Result<(), ApiError> {
        let dest = blob_path(root, hash)?;
        let dir = dest.parent().expect("blob parent");
        std::fs::create_dir_all(dir).map_err(|_| ApiError::internal("blob_mkdir"))?;
        // Persist newly-created shard directory links as well as the final rename.
        for p in [
            dir.parent(),
            dir.parent().and_then(Path::parent),
            dir.parent().and_then(Path::parent).and_then(Path::parent),
        ]
        .into_iter()
        .flatten()
        {
            std::fs::File::open(p)
                .and_then(|f| f.sync_all())
                .map_err(|_| ApiError::internal("blob_dir_sync"))?;
        }
        let temp = dir.join(format!(".{}.tmp", Uuid::now_v7()));
        self.change(&[&temp, &dest], || {
            let result = (|| -> std::io::Result<()> {
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&temp)?;
                f.write_all(bytes)?;
                f.sync_all()?;
                std::fs::rename(&temp, &dest)?;
                std::fs::File::open(dir)?.sync_all()
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(&temp);
            }
            result
        })
        .map_err(|_| ApiError::internal("blob_write"))
    }
}

/// RFC 6266 Unicode filename with conservative ASCII fallback (never raw header Unicode).
pub fn content_disposition(disposition: &str, name: &str) -> String {
    let ascii: String = name
        .chars()
        .map(|c| if c.is_ascii() { c } else { '_' })
        .collect();
    let encoded: String = name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("{disposition}; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}
