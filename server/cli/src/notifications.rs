//! Private inbox and admin inspection; prefs replacements carry the explicitly observed version.
use crate::client::{Client, Error, Result};
use clap::Subcommand;
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Subcommand)]
pub enum NotificationCommand {
    List {
        #[arg(long)]
        unread: bool,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long,value_parser=clap::value_parser!(u16).range(1..=200))]
        limit: Option<u16>,
    },
    Inspect {
        #[arg(long)]
        member: Uuid,
    },
    Read {
        id: Uuid,
    },
    ReadAll,
    Prefs,
    SetPrefs {
        #[arg(long)]
        file: std::path::PathBuf,
        #[arg(long,value_parser=clap::value_parser!(i32).range(1..))]
        version: i32,
    },
}
pub async fn dispatch(client: &Client, command: &NotificationCommand) -> Result<Value> {
    match command {
        NotificationCommand::List {
            unread,
            cursor,
            limit,
        } => {
            let mut path = format!("/v1/notifications?unread={unread}");
            if let Some(cursor) = cursor {
                path.push_str(&format!("&cursor={}", crate::client::segment(cursor)));
            }
            if let Some(limit) = limit {
                path.push_str(&format!("&limit={limit}"));
            }
            client.request("GET", &path, Value::Null).await
        }
        NotificationCommand::Inspect { member } => {
            client
                .request(
                    "GET",
                    &format!("/v1/admin/notifications?member={member}"),
                    Value::Null,
                )
                .await
        }
        NotificationCommand::Read { id } => {
            client
                .request("POST", &format!("/v1/notifications/{id}/read"), json!({}))
                .await
        }
        NotificationCommand::ReadAll => {
            client
                .request("POST", "/v1/notifications/read-all", json!({}))
                .await
        }
        NotificationCommand::Prefs => {
            client
                .request("GET", "/v1/me/notification-prefs", Value::Null)
                .await
        }
        NotificationCommand::SetPrefs { file, version } => {
            let bytes =
                std::fs::read(file).map_err(|_| Error::Local("Could not read preferences file"))?;
            if bytes.len() > 65536 {
                return Err(Error::Local("Preferences file too large"));
            }
            let body = serde_json::from_slice(&bytes)
                .map_err(|_| Error::Local("Invalid preferences JSON"))?;
            client
                .request_version(
                    "PUT",
                    "/v1/me/notification-prefs",
                    body,
                    Some(i64::from(*version)),
                )
                .await
        }
    }
}
