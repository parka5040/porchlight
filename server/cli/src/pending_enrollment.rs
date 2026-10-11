//! Durable, private enrollment identity. Passwords are re-entered, never persisted.
use crate::client::{Error, Result};
use porchlight_core::auth::enroll_uri::EnrollUri;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};
use zeroize::{Zeroize, Zeroizing};

#[cfg(test)]
mod audit_b_final_server;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    pub uri: String,
    pub user: String,
    pub label: String,
    pub platform: String,
    pub csr: String,
    pub key: String,
    pub idempotency_key: String,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.key.zeroize();
        self.uri.zeroize();
    }
}
impl Pending {
    pub fn new(
        uri: &EnrollUri,
        user: &str,
        label: &str,
        platform: &str,
        _password: &str,
    ) -> Result<Self> {
        let (csr, key) = porchlight_pki::generate_csr(label)
            .map_err(|_| Error::Local("Could not generate enrollment key and CSR"))?;
        Ok(Self {
            uri: uri.to_uri(),
            user: user.into(),
            label: label.into(),
            platform: platform.into(),
            csr,
            key: key.to_string(),
            idempotency_key: uuid::Uuid::now_v7().to_string(),
        })
    }
    pub fn body(&self, uri: &EnrollUri, password: &str) -> Value {
        json!({"token":uri.token,"password":password,"csr":self.csr,"platform":self.platform})
    }
    pub fn validate(
        &self,
        uri: &EnrollUri,
        user: &str,
        label: &str,
        platform: &str,
        _password: &str,
    ) -> Result<()> {
        if self.user != user
            || self.label != label
            || self.platform != platform
            || self.uri != uri.to_uri()
        {
            return Err(Error::Local(
                "Pending enrollment requires the original user, device, platform and enrollment URI",
            ));
        }
        Ok(())
    }
    pub fn load(dir: &Path) -> Result<Self> {
        private(dir, true)?;
        let path = dir.join("pending.json");
        private(&path, false)?;
        let bytes = Zeroizing::new(
            fs::read(&path).map_err(|_| Error::Local("Could not read pending enrollment"))?,
        );
        let invalid = || {
            Error::Local(
                "Invalid pending enrollment; preserve it and check or revoke the device before replacing it",
            )
        };
        let mut value: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        // Old attempts contained a fast password verifier. Ignore it, but preserve
        // the key/CSR and idempotency identity needed to recover a committed request.
        let legacy = value
            .as_object_mut()
            .ok_or_else(invalid)?
            .remove("request_hash")
            .is_some();
        let pending: Self = serde_json::from_value(value).map_err(|_| invalid())?;
        if legacy {
            let sanitized = Zeroizing::new(serde_json::to_vec(&pending).map_err(|_| invalid())?);
            let stage_path = dir.join(format!("pending-sanitized-{}.new", uuid::Uuid::now_v7()));
            let mut staged = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&stage_path)
                .map_err(|_| Error::Local("Could not sanitize pending enrollment"))?;
            let published = staged
                .write_all(&sanitized)
                .and_then(|_| staged.sync_all())
                .and_then(|_| fs::rename(&stage_path, &path));
            if published.is_err() {
                let _ = fs::remove_file(&stage_path);
            }
            published.map_err(|_| Error::Local("Could not publish sanitized enrollment"))?;
            sync(dir)?;
        }
        // Retry cleanup even after a crash following sanitized publication. Only
        // discard a staging alias with the same recovery identity, never another key.
        let old_stage = dir.join("pending.new");
        if old_stage.exists() {
            private(&old_stage, false)?;
            let staged = Zeroizing::new(fs::read(&old_stage).map_err(|_| invalid())?);
            let mut value: Value = serde_json::from_slice(&staged).map_err(|_| invalid())?;
            value
                .as_object_mut()
                .ok_or_else(invalid)?
                .remove("request_hash");
            let previous: Self = serde_json::from_value(value).map_err(|_| invalid())?;
            if serde_json::to_value(&previous).map_err(|_| invalid())?
                == serde_json::to_value(&pending).map_err(|_| invalid())?
            {
                fs::remove_file(old_stage).map_err(|_| invalid())?;
                sync(dir)?;
            }
        }
        Ok(pending)
    }
    pub fn save(&self, dir: &Path) -> Result<()> {
        let bytes = Zeroizing::new(
            serde_json::to_vec(self)
                .map_err(|_| Error::Local("Could not encode pending enrollment"))?,
        );
        let staged = dir.join("pending.new");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staged)
            .map_err(|_| Error::Local("Could not stage pending enrollment"))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .and_then(|_| file.write_all(&bytes))
            .and_then(|_| file.sync_all())
            .map_err(|_| Error::Local("Could not persist pending enrollment"))?;
        // No request is sent until publication and both directories are durable.
        fs::hard_link(&staged, dir.join("pending.json"))
            .and_then(|_| fs::remove_file(staged))
            .map_err(|_| Error::Local("Could not publish pending enrollment"))?;
        sync(dir)?;
        if let Some(parent) = dir.parent() {
            sync(parent)?;
        }
        Ok(())
    }
    pub fn finish(dir: &Path) -> Result<()> {
        fs::remove_file(dir.join("pending.json"))
            .map_err(|_| Error::Local("Could not remove completed enrollment attempt"))?;
        sync(dir)
    }
}
pub fn private(path: &Path, directory: bool) -> Result<()> {
    let m = fs::symlink_metadata(path)
        .map_err(|_| Error::Local("Could not inspect private enrollment path"))?;
    if (directory && (!m.is_dir() || m.permissions().mode() & 0o777 != 0o700))
        || (!directory && (!m.is_file() || m.permissions().mode() & 0o777 != 0o600))
    {
        return Err(Error::Local(
            "Enrollment attempts require a private real directory (0700) and regular file (0600)",
        ));
    }
    Ok(())
}
pub fn sync(path: &Path) -> Result<()> {
    fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error::Local("Could not sync enrollment directory"))
}
