use crate::{
    client::{Client, Error, Result},
    domain,
};
use clap::Subcommand;
use serde_json::{Value, json};
#[derive(Subcommand)]
pub enum Command {
    List,
    SetTz {
        member: String,
        tz: String,
        #[arg(long)]
        version: Option<i64>,
    },
}
pub async fn run(c: &Client, cmd: &Command) -> Result<Value> {
    match cmd {
        Command::List => domain::collection(c, "/v1/members").await,
        Command::SetTz {
            member,
            tz,
            version,
        } => {
            let id = domain::member(c, member).await?;
            let list = domain::collection(c, "/v1/members").await?;
            let ver = version
                .or_else(|| {
                    list["items"]
                        .as_array()
                        .and_then(|rows| rows.iter().find(|r| r["id"] == id.to_string()))
                        .and_then(|r| r["version"].as_i64())
                })
                .ok_or(Error::Usage("Member not found"))?;
            c.request_version(
                "PATCH",
                &format!("/v1/members/{id}"),
                json!({"tzid":tz,"tz_source":"manual"}),
                Some(ver),
            )
            .await
        }
    }
}
