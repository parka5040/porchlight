//! Planning command family. All output uses the shared --json renderer.
use crate::client::{Client, Error, Result};
use clap::Subcommand;
use serde_json::{Value, json};
use std::path::PathBuf;
use uuid::Uuid;
#[derive(Subcommand)]
pub enum Command {
    /// Generate EDF proposals for yourself (or a dependent as super admin).
    Suggest {
        #[arg(long)]
        member: Option<Uuid>,
    },
    /// List private proposals and history.
    List {
        #[arg(long)]
        member: Option<Uuid>,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long,default_value="50",value_parser=clap::value_parser!(u16).range(1..=200))]
        limit: u16,
    },
    /// Accept with the occurrence's current version, not the proposal version.
    Accept {
        id: Uuid,
        #[arg(long)]
        version: i32,
    },
    /// Dismiss with the suggestion's current version.
    Dismiss {
        id: Uuid,
        #[arg(long)]
        version: i32,
    },
    /// Show a member's availability collection/version.
    Availability { member: Uuid },
    /// Replace windows from a JSON file containing {"windows":[...]}.
    SetAvailability {
        member: Uuid,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        version: i32,
    },
    /// Rank assignee candidates for an unassigned dependent appointment.
    Candidates { item: Uuid },
}
pub async fn dispatch(client: &Client, command: &Command) -> Result<Value> {
    match command {
        Command::Suggest { member } => {
            client
                .request("POST", "/v1/schedule/suggest", json!({"member_id":member}))
                .await
        }
        Command::List {
            member,
            cursor,
            limit,
        } => {
            let mut path = format!("/v1/suggestions?limit={limit}");
            if let Some(member) = member {
                path.push_str(&format!("&member_id={member}"))
            }
            if let Some(cursor) = cursor {
                path.push_str(&format!("&cursor={}", crate::client::segment(cursor)))
            }
            client.request("GET", &path, Value::Null).await
        }
        Command::Accept { id, version } => {
            client
                .request_version(
                    "POST",
                    &format!("/v1/suggestions/{id}/accept"),
                    json!({}),
                    Some(i64::from(*version)),
                )
                .await
        }
        Command::Dismiss { id, version } => {
            client
                .request_version(
                    "POST",
                    &format!("/v1/suggestions/{id}/dismiss"),
                    json!({}),
                    Some(i64::from(*version)),
                )
                .await
        }
        Command::Availability { member } => {
            client
                .request(
                    "GET",
                    &format!("/v1/members/{member}/availability"),
                    Value::Null,
                )
                .await
        }
        Command::SetAvailability {
            member,
            file,
            version,
        } => {
            let bytes = std::fs::read(file)
                .map_err(|_| Error::Local("Could not read availability JSON"))?;
            let body = serde_json::from_slice(&bytes)
                .map_err(|_| Error::Local("Invalid availability JSON"))?;
            client
                .request_version(
                    "PUT",
                    &format!("/v1/members/{member}/availability"),
                    body,
                    Some(i64::from(*version)),
                )
                .await
        }
        Command::Candidates { item } => {
            client
                .request("GET", &format!("/v1/items/{item}/candidates"), Value::Null)
                .await
        }
    }
}
