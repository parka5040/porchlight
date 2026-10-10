SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

ALTER TABLE items
    ADD COLUMN series_id uuid,
    ADD COLUMN split_from_item_id uuid,
    ADD COLUMN split_from_seq int,
    ADD COLUMN nominal_local_anchor timestamp,
    ADD COLUMN nominal_wall_duration bigint,
    ADD FOREIGN KEY (household_id, series_id) REFERENCES items(household_id,id),
    ADD FOREIGN KEY (household_id, split_from_item_id) REFERENCES items(household_id,id),
    ADD CHECK ((split_from_item_id IS NULL) = (split_from_seq IS NULL)),
    ADD CHECK (split_from_seq IS NULL OR split_from_seq >= 0),
    ADD CONSTRAINT items_nominal_shape CHECK (coalesce((
        (nominal_local_anchor IS NULL AND nominal_wall_duration IS NULL)
        OR (nominal_local_anchor IS NOT NULL AND time_mode='fixed' AND NOT all_day AND recurrence_mode='calendar'
            AND ((kind='deadline' AND nominal_wall_duration IS NULL)
                 OR (kind='appointment' AND nominal_wall_duration > 0 AND nominal_wall_duration IS NOT NULL)))),false));
-- Lineage FKs deliberately RESTRICT physical deletion. S5 purge must remove occurrence/
-- relationship/derived children first, then clear self-root series_id references and
-- delete descendants/tails before their ancestors/heads (or clear surviving lineage).
CREATE INDEX items_series_id_idx ON items(household_id,series_id) WHERE series_id IS NOT NULL;
CREATE INDEX items_split_from_item_id_idx ON items(household_id,split_from_item_id) WHERE split_from_item_id IS NOT NULL;

ALTER TABLE item_occurrences ADD COLUMN skipped_at timestamptz;
-- Historical skip instants cannot be reconstructed: updated_at is an approximation.
UPDATE item_occurrences SET skipped_at=updated_at WHERE status='skipped';
COMMENT ON COLUMN item_occurrences.skipped_at IS 'Skip recording instant: app supplied operation time; raw SQL statement_timestamp(). Not the occurrence anchor. Retained on reopen; every transition into skipped records a fresh time.';
CREATE FUNCTION pl_record_skip() RETURNS trigger LANGUAGE plpgsql
    SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
    IF TG_OP='UPDATE' AND NEW.status IS NOT DISTINCT FROM OLD.status THEN
        NEW.skipped_at := OLD.skipped_at;
    ELSIF NEW.status='skipped' THEN
        IF TG_OP='UPDATE' AND NEW.skipped_at IS NOT DISTINCT FROM OLD.skipped_at THEN
            NEW.skipped_at := statement_timestamp();
        ELSE
            NEW.skipped_at := coalesce(NEW.skipped_at,statement_timestamp());
        END IF;
    ELSIF TG_OP='UPDATE' AND NEW.status NOT IN ('done','skipped') THEN
        NEW.skipped_at := OLD.skipped_at;
    ELSE
        NEW.skipped_at := NULL;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER ab_record_skip BEFORE INSERT OR UPDATE ON item_occurrences
    FOR EACH ROW EXECUTE FUNCTION pl_record_skip();
ALTER TABLE item_occurrences ADD CHECK (status<>'skipped' OR skipped_at IS NOT NULL);
-- Persistent bookkeeping distinguishes reopened closures and ever-moved successors.
-- These columns are not granted to agents and are never written by derivation.
ALTER TABLE item_occurrences ADD COLUMN closed_at timestamptz, ADD COLUMN board_moved boolean NOT NULL DEFAULT false;
UPDATE item_occurrences SET closed_at=coalesce(completed_at,skipped_at),board_moved=(board_position<>'80');
COMMENT ON COLUMN item_occurrences.closed_at IS 'Last closure instant, retained on reopen for R31 successor cleanup. Historical skipped values use updated_at approximation.';
COMMENT ON COLUMN item_occurrences.board_moved IS 'Ever explicitly positioned on board, including moves back to default; R31 touched-successor marker.';
CREATE FUNCTION pl_record_closure() RETURNS trigger LANGUAGE plpgsql
    SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
    IF NEW.status='done' THEN NEW.closed_at := NEW.completed_at;
    ELSIF NEW.status='skipped' THEN NEW.closed_at := NEW.skipped_at;
    ELSIF TG_OP='UPDATE' THEN NEW.closed_at := OLD.closed_at;
    ELSE NEW.closed_at := NULL;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER ac_record_closure BEFORE INSERT OR UPDATE ON item_occurrences
    FOR EACH ROW EXECUTE FUNCTION pl_record_closure();
