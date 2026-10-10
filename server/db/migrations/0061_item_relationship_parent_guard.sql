SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Raw DELETE already owns a child tuple; waiting for its parent would invert
-- WriteScope's item -> relationships order. INSERT/bulk writers have the same
-- nonwaiting contract: prelock all parent items NKU in id order or retry 55P03.
-- Take the guard before any child FK/audit/derivation bookkeeping. The existing
-- AFTER-statement parent bump therefore only reacquires locks we already own.
CREATE FUNCTION pl_item_relationship_parent_guard() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
    PERFORM 1 FROM items
    WHERE id = CASE WHEN TG_OP = 'DELETE' THEN OLD.item_id ELSE NEW.item_id END
    FOR NO KEY UPDATE NOWAIT;
    IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER ab_parent_guard BEFORE INSERT OR DELETE ON item_assignees
    FOR EACH ROW EXECUTE FUNCTION pl_item_relationship_parent_guard();
CREATE TRIGGER ab_parent_guard BEFORE INSERT OR DELETE ON item_viewers
    FOR EACH ROW EXECUTE FUNCTION pl_item_relationship_parent_guard();
CREATE TRIGGER ab_parent_guard BEFORE INSERT OR DELETE ON item_reminders
    FOR EACH ROW EXECUTE FUNCTION pl_item_relationship_parent_guard();

-- Trigger-only ABI: no runtime EXECUTE grant, and no table/column grant changes.
REVOKE ALL ON FUNCTION pl_item_relationship_parent_guard() FROM PUBLIC;
RESET search_path;
RESET ROLE;
