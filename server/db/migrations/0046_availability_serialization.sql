SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- API scopes/worker own the member before touching projections, windows or queues.
-- A raw child writer already owns its window row. It MUST NOT wait for the member
-- while retaining that child lock (API may subsequently need the same window).
-- NOWAIT aborts that raw statement instead, allowing a whole-transaction retry.
CREATE FUNCTION pl_availability_member_lock() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE target uuid;
BEGIN
    IF TG_OP='DELETE' THEN target:=OLD.member_id; ELSE target:=NEW.member_id; END IF;
    PERFORM id FROM household_members WHERE id=target FOR NO KEY UPDATE NOWAIT;
    IF TG_OP='DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END $$;
CREATE TRIGGER ab_member_lock BEFORE INSERT OR UPDATE OR DELETE ON availability_windows
    FOR EACH ROW EXECUTE FUNCTION pl_availability_member_lock();
REVOKE ALL ON FUNCTION pl_availability_member_lock() FROM PUBLIC;

RESET search_path;
RESET ROLE;
