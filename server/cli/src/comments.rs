use crate::client::{Client, Result, segment};
use clap::Subcommand;
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Subcommand)]
pub enum Comments {
    Add {
        item: Uuid,
        body: String,
    },
    List {
        item: Uuid,
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long,default_value_t=50,value_parser=clap::value_parser!(u16).range(1..=200))]
        limit: u16,
    },
}
pub async fn run(client: &Client, command: &Comments) -> Result<Value> {
    match command {
        Comments::Add { item, body } => {
            client
                .request(
                    "POST",
                    &format!("/v1/items/{item}/comments"),
                    json!({"body":body}),
                )
                .await
        }
        Comments::List {
            item,
            cursor,
            limit,
        } => {
            let mut path = format!("/v1/items/{item}/comments?limit={limit}");
            if let Some(cursor) = cursor {
                path.push_str(&format!("&cursor={}", segment(cursor)));
            }
            client.request("GET", &path, Value::Null).await
        }
    }
}
