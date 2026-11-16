SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Private bookkeeping, never caller settings. Rows belong to one backend/xact;
-- the next transaction on that backend retires its predecessor. Bounded lazy
-- retention also retires rows left by disconnected backends after one day.
CREATE TABLE sync_emissions (
    backend_pid integer NOT NULL,
    transaction_id xid8 NOT NULL,
    household_id uuid NOT NULL,
    emitted jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT transaction_timestamp(),
    PRIMARY KEY (backend_pid, transaction_id, household_id)
);
CREATE INDEX sync_emissions_retention ON sync_emissions(created_at);
REVOKE ALL ON sync_emissions FROM PUBLIC, porchlight_app, porchlight_agent, porchlight_maintenance;

CREATE OR REPLACE FUNCTION pl_sync_emit(p_household uuid, p_kind text, p_type text, p_id uuid, p_member uuid DEFAULT NULL)
RETURNS void LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE
    emitted jsonb;
    current_xact xid8 := pg_current_xact_id();
    backend integer := pg_backend_pid();
    key text := p_kind||':'||p_id::text||':'||coalesce(p_member::text,'');
BEGIN
    SELECT s.emitted INTO emitted FROM sync_emissions s
        WHERE s.backend_pid=backend AND s.transaction_id=current_xact AND s.household_id=p_household
        -- A restored bookkeeping row is never authority for a new transaction.
        AND s.created_at=transaction_timestamp();
    IF emitted IS NULL THEN
        DELETE FROM sync_emissions WHERE (backend_pid,transaction_id,household_id) IN (
            SELECT s.backend_pid,s.transaction_id,s.household_id FROM sync_emissions s
            WHERE (s.backend_pid=backend AND s.transaction_id<>current_xact)
                OR s.created_at < statement_timestamp()-interval '1 day'
            ORDER BY s.created_at LIMIT 200 FOR UPDATE SKIP LOCKED);
        emitted := '{}'::jsonb;
    END IF;
    IF emitted ? key OR emitted ? 'reset' THEN RETURN; END IF;
    IF p_kind='sync.reset' OR (SELECT count(*) FROM jsonb_object_keys(emitted)) >= 200 THEN
        INSERT INTO outbox(household_id,kind,entity_type,entity_id)
            VALUES(p_household,'sync.reset','household',p_household);
        emitted := emitted || jsonb_build_object('reset',true);
    ELSE
        INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
            VALUES(p_household,p_kind,p_type,p_id,jsonb_strip_nulls(jsonb_build_object('member_id',p_member)));
        emitted := emitted || jsonb_build_object(key,true);
    END IF;
    INSERT INTO sync_emissions AS s(backend_pid,transaction_id,household_id,emitted)
        VALUES(backend,current_xact,p_household,emitted)
        ON CONFLICT (backend_pid,transaction_id,household_id) DO UPDATE
            SET emitted=EXCLUDED.emitted,created_at=EXCLUDED.created_at;
    PERFORM pg_notify('porchlight_outbox','');
END $$;

-- Ancestor ACL, deletion, relationships and lineage affect inherited templates
-- even when descendant agendas do not overlap the changed ancestor at all.
CREATE OR REPLACE FUNCTION pl_sync_items() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
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
        WITH RECURSIVE descendants AS (
            SELECT id,household_id FROM changed_items
            UNION
            SELECT i.id,i.household_id FROM items i JOIN descendants d
                ON i.split_from_item_id=d.id AND i.household_id=d.household_id
        )
        SELECT household_id,id FROM descendants WHERE id NOT IN (SELECT id FROM changed_items)
        ORDER BY household_id,id LIMIT 201
    LOOP
        PERFORM pl_sync_emit(r.household_id,'item.invalidated','item',r.id);
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

CREATE FUNCTION pl_sync_actor_rename() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE r record;
BEGIN
    FOR r IN
        SELECT DISTINCT n.household_id,n.id,n.recipient_member_id FROM notifications n
        JOIN notification_sources s ON s.notification_id=n.id
        JOIN notification_events e ON e.id=s.event_id
        JOIN renamed_members m ON m.id=e.actor_member_id AND m.household_id=n.household_id
        WHERE n.released_at IS NOT NULL
        ORDER BY n.household_id,n.id LIMIT 201
    LOOP
        PERFORM pl_sync_emit(r.household_id,'notification.invalidated','notification',r.id,r.recipient_member_id);
    END LOOP;
    RETURN NULL;
END $$;
CREATE TRIGGER sync_actor_rename AFTER UPDATE ON household_members
    REFERENCING OLD TABLE AS renamed_members FOR EACH STATEMENT EXECUTE FUNCTION pl_sync_actor_rename();

-- Sweeps and zone/offset enqueues commit before the worker. Pending is already
-- observable then; completion continues to use the ordinary derived event.
CREATE FUNCTION pl_sync_pending() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
    PERFORM pl_sync_emit(NEW.household_id,'item.invalidated','item',NEW.item_id);
    RETURN NULL;
END $$;
CREATE TRIGGER sync_pending AFTER UPDATE ON item_derivation FOR EACH ROW
    WHEN (OLD.requested_gen<=OLD.applied_gen AND NEW.requested_gen>NEW.applied_gen)
    EXECUTE FUNCTION pl_sync_pending();

REVOKE ALL ON FUNCTION pl_sync_actor_rename(),pl_sync_pending() FROM PUBLIC;
RESET search_path;
RESET ROLE;
