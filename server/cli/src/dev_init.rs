//! Development credentials are generated through the production enrollment protocol, not direct issuance.
use crate::{
    client::{Client, Error, Result, field},
    enroll,
    password::password,
    pending_enrollment::{self, Pending},
};
use porchlight_core::auth::{enroll_uri::EnrollUri, session::Platform};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs::{self, DirBuilder, OpenOptions},
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct DeviceSpec {
    label: String,
    platform: Platform,
}
impl std::str::FromStr for DeviceSpec {
    type Err = &'static str;
    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let (label, platform) = value.split_once(':').ok_or("Expected label:platform")?;
        if label.is_empty()
            || label.len() > 60
            || label == "."
            || label == ".."
            || !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        {
            return Err(
                "Device labels must be safe directory names (1-60 ASCII letters, digits, -, _, .)",
            );
        }
        Ok(Self {
            label: label.into(),
            platform: Platform::parse(platform).ok_or("Platform must be android or desktop")?,
        })
    }
}

// Only directories this invocation created are cleaned up. Completed credentials survive a later failure.
struct ReservedDir {
    path: PathBuf,
    complete: bool,
    _lock: fs::File,
}
impl Drop for ReservedDir {
    fn drop(&mut self) {
        if !self.complete {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
fn write_file(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.is_file()
            || metadata.permissions().mode() & 0o777 != mode
            || fs::read(path).map_err(|_| Error::Local("Could not inspect credential file"))?
                != contents
        {
            return Err(Error::Local(
                "Refusing to overwrite a different credential file",
            ));
        }
        return fs::File::open(path)
            .and_then(|f| f.sync_all())
            .map_err(|_| Error::Local("Could not sync credential file"));
    }
    // The directory lock protects staging cleanup; a crash cannot publish partial credentials.
    let staged = path.with_file_name(format!(
        "{}.new",
        path.file_name().unwrap().to_string_lossy()
    ));
    match fs::remove_file(&staged) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(Error::Local("Could not clear credential staging file")),
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&staged)
        .map_err(|_| Error::Local("Could not create credential file"))?;
    // Explicit modes, independent of the caller's umask. The create mode is never more permissive.
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|_| Error::Local("Could not set credential file permissions"))?;
    file.write_all(contents)
        .and_then(|_| file.sync_all())
        .map_err(|_| Error::Local("Could not write credential file"))?;
    fs::hard_link(&staged, path)
        .and_then(|_| fs::remove_file(&staged))
        .map_err(|_| Error::Local("Could not publish credential file"))
}

