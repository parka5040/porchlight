mod attach;
mod client;
mod comments;
mod derive;
mod dev_init;
mod domain;
mod enroll;
mod export;
mod export_verify;
mod feed;
mod history;
mod import;
mod items;
mod lists;
mod members;
mod notifications;
mod occurrences;
mod output;
mod password;
mod reads;
mod release;
mod schedule;
mod users;

use clap::{Parser, Subcommand, ValueEnum};
use client::{Client, Result};
use dev_init::DeviceSpec;
use serde_json::{Value, json};
use std::{path::PathBuf, process::ExitCode};
use uuid::Uuid;

/// Household administration over the local agent socket.
#[derive(Parser)]
#[command(name = "porch", version, about)]
struct Args {
    #[arg(
        long,
        global = true,
        env = "PORCHLIGHT_SOCKET",
        default_value = "/run/porchlight/agent.sock"
    )]
    socket: PathBuf,
    /// Read a PAT from a private file (0600); a trailing newline is trimmed.
    #[arg(long, global = true)]
    token_file: Option<PathBuf>,
    /// Emit JSON, including API problems on failure.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}
impl std::fmt::Debug for Args {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Args")
            .field("token", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[derive(Subcommand)]
enum Command {
    /// Member JSON + ICS content, or secret-free household tar.gz.
    Export(export::Export),
    /// Operational backup status (admin PAT).
    Backups,
    /// Signed server updates and advertised device releases.
    Release {
        #[command(subcommand)]
        command: release::Command,
    },
    Items {
        #[command(subcommand)]
        command: Box<items::Command>,
    },
    Occurrences {
        #[command(subcommand)]
        command: occurrences::Command,
    },
    Members {
        #[command(subcommand)]
        command: members::Command,
    },
    Agenda(reads::Agenda),
    Board(reads::Board),
    Search {
        q: String,
    },
    Revert(history::Revert),
    Revisions {
        item: Uuid,
    },
    Purge(history::Purge),
    Derive {
        #[command(subcommand)]
        command: derive::Command,
    },
    /// Private inbox, cross-device dismissal, canonical preferences and admin inspection.
    Notifications {
        #[command(subcommand)]
        command: notifications::NotificationCommand,
    },
    /// Preview or commit a one-off ICS calendar.
    Import {
        #[command(subcommand)]
        command: import::Command,
    },
    /// Manage server-pulled read-only calendars.
    Feed {
        #[command(subcommand)]
        command: feed::Command,
    },
    /// Availability, EDF proposals, acceptance/dismissal and dependent assignees.
    Schedule {
        #[command(subcommand)]
        command: schedule::Command,
    },
    /// Household lists and checklists.
    Lists {
        #[command(subcommand)]
        command: lists::ListsCommand,
    },
    /// Show the calling token's identity and scopes.
    Whoami,
    /// Add or list item comments.
    Comments {
        #[command(subcommand)]
        command: comments::Comments,
    },
    /// Attach a file (25 MiB maximum) to an item.
    Attach {
        item: Uuid,
        file: PathBuf,
    },
    /// Issue an enrollment URI and terminal QR code.
    Enroll {
        #[arg(long)]
        user: String,
        #[arg(long)]
        device: String,
    },
    /// Super-admin user operations.
    #[command(name = "users", visible_alias = "user")]
    User {
        #[command(subcommand)]
        command: users::Command,
    },
    /// List or revoke household devices.
    #[command(name = "devices", visible_alias = "device")]
    Device {
        #[command(subcommand)]
        command: DeviceCommand,
    },
    /// Create or revoke personal access tokens.
    Token {
        #[command(subcommand)]
        command: TokenCommand,
    },
    /// Enroll dev devices through pinned TLS (keep --out under var/).
    DevInit {
        #[arg(long)]
        out: PathBuf,
        /// Override TCP destination only; TLS identity still comes from the URI.
        #[arg(long)]
        enroll_addr: Option<String>,
        #[arg(long, default_value = "admin")]
        user: String,
        #[arg(
            long,
            value_delimiter = ',',
            default_value = "emulator:android,desktop:desktop"
        )]
        devices: Vec<DeviceSpec>,
    },
}
#[derive(Subcommand)]
enum DeviceCommand {
    List {
        #[arg(long)]
        member: Option<String>,
    },
    Revoke {
        device_id: Uuid,
    },
}
#[derive(Clone, Copy, ValueEnum)]
enum Scope {
    Read,
    Write,
    Admin,
}
impl Scope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Admin => "admin",
        }
    }
}
#[derive(Subcommand)]
enum TokenCommand {
    Create {
        #[arg(long)]
        name: String,
        /// Scopes: admin implies write implies read; every live token can read.
        /// For mutations: least-privilege default: read,write (equivalent to write); admin requires password re-entry.
        #[arg(long, value_delimiter = ',', default_value = "read,write")]
        scope: Vec<Scope>,
        #[arg(long, default_value = "365", value_parser = clap::value_parser!(u32).range(1..))]
        days: u32,
    },
    Revoke {
        token_id: Uuid,
    },
}

