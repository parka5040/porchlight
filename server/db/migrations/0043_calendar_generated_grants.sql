SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
-- Raw agents configure subscriptions, but the scheduler owns validators/status/due cursors.
REVOKE INSERT,UPDATE ON ics_sources FROM porchlight_agent;
GRANT INSERT(id,household_id,owner_member_id,target_member_id,kind,import_key,name,url,allow_http,refresh_seconds,enabled,deleted_at)
    ON ics_sources TO porchlight_agent;
GRANT UPDATE(target_member_id,name,url,allow_http,refresh_seconds,enabled,deleted_at)
    ON ics_sources TO porchlight_agent;
RESET search_path;
RESET ROLE;