pub async fn run(
    client: &Client,
    out: &Path,
    override_addr: Option<&str>,
    user: &str,
    devices: &[DeviceSpec],
) -> Result<Value> {
    let mut labels = HashSet::new();
    for device in devices {
        if !labels.insert(&device.label) {
            return Err(Error::Local("Duplicate device label"));
        }
        match fs::symlink_metadata(out.join(&device.label)) {
            Ok(_) => {
                if !out.join(&device.label).join("pending.json").exists() {
                    return Err(Error::Message(format!(
                        "Refusing to overwrite existing device directory: {}",
                        device.label
                    )));
                }
                // Loading may sanitize a legacy attempt. Do that only after the
                // directory lock below, never during this read-only preflight.
                pending_enrollment::private(&out.join(&device.label), true)?;
                pending_enrollment::private(&out.join(&device.label).join("pending.json"), false)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::Local("Could not inspect device directory")),
        }
    }
    let password = password()?;
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(out)
        .map_err(|_| Error::Local("Could not create output directory"))?;
    let metadata = fs::symlink_metadata(out)
        .map_err(|_| Error::Local("Could not inspect output directory"))?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o022 != 0 {
        return Err(Error::Local(
            "Output directory must be real and not writable by other users",
        ));
    }
    let mut reserved = Vec::new();
    // create_dir (not create_dir_all) atomically reserves each label and rejects races/links.
    for device in devices {
        let path = out.join(&device.label);
        let resumed = path.exists();
        if !resumed {
            DirBuilder::new()
                .mode(0o700)
                .create(&path)
                .map_err(|_| Error::Local("Could not reserve device directory"))?;
        }
        pending_enrollment::private(&path, true)?;
        let lock =
            fs::File::open(&path).map_err(|_| Error::Local("Could not open device directory"))?;
        lock.try_lock()
            .map_err(|_| Error::Local("Another enrollment is using this device directory"))?;
        if resumed {
            Pending::load(&path)?;
        }
        reserved.push(ReservedDir {
            path,
            complete: resumed,
            _lock: lock,
        });
    }

    let mut output = Vec::new();
    let mut progress = Progress::new(devices);
    for (device, dir) in devices.iter().zip(&mut reserved) {
        let resumed = dir.complete;
        let pending = if resumed {
            Some(Pending::load(&dir.path)?)
        } else {
            None
        };
        let uri = if let Some(ref pending) = pending {
            EnrollUri::parse(&pending.uri)
        } else {
            let token = client.enrollment_token(user, &device.label).await?;
            EnrollUri::parse(field(&token, "uri")?)
        }
        .map_err(|_| Error::Local("Invalid enrollment URI"))?;
        if let Some(ref p) = pending {
            p.validate(
                &uri,
                user,
                &device.label,
                device.platform.as_str(),
                &password,
            )?;
        }
        let tls = enroll::connect(&uri, override_addr).await?;
        let pending = match pending {
            Some(p) => p,
            None => {
                let p = Pending::new(
                    &uri,
                    user,
                    &device.label,
                    device.platform.as_str(),
                    &password,
                )?;
                p.save(&dir.path)?;
                p
            }
        };
        dir.complete = true; // From now on an ambiguous outcome must retain the key and retry identity.
        let enrollment = match enroll::complete(tls, &uri, &pending, &password).await {
            Ok(e) => e,
            Err(e) => {
                // A fresh attempt with a definite rejection cannot have issued credentials.
                // A resumed attempt may have committed earlier: never discard its identity.
                let rejected = match &e {
                    Error::Problem(p) | Error::RetryableProblem(p, _) => p["status"]
                        .as_u64()
                        .is_some_and(|s| (400..500).contains(&s)),
                    _ => false,
                };
                if !resumed && rejected {
                    dir.complete = false;
                } else {
                    eprintln!(
                        "Enrollment outcome unknown for {}. Pending credentials retained; rerun the same --devices {}:{} with the original password. If replay has expired, check and revoke this device before replacing it.",
                        device.label,
                        device.label,
                        device.platform.as_str()
                    );
                }
                return Err(e);
            }
        };
        progress.enrolled.push(device.label.clone());
        write_device(&dir.path, &enrollment, &pending.key, uri.ca_fingerprint)?;
        Pending::finish(&dir.path)?;
        dir.complete = true;
        progress.saved.push(device.label.clone());
        output.push(json!({"label":device.label, "device_id":enrollment.device_id, "expires":enrollment.session.expires_at}));
    }
    progress.complete = true;
    Ok(json!({"devices":output}))
}

// Track remote enrollment separately from durable files, including failures after credential issuance.
struct Progress<'a> {
    devices: &'a [DeviceSpec],
    enrolled: Vec<String>,
    saved: Vec<String>,
    complete: bool,
}
impl<'a> Progress<'a> {
    fn new(devices: &'a [DeviceSpec]) -> Self {
        Self {
            devices,
            enrolled: Vec::new(),
            saved: Vec::new(),
            complete: false,
        }
    }
    fn failure_message(&self) -> String {
        let remaining = self
            .devices
            .iter()
            .filter(|d| !self.enrolled.contains(&d.label))
            .map(|d| format!("{}:{}", d.label, d.platform.as_str()))
            .collect::<Vec<_>>()
            .join(",");
        let mut message = format!(
            "Devices enrolled before failure: {}. Credentials retained for: {}.",
            self.enrolled.join(", "),
            self.saved.join(", ")
        );
        if !remaining.is_empty() {
            message.push_str(&format!(" Rerun with --devices {remaining}."));
        }
        if self.enrolled != self.saved {
            message.push_str(" Credentials for the last enrolled device were not saved; its pending attempt is retained. Retry that device with its original identity and password. If replay has expired, revoke it before enrolling a replacement.");
        }
        message
    }
}
impl Drop for Progress<'_> {
    fn drop(&mut self) {
        if !self.complete && !self.enrolled.is_empty() {
            eprintln!("{}", self.failure_message());
        }
    }
}

