//! Count server-side SQL statements, including nested trigger statements, with
//! PostgreSQL's bundled auto_explain. Each measurement rolls back its mutation
//! and optional restoration of the 0079 producers; released migrations stay intact.
use super::prerollout_sync_server_b_fix2::{history, lineage};
use porchlight_db::testing::seed_household;
use sqlx::{ConnectOptions, PgPool};
use std::{
    io::Write,
    process::{Command, Stdio},
};

fn original_producers() -> String {
    let sql = include_str!("../../migrations/0079_targeted_stream_integrity.sql");
    let start = sql
        .find("CREATE OR REPLACE FUNCTION pl_sync_items()")
        .unwrap();
    let end = sql.find("-- Sweeps and zone/offset").unwrap();
    let body = sql[start..end].replace(
        "CREATE FUNCTION pl_sync_actor_rename()",
        "CREATE OR REPLACE FUNCTION pl_sync_actor_rename()",
    );
    format!(
        "set local search_path=porchlight,pg_catalog; drop index notification_events_actor; drop trigger sync_actor_rename on household_members; drop trigger sync_items_update on items; create trigger sync_items_update after update on items referencing old table as changed_items for each statement execute function pl_sync_items(); {body}"
    )
}
fn measure(pool: &PgPool, restore: &str, mutation: &str) -> (usize, i64) {
    let script = format!(
        "begin; {restore}
select count(*) as before from porchlight.outbox \\gset
load 'auto_explain';
set local client_min_messages=notice;
set local auto_explain.log_level=notice;
set local auto_explain.log_format=json;
set local auto_explain.log_nested_statements=on;
set local auto_explain.log_analyze=on;
set local auto_explain.log_timing=off;
set local auto_explain.log_min_duration=0;
{mutation};
set local auto_explain.log_min_duration=-1;
select count(*)-:before from porchlight.outbox;
rollback;"
    );
    // Pass connection settings privately, retaining credentials/TLS for TCP
    // fixtures too. SQLx-only URI parameters are not accepted by libpq.
    let options = pool.connect_options();
    let mut url = options.to_url_lossy();
    let parameters: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| {
            matches!(
                key.as_ref(),
                "sslmode" | "sslrootcert" | "sslcert" | "sslkey" | "application_name" | "options"
            )
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    for (key, value) in parameters {
        url.query_pairs_mut().append_pair(&key, &value);
    }
    if let Some(socket) = options.get_socket() {
        url.query_pairs_mut()
            .append_pair("host", socket.to_str().unwrap());
    }
    let mut child = Command::new("psql")
        .arg(url.as_str())
        .args(["-X", "-qAt", "-v", "ON_ERROR_STOP=1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    if !output.status.success() {
        std::fs::write(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../var/sync-server-b-fix2-cost-error.log"
            ),
            &output.stderr,
        )
        .unwrap();
        panic!("auto_explain measurement failed; diagnostics in copy var log");
    }
    let notices = String::from_utf8(output.stderr).unwrap();
    let statements = notices.matches("\"Query Text\":").count();
    assert!(
        statements > 0,
        "auto_explain did not capture nested statements"
    );
    let outbox: i64 = String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    (statements, outbox)
}
#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn before_after_title_and_colour_statement_counts(pool: PgPool) {
    let h = seed_household(&pool).await.unwrap();
    let root = lineage(&pool, h.id, h.super_admin, 800).await;
    history(&pool, h.id, h.super_admin, h.user).await;
    sqlx::raw_sql("analyze porchlight.items; analyze porchlight.notification_events; analyze porchlight.notification_sources; analyze porchlight.notifications")
        .execute(&pool).await.unwrap();
    let original = original_producers();
    for (name, mutation) in [
        (
            "title",
            format!("update porchlight.items set title='Routine title edit' where id='{root}'"),
        ),
        (
            "colour",
            format!(
                "update porchlight.household_members set color='#ABCDEF' where id='{}'",
                h.super_admin
            ),
        ),
    ] {
        let before = measure(&pool, &original, &mutation);
        let after = measure(&pool, "", &mutation);
        eprintln!(
            "COST {name}: 0079 statements={} outbox={}; 0080 statements={} outbox={}",
            before.0, before.1, after.0, after.1
        );
        assert_eq!(
            before.1, 202,
            "baseline must include domain tickle, 200 extras and reset"
        );
        assert_eq!(after.1, 1, "routine edit must emit only its domain tickle");
        assert!(
            after.0 < before.0,
            "routine mutation query work did not improve"
        );
    }
}
