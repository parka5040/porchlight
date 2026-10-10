SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Stored simple-language documents are indexed, never ACL authority.
ALTER TABLE lists ADD COLUMN search_document tsvector
 GENERATED ALWAYS AS (to_tsvector('simple'::regconfig,name)) STORED;
CREATE INDEX lists_search ON lists USING gin(search_document) WHERE deleted_at IS NULL;
ALTER TABLE list_entries ADD COLUMN search_document tsvector
 GENERATED ALWAYS AS (to_tsvector('simple'::regconfig,label || ' ' || notes)) STORED;
CREATE INDEX list_entries_search ON list_entries USING gin(search_document) WHERE archived_at IS NULL;

RESET search_path;
RESET ROLE;
