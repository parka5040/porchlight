use crate::{
    client::{Client, Error, Result},
    domain,
};
use clap::Args;
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Args)]
pub struct Revert {
    item: Uuid,
    #[arg(long)]
    to: Uuid,
    #[arg(long)]
    version: Option<i64>,
}
#[derive(Args)]
pub struct Purge {
    #[arg(
        long,
        required_unless_present = "deleted_before",
        conflicts_with = "deleted_before"
    )]
    item: Vec<Uuid>,
    #[arg(long, required_unless_present = "item", conflicts_with = "item")]
    deleted_before: Option<chrono::DateTime<chrono::Utc>>,
    #[arg(long)]
    yes: bool,
}
pub async fn revert(c: &Client, b: &Revert) -> Result<Value> {
    let version = if let Some(v) = b.version {
        v
    } else {
        let items = domain::collection(c, "/v1/items?include_deleted=true").await?;
        items["items"]
            .as_array()
            .and_then(|rs| rs.iter().find(|r| r["id"] == b.item.to_string()))
            .and_then(|r| r["version"].as_i64())
            .ok_or(Error::Usage("Item not found"))?
    };
    c.request_version(
        "POST",
        &format!("/v1/admin/items/{}/revert", b.item),
        json!({"revision_id":b.to}),
        Some(version),
    )
    .await
}
pub async fn revisions(c: &Client, item: Uuid) -> Result<Value> {
    domain::collection(c, &format!("/v1/admin/items/{item}/revisions")).await
}
pub async fn purge(c: &Client, b: &Purge) -> Result<Value> {
    if !b.yes {
        use std::io::{IsTerminal, Write};
        if !std::io::stdin().is_terminal() {
            return Err(Error::Usage(
                "Purge requires --yes when no terminal is available",
            ));
        }
        eprint!("Permanently delete selected soft-deleted items and their history? Type yes: ");
        std::io::stderr()
            .flush()
            .map_err(|_| Error::Local("Could not prompt"))?;
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .map_err(|_| Error::Local("Could not read confirmation"))?;
        if answer.trim() != "yes" {
            return Err(Error::Usage("Purge cancelled"));
        }
    }
    let value = if let Some(time) = b.deleted_before {
        json!({"deleted_before":time})
    } else {
        json!({"item_ids":b.item})
    };
    c.request("POST", "/v1/admin/purge", value).await
}
