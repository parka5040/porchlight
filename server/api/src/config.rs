//! Single source of serve configuration. Secrets are never formatted.
use clap::Args;
use ipnet::IpNet;
use std::{fmt, net::SocketAddr, path::PathBuf, time::Duration};

#[derive(Clone, Debug)]
pub struct Settings {
    pub maintenance_file: PathBuf,
    pub backup_dir: PathBuf,
    pub release_repo: Option<String>,
    pub release_public_key: Option<String>,
    pub release_dir: Option<PathBuf>,
    pub upgrade_command: Option<String>,
    pub data_dir: PathBuf,
    pub attachment_quota_bytes: u64,
    pub disk_reserve_bytes: u64,
    _test_data_guard: Option<std::sync::Arc<tempfile::TempDir>>,
    pub min_client_version: crate::protocol::ClientVersion,
    pub device_cert_days: u32,
    pub device_renew_days: u32,
    pub enroll_host: String,
    pub endpoints: Vec<String>,
    pub heartbeat: Duration,
    pub body_timeout: Duration,
}
impl Settings {
    pub fn for_tests() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../var");
        std::fs::create_dir_all(&root).expect("test runtime root");
        let temp = tempfile::Builder::new()
            .prefix("settings-test-")
            .tempdir_in(root)
            .expect("test data directory");
        Self {
            release_repo: None,
            release_public_key: None,
            release_dir: None,
            upgrade_command: None,
            data_dir: temp.path().to_owned(),
            backup_dir: temp.path().join("backups"),
            attachment_quota_bytes: 10 * 1024 * 1024 * 1024,
            disk_reserve_bytes: 2 * 1024 * 1024 * 1024,
            maintenance_file: temp.path().join("maintenance"),
            _test_data_guard: Some(std::sync::Arc::new(temp)),
            min_client_version: "0.1.0".parse().expect("default version"),
            device_cert_days: 365,
            device_renew_days: 30,
            enroll_host: "localhost".into(),
            endpoints: vec!["https://localhost:8443".into()],
            heartbeat: Duration::from_secs(1),
            body_timeout: Duration::from_secs(1),
        }
    }
}
#[derive(Args, Clone)]
pub struct ServeArgs {
    #[arg(long, env = "PORCHLIGHT_RELEASE_REPO", hide_env_values = true)]
    pub release_repo: Option<String>,
    #[arg(long, env = "PORCHLIGHT_RELEASE_PUBLIC_KEY", hide_env_values = true)]
    pub release_public_key: Option<String>,
    #[arg(long, env = "PORCHLIGHT_RELEASE_DIR")]
    pub release_dir: Option<PathBuf>,
    #[arg(long, env = "PORCHLIGHT_UPGRADE_COMMAND")]
    pub upgrade_command: Option<String>,
    #[arg(long, env = "PORCHLIGHT_BACKUP_DIR", default_value = "/backups")]
    pub backup_dir: PathBuf,
    #[arg(long, env = "PORCHLIGHT_MIN_CLIENT_VERSION", default_value = "0.1.0")]
    pub min_client_version: crate::protocol::ClientVersion,
    #[arg(long, env = "PORCHLIGHT_DATA_DIR", default_value = "/data")]
    pub data_dir: PathBuf,
    #[arg(long, env = "PORCHLIGHT_ATTACHMENT_QUOTA_BYTES", default_value_t = 10 * 1024 * 1024 * 1024)]
    pub attachment_quota_bytes: u64,
    #[arg(long, env = "PORCHLIGHT_DISK_RESERVE_BYTES", default_value_t = 2 * 1024 * 1024 * 1024)]
    pub disk_reserve_bytes: u64,
    #[arg(long, env = "PORCHLIGHT_DATABASE_URL", hide_env_values = true)]
    pub database_url: String,
    #[arg(long, env = "PORCHLIGHT_DB_POOL_SIZE", default_value_t = 16)]
    pub db_pool_size: u32,
    #[arg(long, env = "PORCHLIGHT_MIGRATOR_URL", hide_env_values = true)]
    pub migrator_url: Option<String>,
    #[arg(long, env = "PORCHLIGHT_LAN_ADDR", default_value = "0.0.0.0:8443")]
    pub lan_addr: SocketAddr,
    #[arg(long, env = "PORCHLIGHT_FUNNEL_ADDR")]
    pub funnel_addr: Option<SocketAddr>,
    #[arg(
        long,
        env = "PORCHLIGHT_FUNNEL_PEERS",
        value_delimiter = ',',
        requires = "funnel_addr"
    )]
    pub funnel_peers: Vec<IpNet>,
    #[arg(long, env = "PORCHLIGHT_ENROLL_ADDR", default_value = "0.0.0.0:8444")]
    pub enroll_addr: SocketAddr,
    #[arg(
        long,
        env = "PORCHLIGHT_ENROLL_CIDRS",
        value_delimiter = ',',
        required = true
    )]
    pub enroll_cidrs: Vec<IpNet>,
    #[arg(
        long,
        env = "PORCHLIGHT_AGENT_SOCKET",
        default_value = "/run/porchlight/agent.sock"
    )]
    pub agent_socket: PathBuf,
    /// Defaults to `maintenance` beside the agent socket, in the shared run directory.
    #[arg(long, env = "PORCHLIGHT_MAINTENANCE_FILE")]
    pub maintenance_file: Option<PathBuf>,
    #[arg(long, env = "PORCHLIGHT_DEVICE_CERT_DAYS", default_value_t = 365)]
    pub device_cert_days: u32,
    #[arg(long, env = "PORCHLIGHT_DEVICE_RENEW_DAYS", default_value_t = 30)]
    pub device_renew_days: u32,
    #[arg(long, env = "PORCHLIGHT_ENROLL_HOST")]
    pub enroll_host: String,
    #[arg(
        long,
        env = "PORCHLIGHT_ENDPOINTS",
        value_delimiter = ',',
        default_value = "https://porchlight.lan:8443"
    )]
    pub endpoints: Vec<String>,
    #[arg(long, env = "PORCHLIGHT_STREAM_HEARTBEAT_SECS", default_value_t = 120)]
    pub heartbeat_secs: u64,
    #[arg(long, env = "PORCHLIGHT_BODY_TIMEOUT_SECS", default_value_t = 10)]
    pub body_timeout_secs: u64,
}
impl ServeArgs {
    pub fn net_policy(&self) -> crate::server::NetPolicy {
        crate::server::NetPolicy {
            funnel_peers: self.funnel_peers.clone(),
            enroll_cidrs: self.enroll_cidrs.clone(),
            proxy_timeout: Duration::from_secs(5),
            tls_timeout: Duration::from_secs(10),
            ..Default::default()
        }
    }
    pub fn settings(&self) -> Settings {
        Settings {
            release_repo: self.release_repo.clone().filter(|v| !v.is_empty()),
            release_public_key: self.release_public_key.clone().filter(|v| !v.is_empty()),
            release_dir: self.release_dir.clone(),
            upgrade_command: self.upgrade_command.clone().filter(|v| !v.is_empty()),
            data_dir: self.data_dir.clone(),
            backup_dir: self.backup_dir.clone(),
            attachment_quota_bytes: self.attachment_quota_bytes,
            disk_reserve_bytes: self.disk_reserve_bytes,
            maintenance_file: self
                .maintenance_file
                .clone()
                .unwrap_or_else(|| self.agent_socket.with_file_name("maintenance")),
            _test_data_guard: None,
            min_client_version: self.min_client_version,
            device_cert_days: self.device_cert_days,
            device_renew_days: self.device_renew_days,
            enroll_host: self.enroll_host.clone(),
            endpoints: self.endpoints.clone(),
            heartbeat: Duration::from_secs(self.heartbeat_secs),
            body_timeout: Duration::from_secs(self.body_timeout_secs),
        }
    }
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.release_repo
                .as_deref()
                .is_none_or(|v| v.is_empty() || valid_release_repo(v)),
            "invalid release repository"
        );
        anyhow::ensure!(
            self.funnel_addr.is_none() || !self.funnel_peers.is_empty(),
            "funnel peers required when funnel listener is enabled"
        );
        anyhow::ensure!(
            self.funnel_addr.is_some() || self.funnel_peers.is_empty(),
            "funnel peers require a funnel listener"
        );
        anyhow::ensure!(!self.enroll_cidrs.is_empty(), "enrollment CIDRs required");
        anyhow::ensure!(
            !self.enroll_host.trim().is_empty(),
            "enrollment host required"
        );
        anyhow::ensure!(
            self.device_cert_days > 0
                && self.device_renew_days > 0
                && self.device_renew_days < self.device_cert_days,
            "invalid device certificate lifetimes"
        );
        anyhow::ensure!(self.db_pool_size > 0, "database pool size must be positive");
        anyhow::ensure!(self.heartbeat_secs > 0, "heartbeat must be positive");
        anyhow::ensure!(self.body_timeout_secs > 0, "body timeout must be positive");
        anyhow::ensure!(
            !self.endpoints.is_empty()
                && self
                    .endpoints
                    .iter()
                    .all(|s| s.starts_with("https://") && s.len() > 8),
            "HTTPS endpoints required"
        );
        Ok(())
    }
}
fn valid_release_repo(value: &str) -> bool {
    let Some((owner, repo)) = value.split_once('/') else {
        return false;
    };
    (1..=39).contains(&owner.len())
        && owner.as_bytes()[0].is_ascii_alphanumeric()
        && owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && (1..=100).contains(&repo.len())
        && repo
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

pub(crate) fn redacted_url(url: &str) -> String {
    match url.parse::<sqlx::postgres::PgConnectOptions>() {
        Ok(o) => format!(
            "postgres://{}@{}/{}",
            o.get_username(),
            o.get_host(),
            o.get_database().unwrap_or("")
        ),
        Err(_) => "[REDACTED invalid database URL]".into(),
    }
}
impl fmt::Debug for ServeArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServeArgs")
            .field("data_dir", &self.data_dir)
            .field("db_pool_size", &self.db_pool_size)
            .field("database_url", &redacted_url(&self.database_url))
            .field(
                "migrator_url",
                &self.migrator_url.as_deref().map(redacted_url),
            )
            .field("lan_addr", &self.lan_addr)
            .field("funnel_addr", &self.funnel_addr)
            .field("funnel_peers", &self.funnel_peers)
            .field("enroll_addr", &self.enroll_addr)
            .field("enroll_cidrs", &self.enroll_cidrs)
            .field("agent_socket", &self.agent_socket)
            .field("settings", &self.settings())
            .finish()
    }
}
#[cfg(test)]
mod release_tests {
    use super::*;
    #[test]
    fn repository_pattern_is_exact() {
        for good in ["example-owner/porchlight", "A/repo.name_1-", "0/r"] {
            assert!(valid_release_repo(good));
        }
        for bad in [
            "",
            "owner",
            "/repo",
            "owner/",
            "-owner/repo",
            "owner_name/repo",
            "owner/repo/extra",
            "owner/repo?x",
            " owner/repo",
            "owner/repo\n",
            "é/repo",
        ] {
            assert!(!valid_release_repo(bad));
        }
        assert!(valid_release_repo(&format!(
            "{}/{}",
            "a".repeat(39),
            "b".repeat(100)
        )));
        assert!(!valid_release_repo(&format!("{}/r", "a".repeat(40))));
        assert!(!valid_release_repo(&format!("a/{}", "b".repeat(101))));
    }
}
