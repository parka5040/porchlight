-- M7: source configuration must not acquire deleted-item locks after members.
SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

CREATE OR REPLACE FUNCTION pl_ics_config_items() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,porchlight,pg_temp AS $$
DECLARE desired_owner uuid;
BEGIN
    IF NEW.kind <> 'feed' OR (NEW.visibility,NEW.name,NEW.target_member_id,NEW.deleted_at) IS NOT DISTINCT FROM (OLD.visibility,OLD.name,OLD.target_member_id,OLD.deleted_at) THEN RETURN NEW; END IF;
    -- Only live items can be changed below. The API already declared these
    -- before acquiring member locks; never lock deleted, undeclared history.
    PERFORM id FROM items WHERE household_id=NEW.household_id AND ics_source_id=NEW.id AND deleted_at IS NULL ORDER BY id FOR NO KEY UPDATE;
    IF NEW.deleted_at IS NOT NULL THEN
        UPDATE items SET deleted_at=NEW.deleted_at WHERE household_id=NEW.household_id AND ics_source_id=NEW.id AND deleted_at IS NULL;
        DELETE FROM ics_uid_map WHERE household_id=NEW.household_id AND source_id=NEW.id;
    ELSE
        desired_owner := NEW.owner_member_id;
        IF NEW.target_member_id IS NOT NULL AND EXISTS(SELECT 1 FROM household_members WHERE household_id=NEW.household_id AND id=NEW.target_member_id AND role<>'dependent') THEN
            desired_owner := NEW.target_member_id;
        END IF;
        UPDATE items SET visibility=NEW.visibility,owner_id=desired_owner,subject_member_id=NEW.target_member_id,source='ics:'||NEW.name
        WHERE household_id=NEW.household_id AND ics_source_id=NEW.id AND deleted_at IS NULL
          AND (visibility,owner_id,subject_member_id,source) IS DISTINCT FROM (NEW.visibility,desired_owner,NEW.target_member_id,'ics:'||NEW.name);
        -- Household visibility is not an assignment. R2 agenda membership
        -- follows the owner; later joins require no feed synchronization.
        IF NEW.target_member_id IS DISTINCT FROM OLD.target_member_id THEN
            DELETE FROM item_assignees a USING items i WHERE a.item_id=i.id AND i.household_id=NEW.household_id AND i.ics_source_id=NEW.id AND i.deleted_at IS NULL;
        END IF;
    END IF;
    RETURN NEW;
END $$;

-- CREATE OR REPLACE preserves the existing ACL. No new callable function/grants.

-- PostgreSQL's btree tuple limit is byte-based. Legacy larger UIDs remain readable;
-- NOT VALID avoids rejecting old databases, but all new/changed identities obey it.
ALTER TABLE ics_uid_map ADD CONSTRAINT ics_uid_map_uid_bytes CHECK (octet_length(uid) <= 1024) NOT VALID;

-- Legacy expanded feeds may have kept validators (and truncated away the
-- ExpandedRule warning). Their projection is time-dependent, so force refetch.
-- This operational UPDATE intentionally fires source bookkeeping: version bump,
-- revision row and outbox tickles. Failed/partial sources may re-emit a feed_error
-- if the old deduplication event has been purged; no item configuration is changed.
UPDATE ics_sources s SET etag=NULL,last_modified=NULL
WHERE kind='feed' AND (etag IS NOT NULL OR last_modified IS NOT NULL)
  AND EXISTS(SELECT 1 FROM ics_uid_map m WHERE m.source_id=s.id AND m.expanded_start IS NOT NULL);

ALTER TABLE ics_sources DROP CONSTRAINT ics_sources_last_status_check;
ALTER TABLE ics_sources ADD CONSTRAINT ics_sources_last_status_check
    CHECK (last_status IN ('queued','ok','partial','not_modified','failed','retrying'));

-- Internal operational state, not calendar identity or client schema: consecutive
-- apply-deadline failures (R80). Success/304, manual refresh and PUT reset it; contention never changes source state.
ALTER TABLE ics_sources ADD COLUMN apply_timeout_count integer NOT NULL DEFAULT 0
    CHECK (apply_timeout_count BETWEEN 0 AND 3);

RESET search_path;
RESET ROLE;
