//! Each SQLx database loses exactly one producer; the normal differential proof
//! must reject its scenario. No switches are shipped in production SQL.
use super::*;

async fn suppress(m: &Model, producer: &str) {
    if producer == "pl_sync_emit" {
        sqlx::query("create or replace function porchlight.pl_sync_emit(p_household uuid,p_kind text,p_type text,p_id uuid,p_member uuid default null) returns void language plpgsql security definer set search_path=pg_catalog,porchlight,pg_temp as $$ begin return; end $$")
            .execute(&m.pool).await.unwrap();
    } else {
        let triggers:Vec<(String,String)> = sqlx::query_as("select c.relname::text,t.tgname::text from pg_trigger t join pg_class c on c.oid=t.tgrelid join pg_proc p on p.oid=t.tgfoid where c.relnamespace='porchlight'::regnamespace and (p.proname=$1 or t.tgname=$1) and not t.tgisinternal")
            .bind(producer).fetch_all(&m.pool).await.unwrap();
        assert!(!triggers.is_empty(), "unknown producer {producer}");
        for (table, trigger) in triggers {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "alter table porchlight.\"{table}\" disable trigger \"{trigger}\""
            )))
            .execute(&m.pool)
            .await
            .unwrap();
        }
    }
}
pub(super) fn matches_failure(
    error: &CoverageFailure,
    phase: &str,
    kind: FailureKind,
    id: Uuid,
) -> bool {
    error.phase == phase && error.kind == kind && error.id == id
}
async fn prepared(pool: PgPool, setup: &[&str]) -> Model {
    let mut m = fixture(pool, 1).await;
    for op in setup {
        m.mutate(op).await;
        m.drain().await;
    }
    m
}
async fn killed(pool: PgPool, producer: &str, setup: &[&str], scenario: &str, resource: &str) {
    let mut m = prepared(pool, setup).await;
    let phase = format!(
        "{scenario}/{}",
        if scenario == "availability_source" {
            "worker"
        } else {
            "source"
        }
    );
    let (kind, key) = match resource {
        "bundle" => {
            let key = match scenario {
                "raw_title" | "ancestor_private" | "list_edit" | "entry" => m.items[1],
                "dismiss" => id_field(&m.proposals[1], "item_id"),
                "availability_source" => m.items[2],
                _ => m.items[0],
            };
            (FailureKind::Bundle, key)
        }
        "notification" => {
            let key: Uuid = sqlx::query_scalar("select id from porchlight.notifications where recipient_member_id=$1 order by id limit 1")
                .bind(m.members[1]).fetch_one(&m.pool).await.unwrap();
            (FailureKind::Notification, key)
        }
        "settings availability" => (FailureKind::Settings("availability"), m.members[0]),
        "reset" => (FailureKind::Reset, m.s.init.household_id),
        _ => panic!("unknown expected resource"),
    };
    suppress(&m, producer).await;
    let result = checked_step(&mut m, scenario).await;
    m.s.stop().await;
    let error = result.expect_err(&format!("mutation survived: {producer} -> {scenario}"));
    assert!(
        matches_failure(&error, &phase, kind.clone(), key),
        "wrong failure for {producer}: {error}; expected phase={phase} kind={kind:?} id={key}"
    );
    eprintln!("KILLED {producer} -> {scenario}: {error}");
}
fn id_field(value: &Value, field: &str) -> Uuid {
    value[field].as_str().unwrap().parse().unwrap()
}
macro_rules! guards {
    ($name:ident,$producer:literal,[$($setup:literal),*],$scenario:literal,$resource:literal) => {
        mod $name {
            use super::*;
            #[sqlx::test(migrations = "../db/migrations")]
            async fn killed_producer(pool: PgPool) { killed(pool,$producer,&[$($setup),*],$scenario,$resource).await; }
            #[sqlx::test(migrations = "../db/migrations")]
            async fn positive_control(pool: PgPool) {
                let mut m = prepared(pool,&[$($setup),*]).await;
                let result = checked_step(&mut m,$scenario).await;
                m.s.stop().await;
                result.unwrap();
            }
        }
    };
}
guards!(
    emit_guards_raw_enqueue,
    "pl_sync_emit",
    ["create_shared"],
    "enqueue",
    "bundle"
);
guards!(
    pending_guards_source_commit,
    "pl_sync_pending",
    ["create_shared"],
    "enqueue",
    "bundle"
);
guards!(
    pending_guards_member_zone_source,
    "pl_sync_pending",
    ["create_shared"],
    "member_zone_source",
    "bundle"
);
guards!(
    items_guard_conflict_titles,
    "pl_sync_items",
    ["create_shared", "create_private"],
    "raw_title",
    "bundle"
);
guards!(
    items_guard_inherited_acl,
    "pl_sync_items",
    ["create_shared", "recurrence", "split", "list", "entry"],
    "ancestor_private",
    "bundle"
);
guards!(
    agenda_guards_old_and_new_peers,
    "pl_sync_agenda",
    ["create_shared", "create_private"],
    "agenda_move",
    "bundle"
);
guards!(
    load_guards_worker_completion,
    "pl_sync_load",
    ["create_shared", "create_private", "create_task"],
    "availability_source",
    "bundle"
);
guards!(
    suggestion_guards_item_bundle,
    "pl_sync_suggestion",
    [
        "create_shared",
        "create_private",
        "create_task",
        "create_task_peer",
        "availability_week",
        "suggest"
    ],
    "dismiss",
    "bundle"
);
guards!(
    list_guards_inherited_template,
    "pl_sync_list",
    ["create_shared", "recurrence", "split", "list", "entry"],
    "list_edit",
    "bundle"
);
guards!(
    notification_delete_guards_retention,
    "pl_sync_notification_delete",
    ["retention_seed"],
    "retention",
    "notification"
);
guards!(
    physical_delete_trigger_guards_absence,
    "sync_notifications_delete",
    ["actor_sources"],
    "notification_delete",
    "notification"
);
guards!(
    source_delete_trigger_guards_each_id,
    "sync_sources_delete",
    ["actor_sources"],
    "source_delete",
    "notification"
);
guards!(
    entry_trigger_guards_inherited_entry,
    "sync_entry",
    ["create_shared", "recurrence", "split", "list", "entry"],
    "entry",
    "bundle"
);
guards!(
    actor_guards_each_inbox_id,
    "pl_sync_actor_rename",
    ["actor_sources"],
    "actor_rename",
    "notification"
);
guards!(
    availability_guards_exact_settings,
    "pl_availability_profile_changed",
    [],
    "availability",
    "settings availability"
);
guards!(
    reset_guards_household_zone,
    "pl_sync_reset",
    [],
    "household_zone",
    "reset"
);
guards!(
    reset_guards_member_deletion,
    "pl_sync_reset",
    ["member_seed"],
    "member_delete",
    "reset"
);
