use crate::{
    client::{Client, Error, Result, segment},
    domain, password,
};
use clap::Subcommand;
use serde_json::{Value, json};
#[derive(Subcommand)]
pub enum Command {
    List,
    Add {
        #[arg(long)]
        display_name: String,
        #[arg(long, default_value = "user")]
        role: String,
        #[arg(long)]
        username: Option<String>,
        #[arg(long)]
        tz: Option<String>,
    },
    Edit {
        member: String,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        role: Option<String>,
        #[arg(long)]
        color: Option<String>,
        #[arg(long)]
        version: Option<i64>,
    },
    Rm {
        member: String,
        #[arg(long)]
        version: Option<i64>,
    },
    ResetPassword {
        username: String,
    },
}
pub async fn run(c: &Client, cmd: &Command) -> Result<Value> {
    match cmd {
        Command::List => domain::collection(c, "/v1/admin/users").await,
        Command::Add {
            display_name,
            role,
            username,
            tz,
        } => {
            let password = if role != "dependent" {
                Some(password::password()?)
            } else {
                None
            };
            c.request("POST","/v1/admin/users",json!({"display_name":display_name,"role":role,"username":username,"tzid":tz,"initial_password":password.as_ref().map(|v|v.as_str())})).await
        }
        Command::ResetPassword { username } => {
            c.request(
                "POST",
                &format!("/v1/admin/users/{}/password-reset", segment(username)),
                json!({}),
            )
            .await
        }
        Command::Edit {
            member,
            display_name,
            role,
            color,
            version,
        } => {
            let mut b = json!({});
            for (k, v) in [
                ("display_name", display_name),
                ("role", role),
                ("color", color),
            ] {
                if let Some(v) = v {
                    b[k] = json!(v);
                }
            }
            write(c, member, *version, "PATCH", b).await
        }
        Command::Rm { member, version } => write(c, member, *version, "DELETE", json!({})).await,
    }
}
async fn write(
    c: &Client,
    name: &str,
    version: Option<i64>,
    method: &str,
    body: Value,
) -> Result<Value> {
    let id = domain::member(c, name).await?;
    let users = domain::collection(c, "/v1/admin/users").await?;
    let ver = version
        .or_else(|| {
            users["items"]
                .as_array()
                .and_then(|rows| rows.iter().find(|r| r["id"] == id.to_string()))
                .and_then(|r| r["version"].as_i64())
        })
        .ok_or(Error::Usage("Member not found"))?;
    c.request_version(method, &format!("/v1/admin/users/{id}"), body, Some(ver))
        .await
}
