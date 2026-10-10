SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

ALTER TABLE ics_sources ADD CONSTRAINT ics_sources_badge_name_check CHECK(length(name) <= 196);
ALTER TABLE ics_sources ADD COLUMN visibility text CHECK (visibility IN ('private','household'));
UPDATE ics_sources SET visibility=CASE WHEN target_member_id IS NULL OR EXISTS(SELECT 1 FROM household_members m WHERE m.household_id=ics_sources.household_id AND m.id=ics_sources.target_member_id AND m.role='dependent') THEN 'household' ELSE 'private' END;
CREATE FUNCTION pl_ics_visibility_default() RETURNS trigger LANGUAGE plpgsql SET search_path=pg_catalog,porchlight,pg_temp AS $$
BEGIN
    NEW.visibility := coalesce(NEW.visibility, CASE WHEN NEW.target_member_id IS NULL OR EXISTS(SELECT 1 FROM household_members m WHERE m.household_id=NEW.household_id AND m.id=NEW.target_member_id AND m.role='dependent') THEN 'household' ELSE 'private' END);
    RETURN NEW;
END $$;
CREATE TRIGGER a0_visibility_default BEFORE INSERT ON ics_sources FOR EACH ROW EXECUTE FUNCTION pl_ics_visibility_default();
ALTER TABLE ics_sources ALTER COLUMN visibility SET NOT NULL;
ALTER TABLE ics_uid_map ADD COLUMN content_hash text;
CREATE INDEX items_ics_source ON items(ics_source_id) WHERE ics_source_id IS NOT NULL;
ALTER TABLE ics_source_revisions DROP CONSTRAINT ics_source_revisions_source_id_fkey;
ALTER TABLE ics_source_revisions ADD CONSTRAINT ics_source_revision_scope_fk FOREIGN KEY(household_id,source_id) REFERENCES ics_sources(household_id,id);
GRANT INSERT(visibility),UPDATE(visibility) ON ics_sources TO porchlight_agent;

-- Raw source configuration shares the API's authoritative effects. It enqueues
-- normal item derivation (eventual for raw SQL); API WriteScope has already
-- applied/derived these changes so the distinct guards below write nothing.
CREATE FUNCTION pl_ics_config_items() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,porchlight,pg_temp AS $$
DECLARE desired_owner uuid;
BEGIN
    IF NEW.kind <> 'feed' OR (NEW.visibility,NEW.name,NEW.target_member_id,NEW.deleted_at) IS NOT DISTINCT FROM (OLD.visibility,OLD.name,OLD.target_member_id,OLD.deleted_at) THEN RETURN NEW; END IF;
    -- Source -> sorted item locks, never generated-member locks -> items.
    PERFORM id FROM items WHERE household_id=NEW.household_id AND ics_source_id=NEW.id ORDER BY id FOR NO KEY UPDATE;
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
CREATE TRIGGER ics_config_items AFTER UPDATE OF visibility,name,target_member_id,deleted_at ON ics_sources FOR EACH ROW EXECUTE FUNCTION pl_ics_config_items();

ALTER TABLE ics_sources DROP CONSTRAINT ics_sources_last_status_check;
ALTER TABLE ics_sources ADD CONSTRAINT ics_sources_last_status_check CHECK(last_status IN ('ok','partial','not_modified','failed','queued'));
ALTER TABLE ics_sources ADD COLUMN failure_count integer NOT NULL DEFAULT 0 CHECK(failure_count >= 0);
ALTER TABLE ics_sources ADD COLUMN last_warnings jsonb NOT NULL DEFAULT '[]'::jsonb CHECK(jsonb_typeof(last_warnings)='array');

-- R53: extend the registry without replacing the shared producer ABI.
INSERT INTO porchlight.notification_kinds(kind,self_suppressed,description) VALUES
 ('calendar_changed',true,'Imported or subscribed calendar appointment changed'),
 ('feed_error',false,'Subscribed calendar fetch failed or was partial')
ON CONFLICT (kind) DO NOTHING;

CREATE OR REPLACE FUNCTION pl_ics_source_changed() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog,porchlight,pg_temp AS $$
BEGIN
    INSERT INTO ics_source_revisions(household_id,source_id,source_version,snapshot,actor)
    VALUES(NEW.household_id,NEW.id,NEW.version,to_jsonb(NEW)-'last_warnings',pl_current_actor());
    IF NEW.kind='feed' THEN
        INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
        SELECT NEW.household_id,'ics_source.changed','ics_source',NEW.id,jsonb_build_object('source_id',NEW.id,'member_id',m.id)
        FROM household_members m WHERE m.household_id=NEW.household_id AND m.role='super_admin' AND m.deleted_at IS NULL;
        IF NEW.deleted_at IS NULL AND NEW.last_status IN ('failed','partial') AND NEW.last_fetch_at IS NOT NULL THEN
            PERFORM pl_emit_notification_event(NEW.household_id,m.id,'feed_error','ics_source',NEW.id,
                jsonb_build_object('source_id',NEW.id,'status',NEW.last_status),NULL,NULL,
                'feed_error:'||NEW.id::text||':'||m.id::text||':'||(NEW.last_fetch_at AT TIME ZONE 'UTC')::date::text)
            FROM household_members m WHERE m.household_id=NEW.household_id AND m.role='super_admin' AND m.deleted_at IS NULL;
        END IF;
    END IF;
    RETURN NEW;
END $$;

RESET search_path;
RESET ROLE;
