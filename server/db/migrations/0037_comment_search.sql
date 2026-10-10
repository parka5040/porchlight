-- S7b: extend S4b's generated simple-language FTS projection to comment bodies.
SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
ALTER TABLE comments ADD COLUMN search_document tsvector
 GENERATED ALWAYS AS (to_tsvector('simple', body)) STORED;
DROP INDEX comments_search;
CREATE INDEX comments_search_document ON comments USING gin(search_document) WHERE deleted_at IS NULL;
-- search_document is generated and never an authoritative mutation/notification input.
RESET search_path;
RESET ROLE;
