SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

CREATE TABLE item_revisions (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL,
    item_id uuid NOT NULL,
    rev int NOT NULL CHECK (rev >= 1),
    snapshot jsonb NOT NULL,
    actor jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (item_id, rev),
    FOREIGN KEY (household_id, item_id) REFERENCES items (household_id, id)
);

CREATE TABLE task_events (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL,
    item_id uuid NOT NULL,
    occurrence_id uuid,
    kind text NOT NULL CHECK (kind IN ('created', 'updated', 'deleted', 'restored', 'assigned', 'unassigned',
                                       'status_changed', 'completed', 'reopened')),
    actor jsonb NOT NULL,
    data jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (household_id, item_id) REFERENCES items (household_id, id)
);
CREATE INDEX task_events_item ON task_events (item_id, created_at);
-- No FK on occurrence_id: the log is append-only and must never block derivation from deleting occurrences.
CREATE INDEX task_events_occurrence ON task_events (occurrence_id) WHERE occurrence_id IS NOT NULL;

CREATE FUNCTION pl_item_snapshot(p_item_id uuid) RETURNS jsonb
    LANGUAGE sql STABLE
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
    SELECT to_jsonb(i) || jsonb_build_object(
        'assignees', coalesce((SELECT jsonb_agg(a.member_id ORDER BY a.member_id) FROM item_assignees a WHERE a.item_id = i.id), '[]'),
        'viewers', coalesce((SELECT jsonb_agg(v.member_id ORDER BY v.member_id) FROM item_viewers v WHERE v.item_id = i.id), '[]'),
        'reminders', coalesce((SELECT jsonb_agg(r.offset_minutes ORDER BY r.offset_minutes) FROM item_reminders r WHERE r.item_id = i.id), '[]'))
    FROM items i WHERE i.id = p_item_id
$$;

CREATE FUNCTION pl_items_audit() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
DECLARE
    who jsonb := pl_current_actor();
BEGIN
    INSERT INTO item_revisions (household_id, item_id, rev, snapshot, actor)
    SELECT n.household_id, n.id,
           coalesce((SELECT max(r.rev) FROM item_revisions r WHERE r.item_id = n.id), 0) + 1,
           pl_item_snapshot(n.id), who
    FROM new_rows n;

    IF TG_OP = 'INSERT' THEN
        INSERT INTO task_events (household_id, item_id, kind, actor)
        SELECT household_id, id, 'created', who FROM new_rows;
    ELSE
        INSERT INTO task_events (household_id, item_id, kind, actor, data)
        SELECT c.household_id, c.id, c.kind, who,
               CASE WHEN c.kind = 'updated' THEN jsonb_build_object('changed', c.changed) ELSE '{}'::jsonb END
        FROM (
            SELECT n.household_id, n.id,
                   CASE WHEN o.deleted_at IS NULL AND n.deleted_at IS NOT NULL THEN 'deleted'
                        WHEN o.deleted_at IS NOT NULL AND n.deleted_at IS NULL THEN 'restored'
                        ELSE 'updated' END AS kind,
                   (SELECT coalesce(jsonb_agg(e.key ORDER BY e.key), '[]')
                    FROM jsonb_each(to_jsonb(n)) e
                    WHERE e.key NOT IN ('version', 'updated_at')
                      AND to_jsonb(o) -> e.key IS DISTINCT FROM e.value) AS changed
            FROM new_rows n JOIN old_rows o ON o.id = n.id
        ) c
        WHERE c.kind <> 'updated' OR c.changed <> '[]'::jsonb;
    END IF;
    RETURN NULL;
END $$;

CREATE TRIGGER items_audit_insert AFTER INSERT ON items
    REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_items_audit();
CREATE TRIGGER items_audit_update AFTER UPDATE ON items
    REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_items_audit();

-- Child rows bump their parent once per statement; the parent's audit trigger then snapshots the new children.
-- Lock order (see 0007): the t10_ prefix makes this the first AFTER trigger on each child table, and parents are
-- locked in id order before any item_derivation row is touched. NO KEY UPDATE is the lock the UPDATE below takes
-- anyway; FOR UPDATE would also conflict with the FOR KEY SHARE that foreign-key checks hold on the parent, so two
-- transactions that each inserted a child row (or an occurrence) of the same item would deadlock.
CREATE FUNCTION pl_item_children_changed() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
BEGIN
    PERFORM 1 FROM items WHERE id IN (SELECT DISTINCT item_id FROM changed_rows) ORDER BY id FOR NO KEY UPDATE;
    IF TG_TABLE_NAME = 'item_assignees' THEN
        INSERT INTO task_events (household_id, item_id, kind, actor, data)
        SELECT household_id, item_id, CASE TG_OP WHEN 'INSERT' THEN 'assigned' ELSE 'unassigned' END,
               pl_current_actor(), jsonb_build_object('member_id', member_id)
        FROM changed_rows;
    END IF;
    UPDATE items SET updated_at = now() WHERE id IN (SELECT DISTINCT item_id FROM changed_rows);
    RETURN NULL;
