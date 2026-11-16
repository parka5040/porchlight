SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Match the actor lookup to the event -> unique source-link join. Colour, zone
-- and authority writes must not inspect historical notification sources at all.
CREATE INDEX notification_events_actor ON notification_events(household_id,actor_member_id,id)
    WHERE actor_member_id IS NOT NULL;

CREATE FUNCTION pl_sync_is_reset(p_household uuid) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
    SELECT EXISTS (SELECT 1 FROM sync_emissions s
        WHERE s.backend_pid=pg_backend_pid() AND s.transaction_id=pg_current_xact_id()
        AND s.household_id=p_household AND s.created_at=transaction_timestamp()
        AND s.emitted ? 'reset')
$$;

-- No blocking sort/full-closure aggregation before LIMIT. At most 201 rows are
-- yielded, or depth 200 reached; either boundary conservatively resets. Depth
-- also bounds malformed cyclic lineages without growing an unbounded path array.
CREATE FUNCTION pl_sync_descendants(p_household uuid,p_parent uuid)
RETURNS TABLE(id uuid,depth integer) LANGUAGE sql STABLE
SET search_path = pg_catalog, porchlight, pg_temp AS $$
    WITH RECURSIVE descendants AS (
        SELECT i.id,0 AS depth FROM porchlight.items i WHERE i.household_id=p_household AND i.id=p_parent
        UNION ALL
        SELECT i.id,d.depth+1 FROM descendants d CROSS JOIN LATERAL (
            SELECT child.id FROM porchlight.items child
            WHERE child.household_id=p_household AND child.split_from_item_id=d.id LIMIT 201
        ) i WHERE d.depth<200
    ) SELECT * FROM descendants LIMIT 201
$$;
CREATE FUNCTION pl_sync_lineage(p_household uuid,p_parent uuid,p_include_parent boolean)
RETURNS void LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE r record; visited integer := 0;
BEGIN
    IF pl_sync_is_reset(p_household) THEN RETURN; END IF;
    FOR r IN SELECT * FROM pl_sync_descendants(p_household,p_parent) LOOP
        visited := visited+1;
        IF visited=201 OR r.depth=200 THEN
            PERFORM pl_sync_emit(p_household,'sync.reset','household',p_household);
            RETURN;
        END IF;
        IF p_include_parent OR r.id<>p_parent THEN
            PERFORM pl_sync_emit(p_household,'item.invalidated','item',r.id);
            IF pl_sync_is_reset(p_household) THEN RETURN; END IF;
        END IF;
    END LOOP;
END $$;

DROP TRIGGER sync_actor_rename ON household_members;
CREATE OR REPLACE FUNCTION pl_sync_actor_rename() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE h uuid; r record;
BEGIN
    FOR h IN SELECT DISTINCT n.household_id FROM renamed_members n JOIN previous_members o USING(id)
        WHERE n.display_name IS DISTINCT FROM o.display_name
    LOOP
        IF pl_sync_is_reset(h) THEN CONTINUE; END IF;
        FOR r IN
            SELECT DISTINCT n.id,n.recipient_member_id FROM renamed_members m
            JOIN previous_members o ON o.id=m.id
            JOIN notification_events e ON e.household_id=h AND e.actor_member_id=m.id
            JOIN notification_sources s ON s.event_id=e.id
            JOIN notifications n ON n.id=s.notification_id AND n.household_id=h
            WHERE m.household_id=h AND m.display_name IS DISTINCT FROM o.display_name
                AND n.released_at IS NOT NULL LIMIT 201
        LOOP
            PERFORM pl_sync_emit(h,'notification.invalidated','notification',r.id,r.recipient_member_id);
            IF pl_sync_is_reset(h) THEN EXIT; END IF;
        END LOOP;
    END LOOP;
    RETURN NULL;
END $$;
CREATE TRIGGER sync_actor_rename AFTER UPDATE ON household_members
    REFERENCING OLD TABLE AS previous_members NEW TABLE AS renamed_members
    FOR EACH STATEMENT EXECUTE FUNCTION pl_sync_actor_rename();

