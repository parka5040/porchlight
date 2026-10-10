SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Lock order: items -> item_occurrences -> item_derivation. Nothing holds an item_derivation row lock and then
-- locks an item or an occurrence: on the child tables the parent bump (t10_*) fires before the enqueue (t20_*) and
-- access (t30_*) triggers (same-timing triggers fire in name order), and every trigger that touches several rows
-- locks them in item id order.

CREATE SEQUENCE outbox_stream_seq AS bigint;

CREATE TABLE outbox (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL REFERENCES households (id),
    kind text NOT NULL,
    entity_type text NOT NULL,
    entity_id uuid NOT NULL,
    payload jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now(),
    stream_seq bigint UNIQUE
);
CREATE INDEX outbox_unprojected ON outbox (id) WHERE stream_seq IS NULL;

-- A projected event never moves in (or leaves) the stream: stream_seq is set once.
CREATE FUNCTION pl_stream_seq_set_once() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    RAISE EXCEPTION 'stream_seq cannot change once assigned'
        USING ERRCODE = 'PL422', TABLE = TG_TABLE_NAME, COLUMN = 'stream_seq';
END $$;

CREATE TRIGGER stream_seq_set_once BEFORE UPDATE ON outbox
    FOR EACH ROW WHEN (OLD.stream_seq IS NOT NULL AND NEW.stream_seq IS DISTINCT FROM OLD.stream_seq)
    EXECUTE FUNCTION pl_stream_seq_set_once();

-- Serializes stream assignment (porchlight_db::outbox::assign_stream_seqs locks the single row FOR UPDATE), so
-- commit order equals stream order. A row lock, not LOCK TABLE: any role with USAGE on the schema can hold
-- RowExclusiveLock on a table it has no grant on (PREPARE takes parse-time locks before privilege checks), which
-- would block LOCK TABLE ... IN EXCLUSIVE MODE; a row lock needs UPDATE, which only the app has. Agents can still
-- queue conflicting lock requests that stall assignment; that is accepted (agents defend integrity, not
-- availability) and bounded by the app's 5 s lock_timeout.
CREATE TABLE stream_assign_mutex (
    id boolean PRIMARY KEY DEFAULT true CHECK (id)
);
INSERT INTO stream_assign_mutex DEFAULT VALUES;

-- items.changed: one row per item per statement.
CREATE FUNCTION pl_items_outbox() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
BEGIN
    INSERT INTO outbox (household_id, kind, entity_type, entity_id)
    SELECT household_id, 'item.changed', 'item', id FROM new_rows;
    PERFORM pg_notify('porchlight_outbox', '');
    RETURN NULL;
END $$;

CREATE TRIGGER items_outbox_insert AFTER INSERT ON items
    REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_items_outbox();
CREATE TRIGGER items_outbox_update AFTER UPDATE ON items
    REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_items_outbox();

-- Generic row-level outbox writer: TG_ARGV = (kind, entity_type).
CREATE FUNCTION pl_row_outbox() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
BEGIN
    INSERT INTO outbox (household_id, kind, entity_type, entity_id)
    VALUES (NEW.household_id, TG_ARGV[0], TG_ARGV[1], NEW.id);
    PERFORM pg_notify('porchlight_outbox', '');
    RETURN NULL;
END $$;

CREATE TRIGGER occurrence_outbox AFTER UPDATE ON item_occurrences
    FOR EACH ROW WHEN (
        (OLD.status, OLD.board_position, OLD.completed_by, OLD.completed_at,
         OLD.override_local_start, OLD.override_local_end, OLD.override_local_due, OLD.override_due_date)
        IS DISTINCT FROM
        (NEW.status, NEW.board_position, NEW.completed_by, NEW.completed_at,
         NEW.override_local_start, NEW.override_local_end, NEW.override_local_due, NEW.override_due_date))
    EXECUTE FUNCTION pl_row_outbox('occurrence.changed', 'occurrence');

CREATE TRIGGER member_outbox AFTER UPDATE OF tzid ON household_members
    FOR EACH ROW WHEN (OLD.tzid IS DISTINCT FROM NEW.tzid)
    EXECUTE FUNCTION pl_row_outbox('member.changed', 'member');

-- Access can shrink: tell devices to re-check and evict. Delivery (M5) re-checks visibility per recipient.
CREATE FUNCTION pl_item_access_changed() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
BEGIN
    INSERT INTO outbox (household_id, kind, entity_type, entity_id, payload)
    VALUES (NEW.household_id, 'item.access_changed', 'item', NEW.id,
            jsonb_strip_nulls(jsonb_build_object(
                'previous_owner_id', CASE WHEN OLD.owner_id IS DISTINCT FROM NEW.owner_id THEN OLD.owner_id END,
                'previous_visibility', CASE WHEN OLD.visibility IS DISTINCT FROM NEW.visibility THEN OLD.visibility END)));
    PERFORM pg_notify('porchlight_outbox', '');
    RETURN NULL;
END $$;

CREATE TRIGGER items_access_changed AFTER UPDATE ON items
    FOR EACH ROW WHEN (OLD.owner_id IS DISTINCT FROM NEW.owner_id OR OLD.visibility IS DISTINCT FROM NEW.visibility)
    EXECUTE FUNCTION pl_item_access_changed();

CREATE FUNCTION pl_relationship_removed() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
BEGIN
    INSERT INTO outbox (household_id, kind, entity_type, entity_id, payload)
    SELECT household_id, 'item.access_changed', 'item', item_id, jsonb_build_object('member_id', member_id)
    FROM removed_rows;
    PERFORM pg_notify('porchlight_outbox', '');
    RETURN NULL;
