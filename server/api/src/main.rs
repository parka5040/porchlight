use anyhow::{Context, ensure};
use clap::{Args, Parser, Subcommand};
use futures_util::FutureExt;
use porchlight_api::{
    config::ServeArgs,
    db::{APP_SETTINGS, connect_options, healthcheck, migrate},
    init::{AlreadyInitialized, InitParams, finish_init, initialize},
    pki_store::{DataDir, ReissuedCert, load, reissue_server_cert, renew_loaded, verify_install},
    server::{self, Listeners},
    state::AppState,
};
use porchlight_core::auth::password::PasswordHasher;
use porchlight_pki::ServerSans;
use sqlx::postgres::PgPoolOptions;
use std::{
    io::{self, IsTerminal, Write},
    net::IpAddr,
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
    time::SystemTime,
};
use tracing_subscriber::EnvFilter;
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(name = "porchlight", version, about = "Porchlight server")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Print the generated OpenAPI JSON contract.
    Openapi,
    /// Apply database migrations (as the migrator role).
    Migrate {
        #[arg(long, env = "PORCHLIGHT_MIGRATOR_URL", hide_env_values = true)]
        migrator_url: String,
    },
    /// Initialize the household once. Pipe the admin password into stdin, or set PORCHLIGHT_INIT_ADMIN_PASSWORD.
    Init(InitArgs),
    /// Issue a recovery admin PAT locally; prints it once to stdout. Requires the API database credential.
    AdminToken {
        #[arg(long, env = "PORCHLIGHT_DATABASE_URL", hide_env_values = true)]
        database_url: String,
        /// Live super-admin member UUID or username.
        #[arg(long)]
        member: String,
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "365", value_parser = clap::value_parser!(u32).range(1..))]
        expires_days: u32,
    },
    /// Replace the server certificate's complete SAN set without changing its key or household CA.
    /// A running server keeps its loaded certificate: restart it after successful reissue.
    ServerCert(ServerCertArgs),
    /// Run the server until SIGINT/SIGTERM.
    Serve(Box<ServeArgs>),
    /// Exit 0 iff DB/schema and the serving process answer within four seconds.
    Healthcheck {
        #[arg(long, env = "PORCHLIGHT_DATABASE_URL", hide_env_values = true)]
        database_url: String,
    },
}
#[derive(Args)]
struct InitArgs {
    #[arg(long, env = "PORCHLIGHT_DATA_DIR", default_value = "/data")]
    data_dir: PathBuf,
    #[arg(long, env = "PORCHLIGHT_DATABASE_URL", hide_env_values = true)]
    database_url: String,
    #[arg(long, env = "PORCHLIGHT_MIGRATOR_URL", hide_env_values = true)]
    migrator_url: Option<String>,
    #[arg(long)]
    household: String,
    #[arg(long)]
    admin_username: String,
    #[arg(long)]
    admin_name: String,
    #[arg(long, default_value = "America/New_York")]
    tz: String,
    #[arg(long)]
    san_dns: Vec<String>,
    #[arg(long)]
    san_ip: Vec<IpAddr>,
    #[arg(long)]
    age_recipient: String,
    #[arg(long)]
    json: bool,
}
impl std::fmt::Debug for InitArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InitArgs")
            .field("database_url", &"[REDACTED]")
            .field("migrator_url", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}
#[derive(Args)]
struct ServerCertArgs {
    #[arg(long, env = "PORCHLIGHT_DATA_DIR", default_value = "/data")]
    data_dir: PathBuf,
    #[arg(long, env = "PORCHLIGHT_DATABASE_URL", hide_env_values = true)]
    database_url: Option<String>,
    /// Complete new DNS SAN set; porchlight.lan is always included, as at init.
    #[arg(long)]
    san_dns: Vec<String>,
    /// Complete new IP SAN set (not appended to the existing certificate).
    #[arg(long)]
    san_ip: Vec<IpAddr>,
    /// Show the current SANs and expiry without writing.
    #[arg(long, conflicts_with_all = ["san_dns", "san_ip"])]
    show: bool,
    #[arg(long)]
    json: bool,
}
impl std::fmt::Debug for ServerCertArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerCertArgs")
            .field("database_url", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}
