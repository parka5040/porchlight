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
    /// Seconds to wait for export preparation before response headers (default: 15 minutes).
    #[arg(long, default_value_t = 900, value_parser = clap::value_parser!(u64).range(1..=86400))]
    pub prepare_timeout_seconds: u64,
}
pub async fn run(c: &Client, args: &Export) -> Result<serde_json::Value> {
    let path = if let Some(who) = &args.selection.member {
        let id = crate::domain::member(c, who).await?;
        format!("/v1/export/member?member_id={id}")
    } else {
        "/v1/admin/export/household".to_owned()
    };
    eprintln!("Preparing export…");
    c.download(
        &path,
        args.out.as_deref(),
        std::time::Duration::from_secs(args.prepare_timeout_seconds),
    )
    .await
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    #[test]
    fn export_deadline_defaults_to_fifteen_minutes_without_changing_normal_requests() {
        let args = crate::Args::try_parse_from(["porch", "export", "--household"]).unwrap();
        let crate::Command::Export(export) = args.command else {
            panic!("export selection")
        };
        assert_eq!(export.prepare_timeout_seconds, 900);
        assert_eq!(crate::client::DEADLINE.as_secs(), 15);
        for value in ["0", "86401"] {
            assert!(
                crate::Args::try_parse_from([
                    "porch",
                    "export",
                    "--household",
                    "--prepare-timeout-seconds",
                    value
                ])
                .is_err()
            );
        }
    }
}