fn write_device(
    path: &Path,
    enrollment: &enroll::Enrollment,
    key: &str,
    pin: [u8; 32],
) -> Result<()> {
    // The verified handshake does not guarantee that the JSON response contains the same root.
    let ca = porchlight_pki::parse_cert_pem(&enrollment.ca_certificate)
        .map_err(|_| Error::Local("Invalid returned CA certificate"))?;
    if ca.fingerprint != pin {
        return Err(Error::Local(
            "Returned CA certificate does not match pinned fingerprint",
        ));
    }
    write_file(&path.join("client.key"), key.as_bytes(), 0o600)?;
    write_file(
        &path.join("client.crt"),
        enrollment.certificate.as_bytes(),
        0o644,
    )?;
    write_file(
        &path.join("ca.crt"),
        enrollment.ca_certificate.as_bytes(),
        0o644,
    )?;
    write_file(
        &path.join("session.token"),
        enrollment.session.token.as_bytes(),
        0o600,
    )?;
    let metadata = json!({"device_id":enrollment.device_id, "endpoints":enrollment.endpoints, "expires":enrollment.session.expires_at});
    let encoded = serde_json::to_vec_pretty(&metadata)
        .map_err(|_| Error::Local("Could not encode enrollment metadata"))?;
    write_file(&path.join("enrollment.json"), &encoded, 0o644)?;
    fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error::Local("Could not sync device directory"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn runtime_dir() -> tempfile::TempDir {
        tempfile::tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var")).unwrap()
    }

    #[test]
    fn returned_ca_must_match_pin_before_any_file_is_written() {
        let ca = porchlight_pki::HouseholdCa::generate("cli test", std::time::SystemTime::now())
            .unwrap();
        let mut enrollment = enroll::Enrollment {
            device_id: "test-device".into(),
            certificate: "test certificate".into(),
            ca_certificate: ca.cert().cert_pem.clone(),
            session: enroll::Session {
                token: zeroize::Zeroizing::new("test session".into()),
                expires_at: "test expiry".into(),
            },
            endpoints: json!([]),
        };
        let pin = porchlight_pki::parse_cert_pem(&enrollment.ca_certificate)
            .unwrap()
            .fingerprint;
        let mut wrong_pin = pin;
        wrong_pin[0] ^= 1;
        let dir = runtime_dir();
        assert!(matches!(
            write_device(dir.path(), &enrollment, "test key", wrong_pin),
            Err(Error::Local(
                "Returned CA certificate does not match pinned fingerprint"
            ))
        ));
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
        let valid_ca = enrollment.ca_certificate.clone();
        enrollment.ca_certificate = "malformed certificate".into();
        assert!(matches!(
            write_device(dir.path(), &enrollment, "test key", pin),
            Err(Error::Local("Invalid returned CA certificate"))
        ));
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
        enrollment.ca_certificate = valid_ca;
        write_device(dir.path(), &enrollment, "test key", pin).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("ca.crt")).unwrap(),
            enrollment.ca_certificate
        );
    }

    #[test]
    fn failed_file_write_distinguishes_enrolled_from_saved_devices() {
        let devices = [
            "first:android".parse().unwrap(),
            "second:desktop".parse().unwrap(),
            "third:desktop".parse().unwrap(),
        ];
        let mut progress = Progress::new(&devices);
        progress.enrolled = vec!["first".into(), "second".into()];
        progress.saved = vec!["first".into()];
        let message = progress.failure_message();
        assert!(message.contains("Devices enrolled before failure: first, second"));
        assert!(message.contains("Credentials retained for: first."));
        assert!(message.contains("--devices third:desktop"));
        assert!(message.contains("revoke it before enrolling a replacement"));
        progress.complete = true; // Do not print test diagnostics from Drop.
    }

    #[test]
    fn labels_cannot_escape_output() {
        for value in [
            "../escape:android",
            "/absolute:desktop",
            ".:android",
            "..:android",
            "x:ios",
            "x:desktop:android",
            ":android",
        ] {
            assert!(value.parse::<DeviceSpec>().is_err());
        }
        assert!("safe-name_1.2:android".parse::<DeviceSpec>().is_ok());
    }
}