CREATE FUNCTION pl_record_board_move() RETURNS trigger LANGUAGE plpgsql
    SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
    -- WriteScope supplies the explicit mover while automatically respreading a tie.
    -- Bystanders retain their history; raw board writes (no marker) are explicit moves.
    IF nullif(current_setting('porchlight.board_mover',true),'') IS NOT NULL
       AND current_setting('porchlight.board_mover',true) <> NEW.id::text THEN
        NEW.board_moved := OLD.board_moved;
    ELSE
        NEW.board_moved := true;
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER ac_record_board_move BEFORE UPDATE OF board_position ON item_occurrences
    FOR EACH ROW EXECUTE FUNCTION pl_record_board_move();

-- Reparent is an authoritative occurrence change, independent of derived tickles.
CREATE TRIGGER occurrence_reparent_outbox AFTER UPDATE OF item_id,seq ON item_occurrences
    FOR EACH ROW WHEN ((OLD.item_id,OLD.seq) IS DISTINCT FROM (NEW.item_id,NEW.seq))
    EXECUTE FUNCTION pl_row_outbox('occurrence.changed','occurrence');

ALTER TABLE item_derivation ADD COLUMN calendar_horizon timestamptz;
COMMENT ON COLUMN item_derivation.calendar_horizon IS 'Last successfully expanded exclusive calendar horizon. Worker ack only; never authoritative.';

COMMENT ON COLUMN items.nominal_wall_duration IS 'Exact nominal wall-clock microseconds, never calendar months (R14).';

ALTER TABLE item_occurrences ALTER COLUMN board_position SET DEFAULT '80';
-- The core parser accepts exactly nonempty, even-length lowercase hex ending in byte 80.
-- A board is household-scoped. Rebalance the whole affected board in stable legacy order.
WITH bad_boards AS (
    SELECT DISTINCT household_id FROM item_occurrences
    WHERE board_position !~ '^([0-9a-f]{2})*80$' OR octet_length(board_position)>64
), ranked AS (
    SELECT id, div(row_number() OVER w * 18446744073709551615::numeric,
                   (count(*) OVER (PARTITION BY household_id) + 1)::numeric) AS rank
    FROM item_occurrences WHERE household_id IN (SELECT household_id FROM bad_boards)
    WINDOW w AS (PARTITION BY household_id ORDER BY status, board_position COLLATE "C", id)
)
UPDATE item_occurrences o SET board_position =
    lpad(to_hex(floor(r.rank / 4294967296)::bigint),8,'0') ||
    lpad(to_hex(mod(r.rank,4294967296)::bigint),8,'0') || '80'
FROM ranked r WHERE o.id=r.id;

DROP TRIGGER items_enqueue_update ON items;
CREATE TRIGGER items_enqueue_update AFTER UPDATE OF
    kind,time_mode,tzid,all_day,due_date,due_at,starts_at,ends_at,local_due,local_start,local_end,
    buffer_before_min,buffer_after_min,est_minutes,rrule,recurrence_mode,completion_interval,missed_policy,
    deleted_at,owner_id,subject_member_id,series_id,split_from_item_id,split_from_seq,
    nominal_local_anchor,nominal_wall_duration ON items
    FOR EACH ROW WHEN (
        (OLD.kind,OLD.time_mode,OLD.tzid,OLD.all_day,OLD.due_date,OLD.due_at,OLD.starts_at,OLD.ends_at,
         OLD.local_due,OLD.local_start,OLD.local_end,OLD.buffer_before_min,OLD.buffer_after_min,OLD.est_minutes,
         OLD.rrule,OLD.recurrence_mode,OLD.completion_interval,OLD.missed_policy,OLD.deleted_at,
         OLD.owner_id,OLD.subject_member_id,OLD.series_id,OLD.split_from_item_id,OLD.split_from_seq,
         OLD.nominal_local_anchor,OLD.nominal_wall_duration)
        IS DISTINCT FROM
        (NEW.kind,NEW.time_mode,NEW.tzid,NEW.all_day,NEW.due_date,NEW.due_at,NEW.starts_at,NEW.ends_at,
         NEW.local_due,NEW.local_start,NEW.local_end,NEW.buffer_before_min,NEW.buffer_after_min,NEW.est_minutes,
         NEW.rrule,NEW.recurrence_mode,NEW.completion_interval,NEW.missed_policy,NEW.deleted_at,
         NEW.owner_id,NEW.subject_member_id,NEW.series_id,NEW.split_from_item_id,NEW.split_from_seq,
         NEW.nominal_local_anchor,NEW.nominal_wall_duration))
    EXECUTE FUNCTION pl_enqueue_item_row();