async fn dispatch(client: &Client, command: &Command) -> Result<Value> {
    match command {
        Command::Export(args) => export::run(client, args).await,
        Command::Release { command } => release::run(client, command).await,
        Command::Backups => {
            client
                .request("GET", "/v1/admin/backups", Value::Null)
                .await
        }
        Command::Notifications { command } => notifications::dispatch(client, command).await,
        Command::Comments { command } => comments::run(client, command).await,
        Command::Attach { item, file } => attach::run(client, *item, file).await,
        Command::Import { command } => import::run(client, command).await,
        Command::Feed { command } => feed::run(client, command).await,
        Command::Schedule { command } => schedule::dispatch(client, command).await,
        Command::Lists { command } => lists::dispatch(client, command).await,
        Command::Whoami => client.request("GET", "/v1/admin/whoami", Value::Null).await,
        Command::Enroll { user, device } => client.enrollment_token(user, device).await,
        Command::Items { command } => items::run(client, command).await,
        Command::Occurrences { command } => occurrences::run(client, command).await,
        Command::Members { command } => members::run(client, command).await,
        Command::User { command } => users::run(client, command).await,
        Command::Agenda(a) => reads::agenda(client, a).await,
        Command::Board(b) => reads::board(client, b).await,
        Command::Search { q } => reads::search(client, q).await,
        Command::Revert(b) => history::revert(client, b).await,
        Command::Revisions { item } => history::revisions(client, *item).await,
        Command::Purge(b) => history::purge(client, b).await,
        Command::Derive { command } => derive::run(client, command).await,
        Command::Device {
            command: DeviceCommand::List { member },
        } => {
            let member = if let Some(name) = member {
                Some(domain::member(client, name).await?.to_string())
            } else {
                None
            };
            let mut value = domain::collection(
                client,
                &domain::query("/v1/admin/devices", &[("member_id", member)]),
            )
            .await?;
            value["devices"] = value["items"].clone();
            Ok(value)
        }
        Command::Device {
            command: DeviceCommand::Revoke { device_id },
        } => {
            client
                .request(
                    "POST",
                    &format!("/v1/admin/devices/{device_id}/revoke"),
                    json!({}),
                )
                .await
        }
        Command::Token {
            command: TokenCommand::Create { name, scope, days },
        } => {
            let password = if scope.iter().any(|s| matches!(s, Scope::Admin)) {
                Some(password::password()?)
            } else {
                None
            };
            client.request("POST", "/v1/admin/tokens", json!({"name":name, "scopes":scope.iter().map(|s| s.as_str()).collect::<Vec<_>>(), "expires_days":days, "password":password.as_ref().map(|p| p.as_str())})).await
        }
        Command::Token {
            command: TokenCommand::Revoke { token_id },
        } => client.revoke_token(*token_id).await,
        Command::DevInit {
            out,
            enroll_addr,
            user,
            devices,
        } => dev_init::run(client, out, enroll_addr.as_deref(), user, devices).await,
    }
}

