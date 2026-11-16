SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Extra tickles are negotiated at delivery. Keep at most 200 distinct extra
-- entity invalidations per transaction; beyond that send one household reset.
-- Transaction-local state rolls back with savepoints and never locks domain rows.
CREATE FUNCTION pl_sync_emit(p_household uuid, p_kind text, p_type text, p_id uuid, p_member uuid DEFAULT NULL)
RETURNS void LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE
    emitted jsonb := coalesce(nullif(current_setting('porchlight.sync_emitted',true),'')::jsonb,'{}'::jsonb);
    key text := p_household::text||':'||p_kind||':'||p_id::text||':'||coalesce(p_member::text,'');
    reset_key text := p_household::text||':reset';
BEGIN
    IF emitted ? key OR emitted ? reset_key THEN RETURN; END IF;
    IF p_kind='sync.reset' OR (SELECT count(*) FROM jsonb_object_keys(emitted)) >= 200 THEN
        INSERT INTO outbox(household_id,kind,entity_type,entity_id)
            VALUES(p_household,'sync.reset','household',p_household);
        emitted := emitted || jsonb_build_object(reset_key,true);
    ELSE
        INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
            VALUES(p_household,p_kind,p_type,p_id,jsonb_strip_nulls(jsonb_build_object('member_id',p_member)));
        emitted := emitted || jsonb_build_object(key,true);
    END IF;
    PERFORM set_config('porchlight.sync_emitted',emitted::text,true);
    PERFORM pg_notify('porchlight_outbox','');
END $$;

-- Both removed and added busy spans matter. Transition tables keep this one
-- bounded candidate selection per statement, not one query per occurrence.
CREATE FUNCTION pl_sync_agenda() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE r record;
BEGIN
    FOR r IN
        SELECT household_id,item_id FROM (
            SELECT household_id,item_id FROM changed_agenda
            UNION
            SELECT b.household_id,b.item_id FROM changed_agenda a JOIN member_agenda b
                ON b.household_id=a.household_id AND b.member_id=a.member_id AND b.busy && a.busy
        ) candidates ORDER BY household_id,item_id LIMIT 201
    LOOP
        PERFORM pl_sync_emit(r.household_id,'item.invalidated','item',r.item_id);
    END LOOP;
    RETURN NULL;
END $$;
CREATE TRIGGER sync_agenda_insert AFTER INSERT ON member_agenda
    REFERENCING NEW TABLE AS changed_agenda FOR EACH STATEMENT EXECUTE FUNCTION pl_sync_agenda();
CREATE TRIGGER sync_agenda_delete AFTER DELETE ON member_agenda
    REFERENCING OLD TABLE AS changed_agenda FOR EACH STATEMENT EXECUTE FUNCTION pl_sync_agenda();
CREATE TRIGGER sync_agenda_update_old AFTER UPDATE ON member_agenda
    REFERENCING OLD TABLE AS changed_agenda FOR EACH STATEMENT EXECUTE FUNCTION pl_sync_agenda();
CREATE TRIGGER sync_agenda_update_new AFTER UPDATE ON member_agenda
    REFERENCING NEW TABLE AS changed_agenda FOR EACH STATEMENT EXECUTE FUNCTION pl_sync_agenda();

-- Metadata/ACL-only changes never derive, but change masked conflict titles and
-- the readable source count of existing coalesced notifications.
CREATE FUNCTION pl_sync_items() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE r record;
BEGIN
    FOR r IN
        SELECT DISTINCT b.household_id,b.item_id FROM changed_items i
        JOIN member_agenda a ON a.item_id=i.id
        JOIN member_agenda b ON b.household_id=a.household_id AND b.member_id=a.member_id AND b.busy && a.busy
        ORDER BY b.household_id,b.item_id LIMIT 201
    LOOP
        PERFORM pl_sync_emit(r.household_id,'item.invalidated','item',r.item_id);
    END LOOP;
    FOR r IN
        SELECT DISTINCT n.household_id,n.id,n.recipient_member_id FROM notifications n
        WHERE n.released_at IS NOT NULL AND (
            n.item_id IN (SELECT id FROM changed_items)
            OR n.id IN (SELECT s.notification_id FROM notification_sources s
                JOIN notification_events e ON e.id=s.event_id WHERE e.item_id IN (SELECT id FROM changed_items)))
        ORDER BY n.household_id,n.id LIMIT 201
    LOOP
        PERFORM pl_sync_emit(r.household_id,'notification.invalidated','notification',r.id,r.recipient_member_id);
    END LOOP;
    RETURN NULL;
