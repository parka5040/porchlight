SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
-- Complete imports relinquish source-managed exceptions removed by their producer.
-- The label guard on this authoritative mapping table also covers deletes.
GRANT DELETE ON ics_uid_map TO porchlight_app;
RESET search_path;
RESET ROLE;