fn read_token_file(path: &std::path::Path) -> std::result::Result<String, &'static str> {
    use std::{io::Read, os::unix::fs::PermissionsExt};
    let mut file = std::fs::File::open(path).map_err(|_| "cannot open token file")?;
    let metadata = file.metadata().map_err(|_| "cannot inspect token file")?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
        return Err("token file must be a regular file with no group/other permissions (0600)");
    }
    let mut token = String::new();
    file.read_to_string(&mut token)
        .map_err(|_| "cannot read token file")?;
    if token.ends_with('\n') {
        token.pop();
        if token.ends_with('\r') {
            token.pop();
        }
    }
    if token.is_empty() {
        return Err("token file is empty");
    }
    Ok(token)
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(error) => {
            if !matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) && std::env::args_os().any(|a| a == "--json")
            {
                // Never echo argv: it can contain accidental credential values.
                client::Error::Usage("Invalid command arguments; see --help").print(true);
                return ExitCode::from(2);
            }
            error.exit()
        }
    };
    // clap forbids required global arguments; enforce credentials after parsing.
    let token = if let Some(path) = &args.token_file {
        match read_token_file(path) {
            Ok(token) => token,
            Err(message) => {
                client::Error::Local(message).print(args.json);
                return ExitCode::FAILURE;
            }
        }
    } else {
        match std::env::var("PORCHLIGHT_TOKEN") {
            Ok(token) => token,
            Err(_) => {
                client::Error::Usage("--token-file or PORCHLIGHT_TOKEN is required")
                    .print(args.json);
                return ExitCode::from(2);
            }
        }
    };
    let client = Client::new(args.socket, token);
    match dispatch(&client, &args.command)
        .await
        .and_then(|value| output::print(&args.command, &value, args.json))
    {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            error.print(args.json);
            if matches!(error, client::Error::Usage(_)) {
                ExitCode::from(2)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

#[cfg(test)]
mod token_file_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn token_help_qualifies_the_default_as_write_capable() {
        let help = match Args::try_parse_from(["porch", "token", "create", "--help"]) {
            Err(error) => error.to_string(),
            Ok(_) => panic!("--help must display help"),
        };
        assert!(help.contains("equivalent to write"));
        assert!(help.contains("For mutations"));
        assert_eq!(help.matches("Scopes:").count(), 1);
    }
    #[test]
    fn token_file_is_private_and_trims_only_trailing_newline() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var");
        std::fs::create_dir_all(&root).unwrap();
        let dir = tempfile::tempdir_in(root).unwrap();
        let path = dir.path().join("test.token");
        std::fs::write(&path, "test-value\n").unwrap();
        for mode in [0o644, 0o640, 0o604] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(read_token_file(&path).is_err());
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(read_token_file(&path).unwrap() == "test-value");
        std::fs::write(&path, " test-value \r\n").unwrap();
        assert!(read_token_file(&path).unwrap() == " test-value ");
        let args =
            Args::try_parse_from(["porch", "--token-file", path.to_str().unwrap(), "whoami"])
                .unwrap();
        assert_eq!(args.token_file.as_deref(), Some(path.as_path()));
    }
}

#[cfg(test)]
mod calendar_parsing_tests {
    use super::*;
    #[test]
    fn school_ics_dependent_display_name_cli() {
        let a = Args::try_parse_from([
            "porch",
            "import",
            "ics",
            "school.ics",
            "--member",
            "School Child",
        ])
        .unwrap();
        assert!(
            matches!(a.command, Command::Import { command: import::Command::Ics { member, visibility: None, .. } } if member == "School Child")
        );
    }
}