END $$;
CREATE TRIGGER sync_items_update AFTER UPDATE ON items
    REFERENCING OLD TABLE AS changed_items FOR EACH STATEMENT EXECUTE FUNCTION pl_sync_items();
CREATE TRIGGER sync_items_delete AFTER DELETE ON items
    REFERENCING OLD TABLE AS changed_items FOR EACH STATEMENT EXECUTE FUNCTION pl_sync_items();

-- Includes member-only load-worker completion and disappearing overloads, not
-- just item derivation. No item locks while the worker holds its member lock.
CREATE FUNCTION pl_sync_load() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE r record;
BEGIN
    FOR r IN
        SELECT DISTINCT a.household_id,a.item_id FROM changed_load l
        JOIN member_agenda a ON a.member_id=l.member_id
        JOIN household_members m ON m.id=a.member_id JOIN households h ON h.id=m.household_id
        WHERE l.date=coalesce((a.starts_at AT TIME ZONE pl_effective_member_zone(m.role,m.tzid,h.tzid))::date,
            (a.due_at AT TIME ZONE pl_effective_member_zone(m.role,m.tzid,h.tzid))::date,a.due_date)
        ORDER BY a.household_id,a.item_id LIMIT 201
    LOOP
        PERFORM pl_sync_emit(r.household_id,'item.invalidated','item',r.item_id);
    END LOOP;
    RETURN NULL;
END $$;
CREATE TRIGGER sync_load_insert AFTER INSERT ON member_daily_load
    REFERENCING NEW TABLE AS changed_load FOR EACH STATEMENT EXECUTE FUNCTION pl_sync_load();
CREATE TRIGGER sync_load_delete AFTER DELETE ON member_daily_load
    REFERENCING OLD TABLE AS changed_load FOR EACH STATEMENT EXECUTE FUNCTION pl_sync_load();
CREATE TRIGGER sync_load_update_old AFTER UPDATE ON member_daily_load
    REFERENCING OLD TABLE AS changed_load FOR EACH STATEMENT EXECUTE FUNCTION pl_sync_load();
CREATE TRIGGER sync_load_update_new AFTER UPDATE ON member_daily_load
    REFERENCING NEW TABLE AS changed_load FOR EACH STATEMENT EXECUTE FUNCTION pl_sync_load();

CREATE FUNCTION pl_sync_notification_delete() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE r record;
BEGIN
    IF TG_TABLE_NAME='notifications' THEN
        FOR r IN SELECT household_id,id,recipient_member_id FROM deleted_notifications
            WHERE released_at IS NOT NULL ORDER BY household_id,id LIMIT 201
        LOOP
            PERFORM pl_sync_emit(r.household_id,'notification.invalidated','notification',r.id,r.recipient_member_id);
        END LOOP;
    ELSE
        FOR r IN SELECT DISTINCT n.household_id,n.id,n.recipient_member_id FROM deleted_sources s
            JOIN notifications n ON n.id=s.notification_id WHERE n.released_at IS NOT NULL
            ORDER BY n.household_id,n.id LIMIT 201
        LOOP
            PERFORM pl_sync_emit(r.household_id,'notification.invalidated','notification',r.id,r.recipient_member_id);
        END LOOP;
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER sync_notifications_delete AFTER DELETE ON notifications
    REFERENCING OLD TABLE AS deleted_notifications FOR EACH STATEMENT EXECUTE FUNCTION pl_sync_notification_delete();
