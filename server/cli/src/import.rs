//! One-off calendar import, using household-scoped username, display name or UUID targets.
use crate::client::{Client, Error, Result};
use clap::Subcommand;
use serde_json::{Value, json};
use std::path::PathBuf;
#[derive(Subcommand)]
pub enum Command {
    Ics {
        file: PathBuf,
        #[arg(long)]
        member: String,
        #[arg(long, default_value = "import")]
        name: String,
        /// Stable scope for reimport identity (defaults to import, matching HTTP).
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        preview: bool,
        #[arg(long)]
        visibility: Option<String>,
    },
}
pub async fn run(client: &Client, command: &Command) -> Result<Value> {
    match command {
        Command::Ics {
            file,
            member,
            name,
            scope,
            preview,
            visibility,
        } => {
            let metadata = std::fs::metadata(file)
                .map_err(|_| Error::Local("Could not read calendar file"))?;
            if metadata.len() > 5 * 1024 * 1024 {
                return Err(Error::Local("Calendar file exceeds 5 MB"));
            }
            let content = std::fs::read_to_string(file)
                .map_err(|_| Error::Local("Could not read UTF-8 calendar file"))?;
            let member = client.calendar_member(member).await?;
            client.request("POST", "/v1/imports/ics", json!({"content":content,"member_id":member,"name":name,"scope":scope.as_deref().unwrap_or("import"),"preview":preview,"visibility":visibility})).await
        }
    }
}
