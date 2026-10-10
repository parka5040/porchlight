-- R58: S7a creates these relations after 0016. Purge reads them to scope
-- deletions and audit counts; DELETE was already granted by 0030.
SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
GRANT SELECT ON lists, list_entries, list_viewers, list_entry_checks,
    list_revisions, list_entry_revisions TO porchlight_maintenance;
-- R56a: explicit main-now calendar/planning retention privileges (also narrowly
-- granted by their creation migrations); no source/member/audit deletion added.
GRANT SELECT, DELETE ON ics_uid_map, schedule_suggestions TO porchlight_maintenance;
RESET search_path;
RESET ROLE;
