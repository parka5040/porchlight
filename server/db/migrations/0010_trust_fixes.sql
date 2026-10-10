SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- A viewer relationship grants visibility only on selected items. Assignee eviction is unchanged.
CREATE OR REPLACE FUNCTION pl_relationship_removed() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
BEGIN
    IF TG_TABLE_NAME = 'item_viewers' THEN
        INSERT INTO outbox (household_id, kind, entity_type, entity_id, payload)
        SELECT r.household_id, 'item.access_changed', 'item', r.item_id,
               jsonb_build_object('member_id', r.member_id)
        FROM removed_rows r JOIN items i ON i.id = r.item_id
        WHERE i.visibility = 'selected';
    ELSE
        INSERT INTO outbox (household_id, kind, entity_type, entity_id, payload)
        SELECT household_id, 'item.access_changed', 'item', item_id,
               jsonb_build_object('member_id', member_id)
        FROM removed_rows;
    END IF;
    PERFORM pg_notify('porchlight_outbox', '');
    RETURN NULL;
END $$;

CREATE OR REPLACE FUNCTION pl_set_once() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NOT (to_jsonb(OLD) ? TG_ARGV[0]) THEN
        RAISE EXCEPTION 'unknown set-once column %', TG_ARGV[0]
            USING ERRCODE = 'PL422', TABLE = TG_TABLE_NAME, COLUMN = TG_ARGV[0];
    END IF;
    IF to_jsonb(OLD) -> TG_ARGV[0] <> 'null'::jsonb
       AND to_jsonb(NEW) -> TG_ARGV[0] IS DISTINCT FROM to_jsonb(OLD) -> TG_ARGV[0] THEN
        RAISE EXCEPTION '% is final once set', TG_ARGV[0]
            USING ERRCODE = 'PL422', TABLE = TG_TABLE_NAME, COLUMN = TG_ARGV[0];
    END IF;
    RETURN NEW;
END $$;

RESET search_path;
RESET ROLE;
