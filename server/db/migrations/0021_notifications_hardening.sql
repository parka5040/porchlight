SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- S5 physical item purge can retain unrelated sources of a coalesced notification.
-- Event deletion cascades its link; nullable row-level refs must never block purge.
ALTER TABLE notifications DROP CONSTRAINT notifications_household_id_item_id_fkey;
ALTER TABLE notifications ADD CONSTRAINT notifications_household_id_item_id_fkey
 FOREIGN KEY(household_id,item_id) REFERENCES items(household_id,id) ON DELETE SET NULL(item_id);
CREATE FUNCTION pl_valid_notification_offsets(offsets int[]) RETURNS boolean
 LANGUAGE sql IMMUTABLE SET search_path = pg_catalog, porchlight, pg_temp AS $$
 SELECT cardinality(offsets)<=16 AND coalesce(array_ndims(offsets),1)=1
 AND cardinality(offsets)=(SELECT count(DISTINCT v) FROM unnest(offsets) v)
 AND 0<=ALL(offsets) AND 525600>=ALL(offsets)
$$;
REVOKE ALL ON FUNCTION pl_valid_notification_offsets(int[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pl_valid_notification_offsets(int[]) TO porchlight_app;
ALTER TABLE notification_prefs ADD CONSTRAINT notification_offsets_valid CHECK(
 pl_valid_notification_offsets(default_todo_offsets) AND
 pl_valid_notification_offsets(default_deadline_offsets) AND
 pl_valid_notification_offsets(default_appointment_offsets));

RESET search_path;
RESET ROLE;