DROP TRIGGER sync_items_update ON items;
CREATE TRIGGER sync_items_update AFTER UPDATE ON items
    REFERENCING OLD TABLE AS changed_items NEW TABLE AS current_items
    FOR EACH STATEMENT EXECUTE FUNCTION pl_sync_items();
CREATE OR REPLACE FUNCTION pl_sync_items() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE h uuid; r record;
BEGIN
    FOR h IN SELECT DISTINCT household_id FROM changed_items LOOP
        IF pl_sync_is_reset(h) THEN CONTINUE; END IF;
        FOR r IN
            SELECT DISTINCT b.item_id FROM changed_items i
            JOIN member_agenda a ON a.item_id=i.id
            JOIN member_agenda b ON b.household_id=a.household_id AND b.member_id=a.member_id AND b.busy && a.busy
            WHERE i.household_id=h LIMIT 201
        LOOP
            PERFORM pl_sync_emit(h,'item.invalidated','item',r.item_id);
            IF pl_sync_is_reset(h) THEN EXIT; END IF;
        END LOOP;
        IF pl_sync_is_reset(h) THEN CONTINUE; END IF;
        IF TG_OP='UPDATE' THEN
            FOR r IN SELECT o.id FROM changed_items o JOIN current_items n USING(id)
                WHERE o.household_id=h AND (
                    (o.owner_id,o.subject_member_id,o.visibility,o.deleted_at,o.series_id,o.split_from_item_id,o.split_from_seq,o.ics_source_id)
                    IS DISTINCT FROM
                    (n.owner_id,n.subject_member_id,n.visibility,n.deleted_at,n.series_id,n.split_from_item_id,n.split_from_seq,n.ics_source_id)
                    -- Relationship triggers bump only the parent's bookkeeping.
                    OR to_jsonb(o)-'version'-'updated_at'=to_jsonb(n)-'version'-'updated_at')
            LOOP
                PERFORM pl_sync_lineage(h,r.id,false);
                IF pl_sync_is_reset(h) THEN EXIT; END IF;
            END LOOP;
        ELSE
            -- A physical parent DELETE cannot leave FK-linked descendants; its
            -- preceding lineage sever/soft-delete UPDATE supplies their closure.
            NULL;
        END IF;
        IF pl_sync_is_reset(h) THEN CONTINUE; END IF;
        FOR r IN
            SELECT DISTINCT n.id,n.recipient_member_id FROM notifications n
            WHERE n.household_id=h AND n.released_at IS NOT NULL AND (
                n.item_id IN (SELECT id FROM changed_items WHERE household_id=h)
                OR n.id IN (SELECT s.notification_id FROM notification_sources s
                    JOIN notification_events e ON e.id=s.event_id
                    WHERE e.item_id IN (SELECT id FROM changed_items WHERE household_id=h))) LIMIT 201
        LOOP
            PERFORM pl_sync_emit(h,'notification.invalidated','notification',r.id,r.recipient_member_id);
            IF pl_sync_is_reset(h) THEN EXIT; END IF;
        END LOOP;
    END LOOP;
    RETURN NULL;
END $$;

CREATE OR REPLACE FUNCTION pl_sync_agenda() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE h uuid; r record;
BEGIN
    FOR h IN SELECT DISTINCT household_id FROM changed_agenda LOOP
        IF pl_sync_is_reset(h) THEN CONTINUE; END IF;
        FOR r IN SELECT item_id FROM (
            SELECT item_id FROM changed_agenda WHERE household_id=h
            UNION
            SELECT b.item_id FROM changed_agenda a JOIN member_agenda b
                ON b.household_id=a.household_id AND b.member_id=a.member_id AND b.busy && a.busy
                WHERE a.household_id=h
        ) candidates LIMIT 201 LOOP
            PERFORM pl_sync_emit(h,'item.invalidated','item',r.item_id);
            IF pl_sync_is_reset(h) THEN EXIT; END IF;
        END LOOP;
    END LOOP;
    RETURN NULL;
