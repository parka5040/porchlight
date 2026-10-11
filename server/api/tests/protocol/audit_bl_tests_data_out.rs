use porchlight_api::testing::{TestOptions, TestServer, trust};
use porchlight_db::tx::{Actor, begin_as};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{collections::BTreeSet, io::Read};
use uuid::Uuid;

#[sqlx::test(migrations = "../db/migrations")]
async fn final_catalog_and_household_export_invariants(pool: PgPool) {
    let s = TestServer::start_with(
        &pool,
        TestOptions {
            background_derivation: false,
            ..Default::default()
        },
    )
    .await;
    // Inspect the final catalog, not just the original migration's spelling.
    for (function, allowed, denied) in [
        (
            "porchlight.pl_retention(timestamptz)",
            "porchlight_app",
            "porchlight_agent",
        ),
        (
            "porchlight.pl_renew_blob_lease(uuid,interval)",
            "porchlight_backup",
            "porchlight_agent",
        ),
    ] {
        let (definer, config, can_run, forbidden): (bool, Vec<String>, bool, bool) = sqlx::query_as(
            "SELECT prosecdef,proconfig,has_function_privilege($2,oid,'EXECUTE'),has_function_privilege($3,oid,'EXECUTE') FROM pg_proc WHERE oid=$1::regprocedure",
        )
        .bind(function)
        .bind(allowed)
        .bind(denied)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(definer && can_run && !forbidden, "{function}");
        assert_eq!(config, vec!["search_path=pg_catalog, porchlight, pg_temp"]);
    }
    let suppressed: bool = sqlx::query_scalar(
        "SELECT self_suppressed FROM porchlight.notification_kinds WHERE kind='backup_stale'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!suppressed);
    let mut tx = begin_as(
        &s.state.pool,
        &Actor {
            member_id: Some(s.init.admin_member_id),
            ..Actor::system()
        },
    )
    .await
    .unwrap();
    let event: Option<Uuid> = sqlx::query_scalar(
        "SELECT porchlight.pl_emit_notification_event($1,$2,'backup_stale','household',$1)",
    )
    .bind(s.init.household_id)
    .bind(s.init.admin_member_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(event.is_some(), "backup-stale is not self-suppressed");
    tx.commit().await.unwrap();

    let response = trust::admin(&s, "GET", "/v1/admin/export/household", json!({})).await;
    assert_eq!(response.status, 200);
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(response.body.as_ref()));
    let mut tables = BTreeSet::new();
    let mut completed = false;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        assert!(!completed, "completion marker must be the final entry");
        let path = entry.path().unwrap().to_string_lossy().to_string();
        if let Some(table) = path
            .strip_prefix("tables/")
            .and_then(|p| p.strip_suffix(".jsonl"))
        {
            assert!(tables.insert(table.to_owned()));
        } else if path == "complete.json" {
            let mut text = String::new();
            entry.read_to_string(&mut text).unwrap();
            let manifest: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(manifest["complete"], true);
            let manifest_tables: BTreeSet<_> = manifest["table_rows"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect();
            assert_eq!(manifest_tables, tables);
            completed = true;
        }
    }
    assert!(completed);
    for required in [
        "scheduled_reminders",
        "comments",
        "attachments",
        "notification_event_delivery",
        "security_events",
    ] {
        assert!(
            tables.contains(required),
            "missing exported table: {required}"
        );
    }
    for secret in [
        "user_credentials",
        "sessions",
        "enrollment_tokens",
        "password_resets",
        "api_tokens",
        "step_ups",
        "idempotency_keys",
        "ics_feed_tokens",
        "install_state",
    ] {
        assert!(
            !tables.contains(secret),
            "secret table in exported archive: {secret}"
        );
    }
    s.stop().await;
}
