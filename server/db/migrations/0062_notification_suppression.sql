-- M5: the app notifier deletes only unreleased digests when Digest is disabled.
-- No generated-source DELETE or authoritative-table privilege is added; source
-- links are removed by the existing notification FK cascade. Agents stay read-only.
SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
GRANT DELETE ON notifications TO porchlight_app;
RESET search_path;
RESET ROLE;
