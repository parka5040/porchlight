//! Data-only export download. Binary household output is never fed to JSON parser.
use crate::client::{Client, Result};
use clap::Args;
use std::path::PathBuf;
#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
pub struct Selection {
    /// Export this member's visible data through their ACL (admin PAT required).
    #[arg(long, conflicts_with = "household")]
    pub member: Option<String>,
    /// Secret-free household tar.gz, including referenced attachment/thumbnail bytes.
    #[arg(long, conflicts_with = "member")]
    pub household: bool,
}
#[derive(Args, Debug)]
pub struct Export {
    #[command(flatten)]
    pub selection: Selection,
    /// Create a private output file; refuse overwrite. Omit for raw stdout.
    #[arg(long)]
    pub out: Option<PathBuf>,
}
pub async fn run(c: &Client, args: &Export) -> Result<serde_json::Value> {
    let path = if let Some(who) = &args.selection.member {
        let id = crate::domain::member(c, who).await?;
        format!("/v1/export/member?member_id={id}")
    } else {
        "/v1/admin/export/household".to_owned()
    };
    c.download(&path, args.out.as_deref()).await
}
