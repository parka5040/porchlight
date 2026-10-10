-- R55: privileges are migration state, never runtime GRANT/DDL.
SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
GRANT SELECT, DELETE ON notification_events TO porchlight_maintenance;
-- 0030 creates lists AFTER this migration and grants maintenance DELETE only.
-- R58 declares the needed SELECT grants in assigned 0060 after 0030;
-- no lazy runtime grant is acceptable.
RESET search_path;
RESET ROLE;
