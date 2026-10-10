use crate::{
    client::{Client, Result},
    domain,
};
use clap::Subcommand;
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Subcommand)]
pub enum Command {
    Status,
    Retry { item: Uuid },
}
pub async fn run(c: &Client, cmd: &Command) -> Result<Value> {
    match cmd {
        Command::Status => domain::collection(c, "/v1/admin/derive").await,
        Command::Retry { item } => {
            c.request("POST", &format!("/v1/admin/derive/{item}/retry"), json!({}))
                .await
        }
    }
}
