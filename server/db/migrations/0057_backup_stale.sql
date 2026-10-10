-- R53: notification kinds are additive DATA; never redefine the shared emitter.
SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
INSERT INTO porchlight.notification_kinds(kind,self_suppressed,description)
VALUES('backup_stale',false,'No successful household backup for more than 26 hours')
ON CONFLICT (kind) DO NOTHING;
RESET search_path;
RESET ROLE;