END $$;

CREATE TRIGGER t30_assignees_access_removed AFTER DELETE ON item_assignees
    REFERENCING OLD TABLE AS removed_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_relationship_removed();
CREATE TRIGGER t30_viewers_access_removed AFTER DELETE ON item_viewers
    REFERENCING OLD TABLE AS removed_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_relationship_removed();

CREATE FUNCTION pl_enqueue_derivation(p_household_id uuid, p_item_id uuid) RETURNS void
    LANGUAGE sql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
    INSERT INTO item_derivation AS d (item_id, household_id) VALUES (p_item_id, p_household_id)
    ON CONFLICT (item_id) DO UPDATE
        SET requested_gen = d.requested_gen + 1, attempts = 0, retry_at = now(), last_error = NULL, updated_at = now();
    SELECT pg_notify('porchlight_derive', '');
$$;

CREATE FUNCTION pl_enqueue_rows() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
DECLARE
    r record;
BEGIN
    FOR r IN SELECT DISTINCT household_id, item_id FROM changed_rows ORDER BY item_id LOOP
        PERFORM pl_enqueue_derivation(r.household_id, r.item_id);
    END LOOP;
    RETURN NULL;
END $$;

CREATE FUNCTION pl_enqueue_new_items() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
DECLARE
    r record;
BEGIN
    FOR r IN SELECT household_id, id FROM new_rows ORDER BY id LOOP
        PERFORM pl_enqueue_derivation(r.household_id, r.id);
    END LOOP;
    RETURN NULL;
END $$;

CREATE TRIGGER items_enqueue_insert AFTER INSERT ON items
    REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_enqueue_new_items();

CREATE FUNCTION pl_enqueue_item_row() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
BEGIN
    -- plpgsql resolves record fields at run time; a CASE over NEW.id/NEW.item_id fails on items (no item_id column).
    IF TG_TABLE_NAME = 'items' THEN
        PERFORM pl_enqueue_derivation(NEW.household_id, NEW.id);
    ELSE
        PERFORM pl_enqueue_derivation(NEW.household_id, NEW.item_id);
    END IF;
    RETURN NULL;
END $$;

CREATE TRIGGER items_enqueue_update AFTER UPDATE ON items
    FOR EACH ROW WHEN (
        (OLD.kind, OLD.time_mode, OLD.tzid, OLD.all_day, OLD.due_date, OLD.due_at, OLD.starts_at, OLD.ends_at,
         OLD.local_due, OLD.local_start, OLD.local_end, OLD.buffer_before_min, OLD.buffer_after_min, OLD.est_minutes,
         OLD.rrule, OLD.recurrence_mode, OLD.completion_interval, OLD.missed_policy, OLD.deleted_at,
         OLD.owner_id, OLD.subject_member_id)
        IS DISTINCT FROM
        (NEW.kind, NEW.time_mode, NEW.tzid, NEW.all_day, NEW.due_date, NEW.due_at, NEW.starts_at, NEW.ends_at,
         NEW.local_due, NEW.local_start, NEW.local_end, NEW.buffer_before_min, NEW.buffer_after_min, NEW.est_minutes,
         NEW.rrule, NEW.recurrence_mode, NEW.completion_interval, NEW.missed_policy, NEW.deleted_at,
         NEW.owner_id, NEW.subject_member_id))
    EXECUTE FUNCTION pl_enqueue_item_row();

CREATE TRIGGER t20_assignees_enqueue_insert AFTER INSERT ON item_assignees
    REFERENCING NEW TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_enqueue_rows();
CREATE TRIGGER t20_assignees_enqueue_delete AFTER DELETE ON item_assignees
    REFERENCING OLD TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_enqueue_rows();
CREATE TRIGGER t20_reminders_enqueue_insert AFTER INSERT ON item_reminders
    REFERENCING NEW TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_enqueue_rows();
CREATE TRIGGER t20_reminders_enqueue_delete AFTER DELETE ON item_reminders
    REFERENCING OLD TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_enqueue_rows();

CREATE TRIGGER occurrence_enqueue AFTER UPDATE ON item_occurrences
    FOR EACH ROW WHEN (
        (OLD.status, OLD.completed_by, OLD.completed_at,
         OLD.override_local_start, OLD.override_local_end, OLD.override_local_due, OLD.override_due_date)
        IS DISTINCT FROM
        (NEW.status, NEW.completed_by, NEW.completed_at,
         NEW.override_local_start, NEW.override_local_end, NEW.override_local_due, NEW.override_due_date))
    EXECUTE FUNCTION pl_enqueue_item_row();

CREATE FUNCTION pl_enqueue_member_items() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
DECLARE
    r record;
BEGIN
    FOR r IN
        SELECT i.household_id, i.id
        FROM items i
        WHERE i.deleted_at IS NULL
          AND (i.owner_id = NEW.id OR i.subject_member_id = NEW.id
               OR EXISTS (SELECT 1 FROM item_assignees a WHERE a.item_id = i.id AND a.member_id = NEW.id))
        ORDER BY i.id
    LOOP
        PERFORM pl_enqueue_derivation(r.household_id, r.id);
    END LOOP;
    RETURN NULL;
END $$;

CREATE TRIGGER member_tz_enqueue AFTER UPDATE OF tzid ON household_members
    FOR EACH ROW WHEN (OLD.tzid IS DISTINCT FROM NEW.tzid) EXECUTE FUNCTION pl_enqueue_member_items();

RESET search_path;
RESET ROLE;
