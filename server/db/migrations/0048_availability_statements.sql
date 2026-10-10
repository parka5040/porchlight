SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Raw SQL bookkeeping is once per affected member per statement, not per window.
DROP TRIGGER availability_changed ON availability_windows;
CREATE OR REPLACE FUNCTION pl_availability_changed() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE r record;
BEGIN
    FOR r IN SELECT DISTINCT member_id,household_id FROM changed_windows ORDER BY member_id LOOP
        INSERT INTO availability_profiles(member_id,household_id) VALUES(r.member_id,r.household_id)
        ON CONFLICT(member_id) DO UPDATE SET updated_at=now();
        PERFORM pl_enqueue_member_load(r.member_id);
    END LOOP;
    RETURN NULL;
END $$;
CREATE TRIGGER availability_inserted AFTER INSERT ON availability_windows
    REFERENCING NEW TABLE AS changed_windows FOR EACH STATEMENT EXECUTE FUNCTION pl_availability_changed();
CREATE TRIGGER availability_updated AFTER UPDATE ON availability_windows
    REFERENCING NEW TABLE AS changed_windows FOR EACH STATEMENT EXECUTE FUNCTION pl_availability_changed();
CREATE TRIGGER availability_deleted AFTER DELETE ON availability_windows
    REFERENCING OLD TABLE AS changed_windows FOR EACH STATEMENT EXECUTE FUNCTION pl_availability_changed();

RESET search_path;
RESET ROLE;