-- pl_enqueue_derivation already NOTIFYs and resets attempts/retry/error on every generation bump.

-- No running_since: election is an uncommitted row lock, so that field could never
-- be observed. last_run_at/last_success_at and attempts describe committed outcomes.
CREATE TABLE scheduler_jobs (
    name text PRIMARY KEY CHECK (length(name) BETWEEN 1 AND 100),
    last_run_at timestamptz,
    last_success_at timestamptz,
    next_due_at timestamptz NOT NULL,
    last_error text,
    attempts int NOT NULL DEFAULT 0 CHECK (attempts>=0)
);
GRANT SELECT,INSERT,UPDATE ON scheduler_jobs TO porchlight_app;
GRANT SELECT ON scheduler_jobs TO porchlight_agent;

-- The app cannot directly INSERT outbox or execute enqueue bookkeeping functions.
-- Narrow trusted entry points retain that split; IDs/categories only, no titles in events.
CREATE FUNCTION pl_emit_derived(p_item uuid, p_gen bigint, p_error boolean DEFAULT false) RETURNS void
    LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
    INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
    SELECT household_id,'derived','item',id,
           jsonb_build_object('item_id',id,'applied_gen',p_gen) ||
           CASE WHEN p_error THEN '{"error":true}'::jsonb ELSE '{}'::jsonb END
    FROM items WHERE id=p_item;
    SELECT pg_notify('porchlight_outbox','');
$$;
CREATE FUNCTION pl_request_derivation(p_item uuid) RETURNS void
    LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
    SELECT pl_enqueue_derivation(household_id,id) FROM items WHERE id=p_item;
$$;
GRANT EXECUTE ON FUNCTION pl_emit_derived(uuid,bigint,boolean),pl_request_derivation(uuid) TO porchlight_app;

-- Zone-source/authority changes are member events even when the effective zone is unchanged.
DROP TRIGGER member_outbox ON household_members;
CREATE TRIGGER member_outbox AFTER UPDATE OF tzid,tz_source,tz_authority_device_id ON household_members
    FOR EACH ROW WHEN ((OLD.tzid,OLD.tz_source,OLD.tz_authority_device_id)
                       IS DISTINCT FROM (NEW.tzid,NEW.tz_source,NEW.tz_authority_device_id))
    EXECUTE FUNCTION pl_row_outbox('member.changed','member');

-- R27: retain occurrence identity and user state across a lineage-checked split.
DROP TRIGGER aa_version_guard ON item_occurrences;
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON item_occurrences
    FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id', 'household_id');
CREATE FUNCTION pl_occurrence_reparent_guard() RETURNS trigger
    LANGUAGE plpgsql SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
    IF (NEW.item_id, NEW.seq) IS DISTINCT FROM (OLD.item_id, OLD.seq) AND NOT EXISTS (
        SELECT 1 FROM items tail
        WHERE tail.id = NEW.item_id AND tail.household_id = OLD.household_id
          AND tail.split_from_item_id = OLD.item_id
          AND OLD.seq >= tail.split_from_seq
          AND NEW.seq = OLD.seq - tail.split_from_seq
    ) THEN
        RAISE EXCEPTION 'occurrence keys may change only to a valid split tail'
            USING ERRCODE = 'PL422', TABLE = TG_TABLE_NAME;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER ab_reparent_guard BEFORE UPDATE ON item_occurrences
    FOR EACH ROW EXECUTE FUNCTION pl_occurrence_reparent_guard();
-- Device revocation already owns the device first (S1). Clearing authority changes
-- no timezone, so it enqueues no domain jobs and takes only the final member lock.
CREATE FUNCTION pl_clear_revoked_time_authority() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
    UPDATE household_members SET tz_authority_device_id = NULL
    WHERE tz_authority_device_id = NEW.id;
    RETURN NULL;
END $$;
CREATE TRIGGER device_clear_time_authority AFTER UPDATE OF revoked_at ON devices
    FOR EACH ROW WHEN (OLD.revoked_at IS NULL AND NEW.revoked_at IS NOT NULL)
    EXECUTE FUNCTION pl_clear_revoked_time_authority();

RESET search_path;
RESET ROLE;
