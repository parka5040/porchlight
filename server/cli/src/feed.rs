//! Read-only subscribed feed administration over the agent socket.
use crate::client::{Client, Result};
use clap::Subcommand;
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Subcommand)]
pub enum Command {
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        url: String,
        /// Omit for a household-wide busy calendar.
        #[arg(long)]
        member: Option<String>,
        #[arg(long)]
        allow_http: bool,
        #[arg(long, default_value = "86400")]
        refresh_seconds: u32,
        #[arg(long)]
        visibility: Option<String>,
    },
    List {
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long, default_value = "50")]
        limit: u16,
    },
    Rm {
        id: Uuid,
        #[arg(long)]
        if_match: i32,
    },
    Refresh {
        id: Uuid,
        #[arg(long)]
        if_match: i32,
    },
}
pub async fn run(client: &Client, command: &Command) -> Result<Value> {
    match command {
        Command::Add {
            name,
            url,
            member,
            allow_http,
            refresh_seconds,
            visibility,
        } => {
            let target = match member {
                Some(who) => Some(client.calendar_member(who).await?),
                None => None,
            };
            client.request("POST","/v1/admin/feeds",json!({"name":name,"url":url,"target_member_id":target,"allow_http":allow_http,"refresh_seconds":refresh_seconds,"visibility":visibility})).await
        }
        Command::List { cursor, limit } => {
            let mut path = format!("/v1/admin/feeds?limit={limit}");
            if let Some(cursor) = cursor {
                path.push_str("&cursor=");
                path.push_str(&crate::client::segment(cursor));
            }
            client.request("GET", &path, Value::Null).await
        }
        Command::Rm { id, if_match } => {
            client
                .request_version(
                    "DELETE",
                    &format!("/v1/admin/feeds/{id}"),
                    json!({}),
                    Some(i64::from(*if_match)),
                )
                .await
        }
        Command::Refresh { id, if_match } => {
            client
                .request_version(
                    "POST",
                    &format!("/v1/admin/feeds/{id}/refresh"),
                    json!({}),
                    Some(i64::from(*if_match)),
                )
                .await
        }
    }
}
