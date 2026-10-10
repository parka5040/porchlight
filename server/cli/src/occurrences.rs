use crate::{
    client::{Client, Result},
    domain,
};
use clap::{Args, Subcommand};
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Args)]
pub struct Target {
    pub occurrence: Uuid,
    #[arg(long)]
    pub version: Option<i64>,
}
#[derive(Subcommand)]
pub enum Command {
    List {
        item: Uuid,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
    },
    Complete(Target),
    Reopen(Target),
    SetStatus {
        #[command(flatten)]
        target: Target,
        status: String,
    },
    Move {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        after: Option<Uuid>,
        #[arg(long)]
        before: Option<Uuid>,
    },
    Override {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        start: Option<chrono::NaiveDateTime>,
        #[arg(long)]
        end: Option<chrono::NaiveDateTime>,
        #[arg(long)]
        due: Option<chrono::NaiveDateTime>,
        #[arg(long)]
        due_date: Option<chrono::NaiveDate>,
    },
}
pub async fn run(c: &Client, cmd: &Command) -> Result<Value> {
    let (t, suffix, method, body) = match cmd {
        Command::List { item, from, to } => {
            return domain::collection(
                c,
                &domain::query(
                    &format!("/v1/items/{item}/occurrences"),
                    &[("from", from.clone()), ("to", to.clone())],
                ),
            )
            .await;
        }
        Command::Complete(t) => (t, "/complete", "POST", json!({})),
        Command::Reopen(t) => (t, "/reopen", "POST", json!({})),
        Command::SetStatus { target, status } => (
            target,
            "",
            "PATCH",
            json!({"status":status.replace('-',"_")}),
        ),
        Command::Move {
            target,
            after,
            before,
        } => (
            target,
            "",
            "PATCH",
            json!({"board_move":{"after_id":after,"before_id":before}}),
        ),
        Command::Override {
            target,
            start,
            end,
            due,
            due_date,
        } => {
            let mut b = json!({});
            for (k, v) in [
                ("override_local_start", json!(start)),
                ("override_local_end", json!(end)),
                ("override_local_due", json!(due)),
                ("override_due_date", json!(due_date)),
            ] {
                if !v.is_null() {
                    b[k] = v;
                }
            }
            (target, "", "PATCH", b)
        }
    };
    let version = domain::occurrence_version(c, t.occurrence, t.version).await?;
    c.request_version(
        method,
        &format!("/v1/occurrences/{}{suffix}", t.occurrence),
        body,
        Some(version),
    )
    .await
}