END $$;

CREATE TRIGGER t10_children_changed_insert AFTER INSERT ON item_assignees
    REFERENCING NEW TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_item_children_changed();
CREATE TRIGGER t10_children_changed_delete AFTER DELETE ON item_assignees
    REFERENCING OLD TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_item_children_changed();
CREATE TRIGGER t10_children_changed_insert AFTER INSERT ON item_viewers
    REFERENCING NEW TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_item_children_changed();
CREATE TRIGGER t10_children_changed_delete AFTER DELETE ON item_viewers
    REFERENCING OLD TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_item_children_changed();
CREATE TRIGGER t10_children_changed_insert AFTER INSERT ON item_reminders
    REFERENCING NEW TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_item_children_changed();
CREATE TRIGGER t10_children_changed_delete AFTER DELETE ON item_reminders
    REFERENCING OLD TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_item_children_changed();

CREATE FUNCTION pl_occurrence_status_event() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
BEGIN
    INSERT INTO task_events (household_id, item_id, occurrence_id, kind, actor, data)
    VALUES (NEW.household_id, NEW.item_id, NEW.id,
            CASE WHEN NEW.status = 'done' THEN 'completed'
                 WHEN OLD.status = 'done' AND NEW.status <> 'skipped' THEN 'reopened'
                 ELSE 'status_changed' END,
            pl_current_actor(), jsonb_build_object('from', OLD.status, 'to', NEW.status));
    RETURN NULL;
END $$;

CREATE TRIGGER occurrence_status_event AFTER UPDATE OF status ON item_occurrences
    FOR EACH ROW WHEN (OLD.status IS DISTINCT FROM NEW.status) EXECUTE FUNCTION pl_occurrence_status_event();

CREATE FUNCTION pl_append_only() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF current_user <> 'porchlight_maintenance' THEN
        RAISE EXCEPTION '% is append-only', TG_TABLE_NAME USING ERRCODE = 'PL403';
    END IF;
    RETURN NULL;
END $$;

CREATE TRIGGER append_only BEFORE UPDATE OR DELETE ON item_revisions
    FOR EACH STATEMENT EXECUTE FUNCTION pl_append_only();
CREATE TRIGGER append_only BEFORE UPDATE OR DELETE ON task_events
    FOR EACH STATEMENT EXECUTE FUNCTION pl_append_only();

-- Relationship rows change only by DELETE + INSERT, so the parent bump, audit, enqueue and eviction rules see both
-- the removed and the added member.
-- Statement-level BEFORE UPDATE triggers also fire for INSERT ... ON CONFLICT DO UPDATE, so callers must use
-- ON CONFLICT DO NOTHING on relationship tables.
CREATE FUNCTION pl_immutable_row() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    RAISE EXCEPTION '% rows are immutable; delete and insert instead', TG_TABLE_NAME USING ERRCODE = 'PL422';
END $$;

CREATE TRIGGER immutable_row BEFORE UPDATE ON item_assignees
    FOR EACH STATEMENT EXECUTE FUNCTION pl_immutable_row();
CREATE TRIGGER immutable_row BEFORE UPDATE ON item_viewers
    FOR EACH STATEMENT EXECUTE FUNCTION pl_immutable_row();
CREATE TRIGGER immutable_row BEFORE UPDATE ON item_reminders
    FOR EACH STATEMENT EXECUTE FUNCTION pl_immutable_row();

-- History that only the maintenance role (purges, from M3) may change.
CREATE FUNCTION pl_immutable_history() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF current_user <> 'porchlight_maintenance' THEN
        RAISE EXCEPTION '% rows are immutable', TG_TABLE_NAME USING ERRCODE = 'PL422';
    END IF;
    RETURN NULL;
END $$;

CREATE TRIGGER immutable_history BEFORE UPDATE OR DELETE ON device_certificates
    FOR EACH STATEMENT EXECUTE FUNCTION pl_immutable_history();

RESET search_path;
RESET ROLE;