END $$;
CREATE OR REPLACE FUNCTION pl_sync_load() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE h uuid; r record;
BEGIN
    FOR h IN SELECT DISTINCT m.household_id FROM changed_load l
        JOIN household_members m ON m.id=l.member_id LOOP
        IF pl_sync_is_reset(h) THEN CONTINUE; END IF;
        FOR r IN SELECT DISTINCT a.item_id FROM changed_load l
            JOIN member_agenda a ON a.member_id=l.member_id
            JOIN household_members m ON m.id=a.member_id JOIN households hh ON hh.id=m.household_id
            WHERE a.household_id=h AND m.household_id=h AND
                l.date=coalesce((a.starts_at AT TIME ZONE pl_effective_member_zone(m.role,m.tzid,hh.tzid))::date,
                    (a.due_at AT TIME ZONE pl_effective_member_zone(m.role,m.tzid,hh.tzid))::date,a.due_date)
            LIMIT 201
        LOOP
            PERFORM pl_sync_emit(h,'item.invalidated','item',r.item_id);
            IF pl_sync_is_reset(h) THEN EXIT; END IF;
        END LOOP;
    END LOOP;
    RETURN NULL;
END $$;
CREATE OR REPLACE FUNCTION pl_sync_notification_delete() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE h uuid; r record;
BEGIN
    IF TG_TABLE_NAME='notifications' THEN
        FOR h IN SELECT DISTINCT household_id FROM deleted_notifications LOOP
            IF pl_sync_is_reset(h) THEN CONTINUE; END IF;
            FOR r IN SELECT id,recipient_member_id FROM deleted_notifications
                WHERE household_id=h AND released_at IS NOT NULL LIMIT 201
            LOOP
                PERFORM pl_sync_emit(h,'notification.invalidated','notification',r.id,r.recipient_member_id);
                IF pl_sync_is_reset(h) THEN EXIT; END IF;
            END LOOP;
        END LOOP;
    ELSE
        -- Source rows have no household key; resolve the still-existing parent.
        FOR h IN SELECT DISTINCT n.household_id FROM deleted_sources s
            JOIN notifications n ON n.id=s.notification_id
        LOOP
            IF pl_sync_is_reset(h) THEN CONTINUE; END IF;
            FOR r IN SELECT DISTINCT n.id,n.recipient_member_id FROM deleted_sources s
                JOIN notifications n ON n.id=s.notification_id
                WHERE n.household_id=h AND n.released_at IS NOT NULL LIMIT 201
            LOOP
                PERFORM pl_sync_emit(h,'notification.invalidated','notification',r.id,r.recipient_member_id);
                IF pl_sync_is_reset(h) THEN EXIT; END IF;
            END LOOP;
        END LOOP;
    END IF;
    RETURN NULL;
END $$;
CREATE OR REPLACE FUNCTION pl_sync_list() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE parent uuid; h uuid;
BEGIN
    IF TG_OP='DELETE' THEN h:=OLD.household_id; ELSE h:=NEW.household_id; END IF;
    IF pl_sync_is_reset(h) THEN RETURN NULL; END IF;
    IF TG_TABLE_NAME='lists' THEN
        IF TG_OP='DELETE' THEN parent:=OLD.item_id; ELSE parent:=NEW.item_id; END IF;
    ELSE
        SELECT item_id INTO parent FROM lists WHERE id=CASE WHEN TG_OP='DELETE' THEN OLD.list_id ELSE NEW.list_id END;
    END IF;
    IF parent IS NOT NULL THEN PERFORM pl_sync_lineage(h,parent,true); END IF;
    RETURN NULL;
END $$;
CREATE OR REPLACE FUNCTION pl_sync_suggestion() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE r record;
BEGIN
    IF TG_OP='DELETE' THEN r:=OLD; ELSE r:=NEW; END IF;
    IF NOT pl_sync_is_reset(r.household_id) THEN
        PERFORM pl_sync_emit(r.household_id,'item.invalidated','item',r.item_id);
    END IF;
    RETURN NULL;
END $$;
CREATE OR REPLACE FUNCTION pl_sync_pending() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
    IF NOT pl_sync_is_reset(NEW.household_id) THEN
        PERFORM pl_sync_emit(NEW.household_id,'item.invalidated','item',NEW.item_id);
    END IF;
    RETURN NULL;
END $$;

REVOKE ALL ON FUNCTION pl_sync_is_reset(uuid),pl_sync_descendants(uuid,uuid),pl_sync_lineage(uuid,uuid,boolean) FROM PUBLIC;
RESET search_path;
RESET ROLE;
