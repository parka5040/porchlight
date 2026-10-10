use crate::{
    client::{Client, Result},
    domain,
};
use chrono::{Duration, Utc};
use clap::Args;
use serde_json::Value;
#[derive(Args)]
pub struct Agenda {
    #[arg(long)]
    member: Option<String>,
    #[arg(long)]
    from: Option<String>,
    #[arg(long)]
    to: Option<String>,
}
#[derive(Args)]
pub struct Board {
    #[arg(long, default_value = "household")]
    scope: String,
}
pub async fn agenda(c: &Client, a: &Agenda) -> Result<Value> {
    let now = Utc::now();
    let from = a.from.clone().unwrap_or_else(|| now.to_rfc3339());
    let to =
        a.to.clone()
            .unwrap_or_else(|| (now + Duration::days(7)).to_rfc3339());
    let member = if let Some(v) = &a.member {
        Some(domain::member(c, v).await?.to_string())
    } else {
        None
    };
    domain::collection(
        c,
        &domain::query(
            "/v1/agenda",
            &[
                ("from", Some(from)),
                ("to", Some(to)),
                ("member_id", member),
            ],
        ),
    )
    .await
}
pub async fn board(c: &Client, b: &Board) -> Result<Value> {
    domain::collection(
        c,
        &domain::query("/v1/board", &[("scope", Some(b.scope.clone()))]),
    )
    .await
}
pub async fn search(c: &Client, q: &str) -> Result<Value> {
    domain::collection(c, &domain::query("/v1/search", &[("q", Some(q.into()))])).await
}