/// Identical public metadata schema for show and reissue. Timestamps are UTC RFC3339 seconds.
fn print_server_cert(output: &ReissuedCert, json: bool) -> anyhow::Result<()> {
    let sans_json = |sans: &ServerSans| serde_json::json!({"dns": sans.dns, "ips": sans.ips});
    let previous = sans_json(&output.previous);
    let current = sans_json(&output.current);
    let not_after = chrono::DateTime::<chrono::Utc>::from(output.not_after)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut stdout = io::stdout().lock();
    if json {
        writeln!(
            stdout,
            "{}",
            serde_json::json!({
                "previous": previous, "current": current, "not_after": not_after,
            })
        )?;
    } else {
        writeln!(
            stdout,
            "previous: {previous}\ncurrent: {current}\nnot_after: {not_after}"
        )?;
    }
    stdout.flush().context("flushing server certificate output")
}
#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    match dispatch(cli.command).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            if e.is::<AlreadyInitialized>() {
                ExitCode::from(2)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}
async fn app_pool(url: &str, size: u32) -> anyhow::Result<sqlx::PgPool> {
    PgPoolOptions::new()
        .max_connections(size)
        .connect_with(connect_options(url, APP_SETTINGS)?)
        .await
        .map_err(|e| porchlight_api::db::connection_error(url, e))
}
async fn dispatch(command: Command) -> anyhow::Result<()> {
    match command {
        Command::Openapi => {
            print!("{}", porchlight_api::openapi::json());
            Ok(())
        }
        Command::Migrate { migrator_url } => migrate(&migrator_url).await,
        Command::Healthcheck { database_url } => healthcheck(&database_url).await,
        Command::AdminToken {
            database_url,
            member,
            name,
            expires_days,
        } => {
            let pool = app_pool(&database_url, 1).await?;
            let token =
                porchlight_api::admin_recovery::mint(&pool, &member, &name, expires_days).await?;
            let mut stdout = io::stdout().lock();
            writeln!(stdout, "{}", token.expose()).context("writing recovery token")?;
            stdout.flush().context("flushing recovery token output")
        }
        Command::Init(args) => {
            let password = if let Ok(value) = std::env::var("PORCHLIGHT_INIT_ADMIN_PASSWORD") {
                Zeroizing::new(value)
            } else {
                ensure!(
                    !io::stdin().is_terminal(),
                    "set PORCHLIGHT_INIT_ADMIN_PASSWORD or pipe the password into stdin"
                );
                let mut line = Zeroizing::new(String::new());
                io::stdin()
                    .read_line(&mut line)
                    .context("reading password from stdin")?;
                if line.ends_with('\n') {
                    line.pop();
                    if line.ends_with('\r') {
                        line.pop();
                    }
                }
                line
            };
            if let Some(url) = args.migrator_url {
                migrate(&url).await?;
            }
            let pool = app_pool(&args.database_url, 5).await?;
            let dir = DataDir::new(args.data_dir);
            let params = InitParams {
                household_name: args.household,
                tzid: args.tz,
                admin_username: args.admin_username,
                admin_display_name: args.admin_name,
                admin_password: password,
                sans: ServerSans {
                    dns: args.san_dns,
                    ips: args.san_ip,
                },
                age_recipient: args.age_recipient,
            };
            let output = initialize(
                &pool,
                &dir,
                params,
                &PasswordHasher::production(),
                SystemTime::now(),
            )
            .await?;
            let mut stdout = io::stdout().lock();
            if args.json {
                writeln!(stdout, "{}", serde_json::to_string(&output)?)?;
            } else {
                writeln!(
                    stdout,
                    "household_id: {}\nadmin_member_id: {}\nca_fingerprint: {}\nbootstrap_token: {}\ndata_dir: {}",
                    output.household_id,
                    output.admin_member_id,
                    output.ca_fingerprint,
                    output.bootstrap_token,
                    output.data_dir.display()
                )?;
            }
            stdout.flush().context("flushing initialization output")?;
            finish_init(&dir)?;
            Ok(())
        }
        Command::ServerCert(args) => {
            let dir = DataDir::new(args.data_dir);
            let pki = load(&dir)?;
            if let Some(url) = args.database_url {
                let pool = app_pool(&url, 1).await?;
                verify_install(&pool, &pki).await?;
            }
            let output = if args.show {
                let info = porchlight_pki::parse_cert_pem(&pki.server_cert_pem)?;
                ReissuedCert {
                    previous: info.sans.clone(),
                    current: info.sans,
                    not_after: info.not_after,
                }
            } else {
                reissue_server_cert(
                    &dir,
                    &pki,
                    &ServerSans {
                        dns: args.san_dns,
                        ips: args.san_ip,
                    },
                    SystemTime::now(),
                )?
            };
            print_server_cert(&output, args.json)
        }
        Command::Serve(args) => {
            args.validate()?;
            let shutdown = shutdown_signal().shared();
            let policy = args.net_policy();
            let settings = args.settings();
            if let Some(url) = &args.migrator_url {
                migrate(url).await?;
            }
            let pool = app_pool(&args.database_url, args.db_pool_size).await?;
            let dir = DataDir::new(args.data_dir);
            let pki = load(&dir)?;
            verify_install(&pool, &pki).await?;
            let (pki, renewed) = renew_loaded(&dir, pki, SystemTime::now())?;
            let state = AppState::new(
                pool.clone(),
                Arc::new(pki),
                settings,
                &*porchlight_api::pki_store::load_idempotency_key(&dir)?,
                Arc::new(PasswordHasher::production()),
                porchlight_api::ratelimit::RateLimiter::default(),
            )
            .await?;
            let lan = tokio::net::TcpListener::bind(args.lan_addr)
                .await
                .with_context(|| format!("binding LAN listener at {}", args.lan_addr))?;
            let funnel = match args.funnel_addr {
                Some(addr) => Some(
                    tokio::net::TcpListener::bind(addr)
                        .await
                        .with_context(|| format!("binding Funnel listener at {addr}"))?,
                ),
                None => None,
            };
            let enroll = tokio::net::TcpListener::bind(args.enroll_addr)
                .await
                .with_context(|| format!("binding enrollment listener at {}", args.enroll_addr))?;
            let agent = Some(
                server::bind_agent_socket(&args.agent_socket).with_context(|| {
                    format!("binding agent socket at {}", args.agent_socket.display())
                })?,
            );
            tracing::info!(renewed, "porchlight serving");
            porchlight_api::lifecycle::serve_all(
                state,
                Listeners {
                    lan,
                    funnel,
                    enroll,
                    agent,
                },
                policy,
                shutdown,
                true,
                #[cfg(feature = "testing")]
                false,
            )
            .await?;
            Ok(())
        }
    }
}
/// Installs the handlers immediately; the returned future completes on SIGINT or SIGTERM.
fn shutdown_signal() -> impl std::future::Future<Output = ()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    async move {
        tokio::select! { _=int.recv()=>{} _=term.recv()=>{} }
        tracing::info!("shutdown signal received");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_cert_debug_redacts_database_url() {
        let Cli { command } = Cli::try_parse_from([
            "porchlight",
            "server-cert",
            "--database-url",
            "postgres://user:secret-marker@localhost/db",
            "--show",
        ])
        .unwrap();
        let Command::ServerCert(args) = command else {
            panic!("expected server-cert");
        };
        let debug = format!("{args:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("secret-marker"));
        assert!(!debug.contains("postgres://"));
    }
}
