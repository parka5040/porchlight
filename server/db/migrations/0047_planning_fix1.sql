SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

ALTER TABLE member_load_jobs ADD COLUMN attempts int NOT NULL DEFAULT 0 CHECK(attempts>=0),
    ADD COLUMN retry_at timestamptz NOT NULL DEFAULT now(), ADD COLUMN last_error text;
GRANT UPDATE(attempts,retry_at,last_error) ON member_load_jobs TO porchlight_app;
CREATE OR REPLACE FUNCTION pl_enqueue_member_load(p_member uuid) RETURNS void
    LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
    INSERT INTO member_load_jobs(member_id) VALUES(p_member)
    ON CONFLICT(member_id) DO UPDATE SET requested_gen=member_load_jobs.requested_gen+1,updated_at=now(),attempts=0,retry_at=now(),last_error=null;
    PERFORM pg_notify('porchlight_derive','');
END $$;
CREATE INDEX schedule_suggestions_member_page ON schedule_suggestions(member_id,id);

-- Immutable wakeups for peer members a scope does NOT own. Never upsert a shared
-- member queue row while holding item/member locks: that could invert queue locks.
-- Each transaction inserts a fresh signal; only the member-only worker deletes it.
-- No member FK: even a KEY SHARE acquisition would touch an unheld member.
-- The worker acknowledges orphan signals after physical member purge.
CREATE TABLE member_load_signals (
    id uuid PRIMARY KEY DEFAULT uuidv7(), member_id uuid NOT NULL
);
CREATE INDEX member_load_signals_member ON member_load_signals(member_id);
CREATE FUNCTION pl_signal_member_load(p_members uuid[]) RETURNS void
    LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
    INSERT INTO member_load_signals(member_id) SELECT DISTINCT unnest(p_members) ON CONFLICT(id) DO NOTHING;
    PERFORM pg_notify('porchlight_derive','');
END $$;
REVOKE ALL ON FUNCTION pl_signal_member_load(uuid[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pl_signal_member_load(uuid[]) TO porchlight_app;
GRANT SELECT,DELETE ON member_load_signals TO porchlight_app,porchlight_maintenance;

-- Terminal suggestion history cannot be reopened by agent SQL (or any writer).
CREATE FUNCTION pl_suggestion_transition() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
    IF NEW.status IS DISTINCT FROM OLD.status AND (OLD.status<>'proposed' OR NEW.status NOT IN ('accepted','dismissed')) THEN
        RAISE EXCEPTION 'invalid suggestion transition' USING ERRCODE='PL422';
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER ab_transition BEFORE UPDATE OF status ON schedule_suggestions
    FOR EACH ROW EXECUTE FUNCTION pl_suggestion_transition();
REVOKE ALL ON FUNCTION pl_suggestion_transition() FROM PUBLIC;

-- Due-only todo/deadline anchors require complete span overrides. Appointment
-- anchors already carry both generated local bounds and may inherit the other bound.
ALTER TABLE item_occurrences ADD CONSTRAINT occurrence_override_span_pair
    CHECK ((override_local_start IS NULL)=(override_local_end IS NULL)
        OR (local_start IS NOT NULL AND local_end IS NOT NULL));

RESET search_path;
RESET ROLE;
