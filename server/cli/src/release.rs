//! Read-only remote update inspection; upgrades remain an owner-run host command.
use crate::client::{Client, Result};
use clap::Subcommand;
use serde_json::Value;

#[derive(Subcommand)]
pub enum Command {
    /// Show server update checks and the verified client release (admin PAT).
    Status,
}
pub async fn run(client: &Client, command: &Command) -> Result<Value> {
    match command {
        Command::Status => {
            client
                .request("GET", "/v1/admin/release", Value::Null)
                .await
        }
    }
}
