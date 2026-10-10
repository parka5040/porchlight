SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
-- A revision stores its parent's version; it is append-only, not itself a versioned resource.
ALTER TABLE ics_source_revisions RENAME COLUMN version TO source_version;
CREATE OR REPLACE FUNCTION pl_ics_source_changed() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog, porchlight, pg_temp AS $$
BEGIN
    INSERT INTO ics_source_revisions(household_id,source_id,source_version,snapshot,actor)
    VALUES(NEW.household_id,NEW.id,NEW.version,to_jsonb(NEW)-'last_warnings',pl_current_actor());
    IF NEW.kind='feed' THEN
        INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
        SELECT NEW.household_id,'ics_source.changed','ics_source',NEW.id,jsonb_build_object('source_id',NEW.id,'member_id',m.id)
        FROM household_members m WHERE m.household_id=NEW.household_id AND m.role='super_admin' AND m.deleted_at IS NULL;
    END IF;
    RETURN NEW;
END $$;
RESET search_path;
RESET ROLE;