CREATE TRIGGER sync_sources_delete AFTER DELETE ON notification_sources
    REFERENCING OLD TABLE AS deleted_sources FOR EACH STATEMENT EXECUTE FUNCTION pl_sync_notification_delete();

CREATE FUNCTION pl_sync_suggestion() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE r record;
BEGIN
    IF TG_OP='DELETE' THEN r:=OLD; ELSE r:=NEW; END IF;
    PERFORM pl_sync_emit(r.household_id,'item.invalidated','item',r.item_id);
    RETURN NULL;
END $$;
CREATE TRIGGER sync_suggestion AFTER INSERT OR UPDATE OR DELETE ON schedule_suggestions
    FOR EACH ROW EXECUTE FUNCTION pl_sync_suggestion();

-- A per-occurrence template on a split ancestor contributes to tail bundles.
CREATE FUNCTION pl_sync_list() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE parent uuid; r record;
BEGIN
    IF TG_TABLE_NAME='lists' THEN
        IF TG_OP='DELETE' THEN parent:=OLD.item_id; ELSE parent:=NEW.item_id; END IF;
    ELSE
        SELECT item_id INTO parent FROM lists WHERE id=CASE WHEN TG_OP='DELETE' THEN OLD.list_id ELSE NEW.list_id END;
    END IF;
    IF parent IS NULL THEN RETURN NULL; END IF;
    FOR r IN
        WITH RECURSIVE descendants AS (
            SELECT id,household_id FROM items WHERE id=parent
            UNION ALL
            SELECT i.id,i.household_id FROM items i JOIN descendants d ON i.split_from_item_id=d.id
        ) CYCLE id SET cycle USING path
        SELECT household_id,id FROM descendants WHERE NOT cycle ORDER BY household_id,id LIMIT 201
    LOOP
        PERFORM pl_sync_emit(r.household_id,'item.invalidated','item',r.id);
    END LOOP;
    RETURN NULL;
END $$;
CREATE TRIGGER sync_list AFTER INSERT OR UPDATE OR DELETE ON lists FOR EACH ROW EXECUTE FUNCTION pl_sync_list();
CREATE TRIGGER sync_entry AFTER INSERT OR UPDATE OR DELETE ON list_entries FOR EACH ROW EXECUTE FUNCTION pl_sync_list();

CREATE FUNCTION pl_sync_reset() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
    IF TG_TABLE_NAME='households' THEN
        PERFORM pl_sync_emit(NEW.id,'sync.reset','household',NEW.id);
    ELSE
        PERFORM pl_sync_emit(NEW.household_id,'sync.reset','household',NEW.household_id);
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER sync_household AFTER UPDATE ON households FOR EACH ROW
    WHEN (OLD IS DISTINCT FROM NEW) EXECUTE FUNCTION pl_sync_reset();
CREATE TRIGGER sync_member_role AFTER UPDATE OF role,deleted_at ON household_members FOR EACH ROW
    WHEN ((OLD.role,OLD.deleted_at) IS DISTINCT FROM (NEW.role,NEW.deleted_at)) EXECUTE FUNCTION pl_sync_reset();

-- 0048 funnels every availability replacement through the profile trigger.
CREATE OR REPLACE FUNCTION pl_availability_profile_changed() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
    INSERT INTO planning_audit(household_id,entity_type,entity_id,operation,snapshot,actor)
        VALUES(NEW.household_id,'availability',NEW.member_id,TG_OP,to_jsonb(NEW),pl_current_actor());
    INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
        VALUES(NEW.household_id,'settings.changed','availability',NEW.member_id,jsonb_build_object('member_id',NEW.member_id));
    PERFORM pg_notify('porchlight_outbox',''); RETURN NULL;
END $$;

REVOKE ALL ON FUNCTION pl_sync_emit(uuid,text,text,uuid,uuid),pl_sync_agenda(),pl_sync_items(),pl_sync_load(),
    pl_sync_notification_delete(),pl_sync_suggestion(),pl_sync_list(),pl_sync_reset() FROM PUBLIC;
RESET search_path;
RESET ROLE;
