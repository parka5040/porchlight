//! Catalog invariants that must hold for every table and function, including ones added by later migrations.
//! (Version-guard coverage of every table with a `version` column lives in version_guard.rs.)
use sqlx::PgPool;
use std::collections::BTreeSet;

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn table_touching_and_definer_functions_pin_search_path(pool: PgPool) {
    // pg_temp must be listed last: when omitted it is searched first, so callers could shadow our tables.
    let functions: Vec<(String, bool)> = sqlx::query_as(
        "select p.oid::regprocedure::text, \
                coalesce('search_path=pg_catalog, porchlight, pg_temp' = any(p.proconfig), false) \
         from pg_proc p \
         where p.pronamespace = 'porchlight'::regnamespace \
           and not exists (select 1 from pg_depend d where d.classid = 'pg_proc'::regclass \
                           and d.objid = p.oid and d.deptype = 'e') \
           and (p.prosecdef \
                or exists (select 1 from pg_class c \
                           where c.relnamespace = 'porchlight'::regnamespace \
                             and c.relkind in ('r', 'p', 'v', 'm', 'S') \
                             and p.prosrc ~ ('\\m' || c.relname || '\\M'))) \
         order by 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let unpinned: Vec<_> = functions.iter().filter(|(_, ok)| !ok).collect();
    assert!(unpinned.is_empty(), "missing search_path: {unpinned:?}");
    // Non-vacuous: a definer trigger and a plain helper that reads tables are both covered.
    for f in ["pl_items_audit()", "pl_item_snapshot(uuid)"] {
        assert!(
            functions.iter().any(|(name, _)| name.ends_with(f)),
            "{f} not checked: {functions:?}"
        );
    }
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn every_table_the_agent_writes_requires_the_agent_label(pool: PgPool) {
    let tables: Vec<(String, bool)> = sqlx::query_as(
        "select c.relname::text, \
                exists (select 1 from pg_trigger t where t.tgrelid = c.oid and t.tgname = 'aa_require_agent_label' \
                        and t.tgfoid = 'porchlight.pl_require_agent_label'::regproc \
                        and t.tgtype = 2 | 4 | 8 | 16) \
         from pg_class c \
         where c.relnamespace = 'porchlight'::regnamespace and c.relkind in ('r', 'p') \
           and (has_table_privilege('porchlight_agent', c.oid, 'INSERT, UPDATE, DELETE') \
                or has_any_column_privilege('porchlight_agent', c.oid, 'INSERT, UPDATE')) \
         order by 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let unlabelled: Vec<_> = tables.iter().filter(|(_, ok)| !ok).collect();
    assert!(
        unlabelled.is_empty(),
        "agent-writable without aa_require_agent_label (BEFORE INSERT OR UPDATE OR DELETE, per statement): \
         {unlabelled:?}"
    );
    let names: Vec<&str> = tables.iter().map(|(t, _)| t.as_str()).collect();
    assert!(
        names.contains(&"items") && names.contains(&"item_occurrences"),
        "{names:?}"
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn every_object_in_the_schema_belongs_to_the_owner(pool: PgPool) {
    let foreign: Vec<(String, String, String)> = sqlx::query_as(
        "with objects (kind, name, owner, classid, oid) as ( \
             select 'schema', n.nspname::text, n.nspowner, 'pg_namespace'::regclass, n.oid \
             from pg_namespace n where n.nspname = 'porchlight' \
             union all \
             select 'relation', c.oid::regclass::text, c.relowner, 'pg_class'::regclass, c.oid \
             from pg_class c where c.relnamespace = 'porchlight'::regnamespace \
             union all \
             select 'function', p.oid::regprocedure::text, p.proowner, 'pg_proc'::regclass, p.oid \
             from pg_proc p where p.pronamespace = 'porchlight'::regnamespace \
             union all \
             select 'type', t.oid::regtype::text, t.typowner, 'pg_type'::regclass, t.oid \
             from pg_type t where t.typnamespace = 'porchlight'::regnamespace \
               and not exists (select 1 from pg_depend d where d.classid = 'pg_type'::regclass \
                               and d.objid = t.typelem and d.deptype = 'e') \
             union all \
             select 'operator', o.oid::regoperator::text, o.oprowner, 'pg_operator'::regclass, o.oid \
             from pg_operator o where o.oprnamespace = 'porchlight'::regnamespace \
             union all \
             select 'operator class', oc.opcname::text, oc.opcowner, 'pg_opclass'::regclass, oc.oid \
             from pg_opclass oc where oc.opcnamespace = 'porchlight'::regnamespace \
             union all \
             select 'operator family', f.opfname::text, f.opfowner, 'pg_opfamily'::regclass, f.oid \
             from pg_opfamily f where f.opfnamespace = 'porchlight'::regnamespace \
             union all \
             select 'collation', co.collname::text, co.collowner, 'pg_collation'::regclass, co.oid \
             from pg_collation co where co.collnamespace = 'porchlight'::regnamespace \
             union all \
             select 'statistics', s.stxname::text, s.stxowner, 'pg_statistic_ext'::regclass, s.oid \
             from pg_statistic_ext s where s.stxnamespace = 'porchlight'::regnamespace \
         ) \
         select o.kind, o.name, o.owner::regrole::text from objects o \
         where o.owner <> 'porchlight_owner'::regrole \
           and not exists (select 1 from pg_depend d where d.classid = o.classid and d.objid = o.oid \
                           and d.deptype = 'e') \
         order by 1, 2",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        foreign,
        vec![
            (
                "function".into(),
                "pl_purge_items(uuid,uuid[])".into(),
                "porchlight_maintenance".into()
            ),
            (
                "function".into(),
                "pl_retention(timestamp with time zone)".into(),
                "porchlight_maintenance".into()
            )
        ]
    );
}

/// Every privilege on a porchlight relation held by a non-owner role. Add new grants here deliberately.
// 0061 adds only a trigger-owned parent guard: no runtime function/table/column grants.
// In particular the existing agent relationship SELECT INSERT DELETE grants stay unchanged.
// 0063 reorders the existing maintenance-owned purge plan only: EXPECTED_GRANTS has no delta.
// 0064 replaces the existing trigger body and adds UID/status CHECKs and internal timeout state: its ACL is unchanged, with
// no new executable routine or table privilege to add to this exhaustive list.
const EXPECTED_GRANTS: &[(&str, &str, &str)] = &[
    // 0070: administrative security history; actor/time/identity stay database-owned.
    ("porchlight_app", "security_events", "SELECT"),
    ("porchlight_app", "security_events(household_id)", "INSERT"),
    ("porchlight_app", "security_events(kind)", "INSERT"),
    ("porchlight_app", "security_events(outcome)", "INSERT"),
    ("porchlight_app", "security_events(target_id)", "INSERT"),
    ("porchlight_app", "security_events(target_type)", "INSERT"),
    ("porchlight_maintenance", "security_events", "SELECT DELETE"),
    (
        "porchlight_maintenance",
        "blobs(last_referenced_at)",
        "UPDATE",
    ),
    (
        "porchlight_maintenance",
        "notification_events(id)",
        "UPDATE",
    ),
    ("porchlight_maintenance", "sessions(id)", "UPDATE"),
    ("porchlight_maintenance", "notifications(item_id)", "UPDATE"),
    (
        "porchlight_maintenance",
        "notifications(occurrence_id)",
        "UPDATE",
    ),
    // o2a: narrow maintenance paths; app/agent cannot mutate the watermark.
    ("porchlight_maintenance", "notifications", "SELECT DELETE"),
    (
        "porchlight_maintenance",
        "notification_sources",
        "SELECT DELETE",
    ),
    (
        "porchlight_maintenance",
        "notification_event_delivery",
        "SELECT DELETE",
    ),
    ("porchlight_maintenance", "sessions", "SELECT DELETE"),
    (
        "porchlight_maintenance",
        "enrollment_tokens",
        "SELECT DELETE",
    ),
    ("porchlight_maintenance", "password_resets", "SELECT DELETE"),
    ("porchlight_maintenance", "api_tokens", "SELECT DELETE"),
    ("porchlight_maintenance", "step_ups", "SELECT DELETE"),
    ("porchlight_maintenance", "outbox", "SELECT DELETE"),
    (
        "porchlight_maintenance",
        "stream_assign_mutex",
        "SELECT UPDATE",
    ),
    ("porchlight_maintenance", "households", "SELECT"),
    (
        "porchlight_maintenance",
        "stream_retention",
        "SELECT UPDATE",
    ),
    ("porchlight_app", "stream_retention", "SELECT"),
    ("porchlight_agent", "stream_retention", "SELECT"),
    (
        "porchlight_maintenance",
        "notification_events",
        "SELECT DELETE",
    ),
    ("porchlight_maintenance", "item_assignees", "SELECT DELETE"),
    ("porchlight_maintenance", "item_viewers", "SELECT DELETE"),
    ("porchlight_maintenance", "item_reminders", "SELECT DELETE"),
    ("porchlight_maintenance", "member_agenda", "SELECT DELETE"),
    (
        "porchlight_maintenance",
        "scheduled_reminders",
        "SELECT DELETE",
    ),
    ("porchlight_maintenance", "item_revisions", "SELECT DELETE"),
    ("porchlight_maintenance", "task_events", "SELECT DELETE"),
    ("porchlight_maintenance", "items", "SELECT UPDATE DELETE"),
    (
        "porchlight_maintenance",
        "item_occurrences",
        "SELECT UPDATE DELETE",
    ),
    (
        "porchlight_maintenance",
        "item_derivation",
        "SELECT UPDATE DELETE",
    ),
    ("porchlight_maintenance", "purge_log", "SELECT INSERT"),
    ("porchlight_app", "purge_log", "SELECT"),
    ("porchlight_agent", "purge_log", "SELECT"),
    (
        "porchlight_app",
        "notification_delivery_retry",
        "SELECT INSERT UPDATE DELETE",
    ),
    (
        "porchlight_app",
        "notification_event_delivery",
        "SELECT INSERT UPDATE DELETE",
    ),
    // 0065 only expands retry constraints (zero grace strikes / cancellation class);
    // app writes and agent inspection grants remain exactly those from 0023.
    ("porchlight_agent", "notification_event_delivery", "SELECT"),
    // s6a: private generated inbox/source links, canonical self prefs.
    (
        "porchlight_app",
        "notifications",
        "SELECT INSERT UPDATE DELETE",
    ),
    ("porchlight_app", "notification_sources", "SELECT INSERT"),
    ("porchlight_app", "notification_prefs", "SELECT UPDATE"),
    ("porchlight_agent", "notifications", "SELECT"),
    ("porchlight_agent", "notification_sources", "SELECT"),
    ("porchlight_agent", "notification_prefs", "SELECT"),
    // s7b: comments, blob metadata and operational GC leases.
    ("porchlight_app", "blob_storage_locks", "SELECT"),
    ("porchlight_agent", "blob_storage_locks", "SELECT"),
    ("porchlight_app", "blob_storage_locks(shard)", "UPDATE"),
    ("porchlight_agent", "blob_storage_locks(shard)", "UPDATE"),
    ("porchlight_app", "comments", "SELECT"),
    ("porchlight_app", "collaboration_audit", "SELECT"),
    ("porchlight_agent", "comments", "SELECT"),
    ("porchlight_agent", "collaboration_audit", "SELECT"),
    ("porchlight_app", "comments(id)", "INSERT"),
    ("porchlight_app", "comments(household_id)", "INSERT"),
    ("porchlight_app", "comments(item_id)", "INSERT"),
    ("porchlight_app", "comments(author_id)", "INSERT"),
    ("porchlight_app", "comments(author_token_id)", "INSERT"),
    ("porchlight_app", "comments(body)", "INSERT"),
    ("porchlight_app", "comments(mentions)", "INSERT"),
    ("porchlight_app", "comments(body)", "UPDATE"),
    ("porchlight_app", "comments(mentions)", "UPDATE"),
    ("porchlight_app", "comments(edited_at)", "UPDATE"),
    ("porchlight_app", "comments(deleted_at)", "UPDATE"),
    ("porchlight_agent", "comments(id)", "INSERT"),
    ("porchlight_agent", "comments(household_id)", "INSERT"),
    ("porchlight_agent", "comments(item_id)", "INSERT"),
    ("porchlight_agent", "comments(author_id)", "INSERT"),
    ("porchlight_agent", "comments(author_token_id)", "INSERT"),
    ("porchlight_agent", "comments(body)", "INSERT"),
    ("porchlight_agent", "comments(mentions)", "INSERT"),
    ("porchlight_agent", "comments(body)", "UPDATE"),
    ("porchlight_agent", "comments(mentions)", "UPDATE"),
    ("porchlight_agent", "comments(edited_at)", "UPDATE"),
    ("porchlight_agent", "comments(deleted_at)", "UPDATE"),
    ("porchlight_maintenance", "comments", "SELECT DELETE"),
    (
        "porchlight_maintenance",
        "collaboration_audit",
        "SELECT DELETE",
    ),
    ("porchlight_app", "blobs", "SELECT"),
    ("porchlight_app", "attachments", "SELECT"),
    ("porchlight_agent", "blobs", "SELECT"),
    ("porchlight_agent", "attachments", "SELECT"),
    ("porchlight_app", "blobs(household_id)", "INSERT"),
    ("porchlight_app", "blobs(sha256)", "INSERT"),
    ("porchlight_app", "blobs(size)", "INSERT"),
    ("porchlight_app", "blobs(content_type)", "INSERT"),
    ("porchlight_app", "blobs(last_referenced_at)", "UPDATE"),
    ("porchlight_app", "blobs(size)", "UPDATE"),
    ("porchlight_app", "blobs(content_type)", "UPDATE"),
    ("porchlight_agent", "blobs(household_id)", "INSERT"),
    ("porchlight_agent", "blobs(sha256)", "INSERT"),
    ("porchlight_agent", "blobs(size)", "INSERT"),
    ("porchlight_agent", "blobs(content_type)", "INSERT"),
    ("porchlight_agent", "blobs(last_referenced_at)", "UPDATE"),
    ("porchlight_agent", "blobs(size)", "UPDATE"),
    ("porchlight_agent", "blobs(content_type)", "UPDATE"),
    ("porchlight_app", "attachments(id)", "INSERT"),
    ("porchlight_app", "attachments(household_id)", "INSERT"),
    ("porchlight_app", "attachments(item_id)", "INSERT"),
    ("porchlight_app", "attachments(comment_id)", "INSERT"),
    ("porchlight_app", "attachments(blob_sha256)", "INSERT"),
    ("porchlight_app", "attachments(thumbnail_sha256)", "INSERT"),
    ("porchlight_app", "attachments(original_name)", "INSERT"),
    ("porchlight_app", "attachments(uploader_id)", "INSERT"),
    ("porchlight_app", "attachments(uploader_token_id)", "INSERT"),
    ("porchlight_app", "attachments(deleted_at)", "UPDATE"),
    ("porchlight_agent", "attachments(id)", "INSERT"),
    ("porchlight_agent", "attachments(household_id)", "INSERT"),
    ("porchlight_agent", "attachments(item_id)", "INSERT"),
    ("porchlight_agent", "attachments(comment_id)", "INSERT"),
    ("porchlight_agent", "attachments(blob_sha256)", "INSERT"),
    (
        "porchlight_agent",
        "attachments(thumbnail_sha256)",
        "INSERT",
    ),
    ("porchlight_agent", "attachments(original_name)", "INSERT"),
    ("porchlight_agent", "attachments(uploader_id)", "INSERT"),
    (
        "porchlight_agent",
        "attachments(uploader_token_id)",
        "INSERT",
    ),
    ("porchlight_agent", "attachments(deleted_at)", "UPDATE"),
    ("porchlight_maintenance", "blobs", "SELECT DELETE"),
    ("porchlight_maintenance", "attachments", "SELECT DELETE"),
    ("porchlight_app", "blobs", "DELETE"),
    ("porchlight_app", "blob_leases", "SELECT DELETE"),
    ("porchlight_app", "blob_lease_items", "SELECT DELETE"),
    ("porchlight_backup", "blob_leases", "SELECT DELETE"),
    ("porchlight_backup", "blob_lease_items", "SELECT DELETE"),
    ("porchlight_maintenance", "blob_leases", "SELECT DELETE"),
    (
        "porchlight_maintenance",
        "blob_lease_items",
        "SELECT DELETE",
    ),
    // S8a: authoritative sources, service-only identity map, append-only history.
    ("porchlight_app", "ics_sources", "SELECT INSERT UPDATE"),
    ("porchlight_agent", "ics_sources", "SELECT"),
    ("porchlight_agent", "ics_sources(id)", "INSERT"),
    ("porchlight_agent", "ics_sources(household_id)", "INSERT"),
    ("porchlight_agent", "ics_sources(owner_member_id)", "INSERT"),
    ("porchlight_agent", "ics_sources(kind)", "INSERT"),
    ("porchlight_agent", "ics_sources(import_key)", "INSERT"),
    (
        "porchlight_agent",
        "ics_sources(target_member_id)",
        "INSERT UPDATE",
    ),
    ("porchlight_agent", "ics_sources(name)", "INSERT UPDATE"),
    ("porchlight_agent", "ics_sources(url)", "INSERT UPDATE"),
    (
        "porchlight_agent",
        "ics_sources(allow_http)",
        "INSERT UPDATE",
    ),
    (
        "porchlight_agent",
        "ics_sources(refresh_seconds)",
        "INSERT UPDATE",
    ),
    ("porchlight_agent", "ics_sources(enabled)", "INSERT UPDATE"),
    (
        "porchlight_agent",
        "ics_sources(visibility)",
        "INSERT UPDATE",
    ),
    (
        "porchlight_agent",
        "ics_sources(deleted_at)",
        "INSERT UPDATE",
    ),
    (
        "porchlight_app",
        "ics_uid_map",
        "SELECT INSERT UPDATE DELETE",
    ),
    ("porchlight_agent", "ics_uid_map", "SELECT"),
    ("porchlight_app", "ics_source_revisions", "SELECT"),
    ("porchlight_agent", "ics_source_revisions", "SELECT"),
    ("porchlight_maintenance", "ics_sources", "SELECT DELETE"),
    ("porchlight_maintenance", "ics_uid_map", "SELECT DELETE"),
    (
        "porchlight_maintenance",
        "ics_source_revisions",
        "SELECT DELETE",
    ),
    ("porchlight_app", "notification_kinds", "SELECT"),
    ("porchlight_agent", "notification_kinds", "SELECT"),
    // s8b: planning authoritative/generated split.
    ("porchlight_app", "availability_profiles", "SELECT"),
    ("porchlight_app", "availability_windows", "SELECT DELETE"),
    ("porchlight_app", "schedule_suggestions", "SELECT DELETE"),
    ("porchlight_app", "planning_audit", "SELECT"),
    ("porchlight_app", "member_load_jobs", "SELECT"),
    ("porchlight_app", "member_load_signals", "SELECT DELETE"),
    (
        "porchlight_maintenance",
        "member_load_signals",
        "SELECT DELETE",
    ),
    ("porchlight_app", "member_load_jobs(attempts)", "UPDATE"),
    ("porchlight_app", "member_load_jobs(retry_at)", "UPDATE"),
    ("porchlight_app", "member_load_jobs(last_error)", "UPDATE"),
    (
        "porchlight_app",
        "member_daily_load",
        "SELECT INSERT UPDATE DELETE",
    ),
    ("porchlight_agent", "availability_profiles", "SELECT"),
    ("porchlight_agent", "availability_windows", "SELECT DELETE"),
    ("porchlight_agent", "schedule_suggestions", "SELECT"),
    ("porchlight_agent", "planning_audit", "SELECT"),
    ("porchlight_agent", "member_load_jobs", "SELECT"),
    ("porchlight_agent", "member_daily_load", "SELECT"),
    (
        "porchlight_maintenance",
        "availability_profiles",
        "SELECT DELETE",
    ),
    (
        "porchlight_maintenance",
        "availability_windows",
        "SELECT DELETE",
    ),
    (
        "porchlight_maintenance",
        "schedule_suggestions",
        "SELECT DELETE",
    ),
    ("porchlight_maintenance", "planning_audit", "SELECT DELETE"),
    (
        "porchlight_maintenance",
        "member_load_jobs",
        "SELECT DELETE",
    ),
    (
        "porchlight_maintenance",
        "member_daily_load",
        "SELECT DELETE",
    ),
    (
        "porchlight_app",
        "availability_windows(household_id)",
        "INSERT",
    ),
    (
        "porchlight_app",
        "availability_windows(member_id)",
        "INSERT",
    ),
    (
        "porchlight_app",
        "availability_windows(weekday)",
        "INSERT UPDATE",
    ),
    (
        "porchlight_app",
        "availability_windows(starts_local)",
        "INSERT UPDATE",
    ),
    (
        "porchlight_app",
        "availability_windows(ends_local)",
        "INSERT UPDATE",
    ),
    ("porchlight_app", "schedule_suggestions(status)", "UPDATE"),
    (
        "porchlight_agent",
        "availability_windows(household_id)",
        "INSERT",
    ),
    (
        "porchlight_agent",
        "availability_windows(member_id)",
        "INSERT",
    ),
    (
        "porchlight_agent",
        "availability_windows(weekday)",
        "INSERT UPDATE",
    ),
    (
        "porchlight_agent",
        "availability_windows(starts_local)",
        "INSERT UPDATE",
    ),
    (
        "porchlight_agent",
        "availability_windows(ends_local)",
        "INSERT UPDATE",
    ),
    ("porchlight_agent", "schedule_suggestions(status)", "UPDATE"),
    (
        "porchlight_app",
        "schedule_suggestions(household_id)",
        "INSERT",
    ),
    (
        "porchlight_app",
        "schedule_suggestions(member_id)",
        "INSERT",
    ),
    ("porchlight_app", "schedule_suggestions(item_id)", "INSERT"),
    (
        "porchlight_app",
        "schedule_suggestions(occurrence_id)",
        "INSERT",
    ),
    (
        "porchlight_app",
        "schedule_suggestions(proposed_start)",
        "INSERT",
    ),
    (
        "porchlight_app",
        "schedule_suggestions(proposed_end)",
        "INSERT",
    ),
    ("porchlight_app", "schedule_suggestions(run_id)", "INSERT"),
    (
        "porchlight_app",
        "availability_profiles(member_id)",
        "INSERT",
    ),
    (
        "porchlight_app",
        "availability_profiles(household_id)",
        "INSERT",
    ),
    (
        "porchlight_app",
        "availability_profiles(updated_at)",
        "UPDATE",
    ),
    ("porchlight_app", "member_load_jobs(applied_gen)", "UPDATE"),
    // s7a: list authoritative and history relations.
    ("porchlight_app", "lists", "SELECT INSERT UPDATE"),
    ("porchlight_app", "list_entries", "SELECT INSERT UPDATE"),
    ("porchlight_app", "list_viewers", "SELECT INSERT DELETE"),
    (
        "porchlight_app",
        "list_entry_checks",
        "SELECT INSERT DELETE",
    ),
    ("porchlight_app", "list_revisions", "SELECT"),
    ("porchlight_app", "list_entry_revisions", "SELECT"),
    ("porchlight_agent", "lists", "SELECT INSERT UPDATE"),
    ("porchlight_agent", "list_entries", "SELECT INSERT UPDATE"),
    ("porchlight_agent", "list_viewers", "SELECT INSERT DELETE"),
    (
        "porchlight_agent",
        "list_entry_checks",
        "SELECT INSERT DELETE",
    ),
    ("porchlight_agent", "list_revisions", "SELECT"),
    ("porchlight_agent", "list_entry_revisions", "SELECT"),
    ("porchlight_maintenance", "lists", "SELECT DELETE"),
    ("porchlight_maintenance", "list_entries", "SELECT DELETE"),
    ("porchlight_maintenance", "list_viewers", "SELECT DELETE"),
    (
        "porchlight_maintenance",
        "list_entry_checks",
        "SELECT DELETE",
    ),
    ("porchlight_maintenance", "list_revisions", "SELECT DELETE"),
    (
        "porchlight_maintenance",
        "list_entry_revisions",
        "SELECT DELETE",
    ),
    ("porchlight_app", "notification_events", "SELECT"),
    (
        "porchlight_app",
        "notification_events(processed_at)",
        "UPDATE",
    ),
    ("porchlight_agent", "notification_events", "SELECT"),
    // O1: backup role reads through pg_read_all_data (implicit, not relation ACLs).
    ("porchlight_backup", "backup_runs", "INSERT UPDATE"),
    ("porchlight_app", "backup_runs", "SELECT"),
    ("porchlight_agent", "backup_runs", "SELECT"),
    ("porchlight_app", "api_tokens", "SELECT INSERT UPDATE"),
    ("porchlight_app", "device_certificates", "SELECT INSERT"),
    (
        "porchlight_app",
        "device_certificates(first_seen_at)",
        "UPDATE",
    ),
    (
        "porchlight_app",
        "device_certificates(revoked_at)",
        "UPDATE",
    ),
    ("porchlight_app", "devices", "SELECT INSERT UPDATE"),
    (
        "porchlight_app",
        "enrollment_tokens",
        "SELECT INSERT UPDATE DELETE",
    ),
    (
        "porchlight_app",
        "household_members",
        "SELECT INSERT UPDATE",
    ),
    ("porchlight_app", "households", "SELECT INSERT UPDATE"),
    (
        "porchlight_app",
        "idempotency_keys",
        "SELECT INSERT UPDATE DELETE",
    ),
    ("porchlight_app", "install_state", "SELECT INSERT"),
    ("porchlight_app", "item_assignees", "SELECT INSERT DELETE"),
    (
        "porchlight_app",
        "item_derivation",
        "SELECT INSERT UPDATE DELETE",
    ),
    (
        "porchlight_app",
        "item_occurrences",
        "SELECT INSERT UPDATE DELETE",
    ),
    ("porchlight_app", "item_reminders", "SELECT INSERT DELETE"),
    ("porchlight_app", "item_revisions", "SELECT"),
    ("porchlight_app", "item_viewers", "SELECT INSERT DELETE"),
    ("porchlight_app", "items", "SELECT INSERT UPDATE"),
    (
        "porchlight_app",
        "member_agenda",
        "SELECT INSERT UPDATE DELETE",
    ),
    ("porchlight_app", "member_settings", "SELECT INSERT UPDATE"),
    ("porchlight_app", "outbox", "SELECT"),
    ("porchlight_app", "outbox(stream_seq)", "UPDATE"),
    ("porchlight_app", "outbox_stream_seq", "USAGE"),
    (
        "porchlight_app",
        "password_resets",
        "SELECT INSERT UPDATE DELETE",
    ),
    (
        "porchlight_app",
        "scheduled_reminders",
        "SELECT INSERT UPDATE DELETE",
    ),
    ("porchlight_app", "scheduler_jobs", "SELECT INSERT UPDATE"),
    ("porchlight_app", "sessions", "SELECT INSERT UPDATE DELETE"),
    ("porchlight_app", "step_ups", "SELECT INSERT UPDATE DELETE"),
    ("porchlight_app", "stream_assign_mutex", "SELECT UPDATE"),
    ("porchlight_app", "task_events", "SELECT"),
    ("porchlight_app", "user_credentials", "SELECT INSERT UPDATE"),
    ("porchlight_app", "users", "SELECT INSERT UPDATE"),
    ("porchlight_agent", "household_members", "SELECT"),
    ("porchlight_agent", "households", "SELECT"),
    ("porchlight_agent", "item_assignees", "SELECT INSERT DELETE"),
    ("porchlight_agent", "item_derivation", "SELECT"),
    ("porchlight_agent", "item_occurrences", "SELECT"),
    (
        "porchlight_agent",
        "item_occurrences(board_position)",
        "UPDATE",
    ),
    (
        "porchlight_agent",
        "item_occurrences(completed_at)",
        "UPDATE",
    ),
    (
        "porchlight_agent",
        "item_occurrences(completed_by)",
        "UPDATE",
    ),
    (
        "porchlight_agent",
        "item_occurrences(override_due_date)",
        "UPDATE",
    ),
    (
        "porchlight_agent",
        "item_occurrences(override_local_due)",
        "UPDATE",
    ),
    (
        "porchlight_agent",
        "item_occurrences(override_local_end)",
        "UPDATE",
    ),
    (
        "porchlight_agent",
        "item_occurrences(override_local_start)",
        "UPDATE",
    ),
    ("porchlight_agent", "item_occurrences(status)", "UPDATE"),
    ("porchlight_agent", "item_reminders", "SELECT INSERT DELETE"),
    ("porchlight_agent", "item_revisions", "SELECT"),
    ("porchlight_agent", "item_viewers", "SELECT INSERT DELETE"),
    ("porchlight_agent", "items", "SELECT INSERT UPDATE"),
    ("porchlight_agent", "member_agenda", "SELECT"),
    (
        "porchlight_agent",
        "member_settings",
        "SELECT INSERT UPDATE",
    ),
    ("porchlight_agent", "scheduled_reminders", "SELECT"),
    ("porchlight_agent", "scheduler_jobs", "SELECT"),
    ("porchlight_agent", "task_events", "SELECT"),
    ("porchlight_agent", "users", "SELECT"),
];

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn privileges_match_the_snapshot(pool: PgPool) {
    // Table, sequence and column ACLs for every role but the owner (including PUBLIC, which must hold none).
    let actual: BTreeSet<String> = sqlx::query_scalar(
        "with acl (grantee, object, privilege) as ( \
             select a.grantee, c.relname::text, a.privilege_type \
             from pg_class c, aclexplode(c.relacl) a \
             where c.relnamespace = 'porchlight'::regnamespace \
             union all \
             select a.grantee, c.relname || '(' || at.attname || ')', a.privilege_type \
             from pg_attribute at join pg_class c on c.oid = at.attrelid, aclexplode(at.attacl) a \
             where c.relnamespace = 'porchlight'::regnamespace and at.attnum > 0 and not at.attisdropped \
         ) \
         select format('%s %s %s', case when grantee = 0 then 'PUBLIC' else grantee::regrole::text end, \
                       object, privilege) \
         from acl where grantee <> 'porchlight_owner'::regrole",
    )
    .fetch_all(&pool)
    .await
    .unwrap()
    .into_iter()
    .collect();
    let expected: BTreeSet<String> = EXPECTED_GRANTS
        .iter()
        .flat_map(|(role, object, privileges)| {
            privileges
                .split(' ')
                .map(move |p| format!("{role} {object} {p}"))
        })
        .collect();
    let unexpected: Vec<_> = actual.difference(&expected).collect();
    let missing: Vec<_> = expected.difference(&actual).collect();
    assert!(
        unexpected.is_empty() && missing.is_empty(),
        "unexpected grants: {unexpected:#?}\nmissing grants: {missing:#?}"
    );
}

#[sqlx::test(migrator = "porchlight_db::MIGRATOR")]
async fn child_table_after_triggers_fire_parent_bump_first(pool: PgPool) {
    // Same-timing triggers fire in name order; the tNN_ prefixes encode the lock order (see 0007).
    let triggers: Vec<(String, String)> = sqlx::query_as(
        "select c.relname::text, t.tgname::text from pg_trigger t join pg_class c on c.oid = t.tgrelid \
         where c.relnamespace = 'porchlight'::regnamespace \
           and c.relname in ('item_assignees', 'item_viewers', 'item_reminders') \
           and not t.tgisinternal and t.tgtype & 2 = 0 \
         order by 1, 2",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    for table in ["item_assignees", "item_viewers", "item_reminders"] {
        let names: Vec<&str> = triggers
            .iter()
            .filter(|(t, _)| t == table)
            .map(|(_, n)| n.as_str())
            .collect();
        assert!(
            names
                .first()
                .is_some_and(|n| n.starts_with("t10_children_changed_")),
            "{table}: {names:?}"
        );
        for name in &names {
            let prefix = name.split('_').next().unwrap_or("");
            assert!(
                prefix.len() == 3
                    && prefix.starts_with('t')
                    && prefix[1..].bytes().all(|b| b.is_ascii_digit()),
                "{table}: AFTER trigger {name} lacks a tNN_ order prefix"
            );
        }
    }
}
