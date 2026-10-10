SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- S7a's real ACL may already exist on a long-lived database when this lower-numbered
-- migration is installed. Never replace that implementation with the fail-closed stub.
DO $$
BEGIN
    IF to_regprocedure('porchlight.pl_list_can_read(uuid,uuid)') IS NULL THEN
        EXECUTE $fn$ CREATE FUNCTION porchlight.pl_list_can_read(uuid,uuid) RETURNS boolean
            LANGUAGE sql STABLE SET search_path = pg_catalog, porchlight, pg_temp
            AS 'SELECT false' $fn$;
    END IF;
END $$;
GRANT EXECUTE ON FUNCTION pl_list_can_read(uuid,uuid) TO porchlight_app, porchlight_agent;

-- Former readers must be real live app members, not dependent subjects or deleted
-- relationship targets. Keep the ABI used by the access-change producer unchanged.
CREATE OR REPLACE FUNCTION pl_item_readers(p_household_id uuid, p_item_id uuid, p_owner_id uuid, p_visibility text)
RETURNS uuid[] LANGUAGE sql STABLE SET search_path = pg_catalog, porchlight, pg_temp AS $$
    SELECT coalesce(array_agg(DISTINCT m.id ORDER BY m.id), '{}')
    FROM household_members m
    WHERE m.household_id=p_household_id AND m.deleted_at IS NULL AND m.role<>'dependent'
      AND (m.id=p_owner_id
        OR EXISTS(SELECT 1 FROM item_assignees a WHERE a.item_id=p_item_id AND a.member_id=m.id)
        OR (p_visibility='selected' AND EXISTS(SELECT 1 FROM item_viewers v WHERE v.item_id=p_item_id AND v.member_id=m.id))
        OR p_visibility='household')
$$;

RESET ROLE;
RESET search_path;
