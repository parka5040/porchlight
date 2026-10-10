//! Daily, signed release discovery. Failures never replace the last verified version.
use crate::{
    config::Settings, outbound::Fetcher, protocol::ClientVersion, release::verified_manifest,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::{future::Future, time::Duration};
use tokio::sync::RwLock;

#[derive(Clone, Serialize, utoipa::ToSchema)]
pub struct LatestRelease {
    pub version: String,
}
#[derive(Clone, Default)]
struct Checked {
    latest: Option<LatestRelease>,
    checked_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
}
#[derive(Serialize, utoipa::ToSchema)]
pub struct ServerRelease {
    pub running: String,
    pub latest: Option<LatestRelease>,
    pub update_available: bool,
    pub checked_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub upgrade_command: Option<String>,
}
pub struct ReleaseWatch {
    checked: RwLock<Checked>,
    fetcher: Fetcher,
    source: Option<(String, String)>,
}
impl ReleaseWatch {
    pub fn new(settings: &Settings) -> Self {
        Self {
            checked: RwLock::new(Checked::default()),
            fetcher: Fetcher::default(),
            source: settings
                .release_repo
                .as_ref()
                .zip(settings.release_public_key.as_ref())
                .map(|(repo, key)| {
                    (
                        format!("https://github.com/{repo}/releases/latest/download"),
                        key.clone(),
                    )
                }),
        }
    }
    #[cfg(feature = "testing")]
    pub fn local_test(fetcher: Fetcher, base_url: String, public_key: String) -> Self {
        Self {
            checked: RwLock::new(Checked::default()),
            fetcher,
            source: Some((base_url, public_key)),
        }
    }
    pub async fn status(&self, upgrade_command: Option<String>) -> ServerRelease {
        let checked = self.checked.read().await;
        let running: ClientVersion = porchlight_core::VERSION.parse().expect("running version");
        ServerRelease {
            running: running.to_string(),
            latest: checked.latest.clone(),
            update_available: checked.latest.as_ref().is_some_and(|v| {
                v.version
                    .parse::<ClientVersion>()
                    .is_ok_and(|v| v > running)
            }),
            checked_at: checked.checked_at,
            last_error: checked.last_error.clone(),
            upgrade_command,
        }
    }
    async fn check(&self) {
        let Some((base, key)) = &self.source else {
            return;
        };
        let result = async {
            let manifest = self
                .fetcher
                .fetch_release(&format!("{base}/manifest.json"))
                .await
                .map_err(|e| e.0)?;
            let signature = self
                .fetcher
                .fetch_release(&format!("{base}/manifest.json.minisig"))
                .await
                .map_err(|e| e.0)?;
            verified_manifest(manifest.as_bytes(), &signature, key)
        }
        .await;
        let mut checked = self.checked.write().await;
        checked.checked_at = Some(Utc::now());
        match result {
            Ok(manifest) => {
                checked.latest = Some(LatestRelease {
                    version: manifest.version,
                });
                checked.last_error = None;
            }
            Err(reason) => {
                checked.last_error = Some(reason.into());
                tracing::warn!(reason, "release check failed");
            }
        }
    }
    #[cfg(feature = "testing")]
    pub async fn check_for_test(&self) {
        self.check().await;
    }

    pub async fn run(&self, shutdown: impl Future<Output = ()>) -> anyhow::Result<()> {
        tokio::pin!(shutdown);
        if self.source.is_none() {
            shutdown.await;
            return Ok(());
        }
        let mut random = [0; 8];
        // A failed OS entropy read still delays the first request (no startup hot loop).
        let _ = getrandom::fill(&mut random);
        let mut delay = Duration::from_secs(30 + u64::from_le_bytes(random) % 91);
        loop {
            tokio::select! {
                () = shutdown.as_mut() => return Ok(()),
                () = tokio::time::sleep(delay) => {}
            }
            tokio::select! {
                () = shutdown.as_mut() => return Ok(()),
                () = self.check() => {}
            }
            delay = Duration::from_secs(24 * 60 * 60);
        }
    }
}
