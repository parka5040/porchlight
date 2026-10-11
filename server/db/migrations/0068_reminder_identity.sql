SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- NULL means this row's own id, preserving every existing dedupe witness without
-- rewriting populated tables. Split tails copy these logical identities, not FKs:
-- purging or editing a head must never change the surviving tail's delivery key.
ALTER TABLE items ADD COLUMN reminder_origin_id uuid;
ALTER TABLE item_reminders ADD COLUMN delivery_id uuid;
-- Offsets within one item need distinct delivery keys, including NULL/self aliases.
-- Split heads and tails may still share those keys across different items.
CREATE UNIQUE INDEX item_reminders_delivery_identity
 ON item_reminders (item_id, coalesce(delivery_id, id));

CREATE FUNCTION pl_reminder_origin_immutable() RETURNS trigger
 LANGUAGE plpgsql SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
 IF NEW.reminder_origin_id IS DISTINCT FROM OLD.reminder_origin_id THEN
  RAISE EXCEPTION 'reminder origin is immutable' USING ERRCODE='PL403';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER reminder_origin_immutable BEFORE UPDATE ON items
 FOR EACH ROW EXECUTE FUNCTION pl_reminder_origin_immutable();
-- item_reminders already rejects all UPDATEs. Existing table grants, insert audit,
-- parent bumps and derivation/outbox triggers cover copied delivery identities.
REVOKE ALL ON FUNCTION pl_reminder_origin_immutable() FROM PUBLIC;

CREATE OR REPLACE FUNCTION pl_reminder_offsets(p_item uuid,p_member uuid) RETURNS TABLE(id uuid,offset_minutes int)
 LANGUAGE sql STABLE SET search_path = pg_catalog, porchlight, pg_temp AS $$
 SELECT coalesce(r.delivery_id,r.id),r.offset_minutes FROM item_reminders r WHERE r.item_id=p_item
 UNION ALL
 SELECT md5(coalesce(i.reminder_origin_id,i.id)::text||':'||p_member::text||':'||v::text)::uuid,v FROM items i
 JOIN notification_prefs p ON p.member_id=p_member AND p.household_id=i.household_id
 CROSS JOIN LATERAL unnest(CASE i.kind WHEN 'todo' THEN p.default_todo_offsets WHEN 'deadline' THEN p.default_deadline_offsets ELSE p.default_appointment_offsets END) v
 WHERE i.id=p_item AND NOT EXISTS(SELECT 1 FROM item_reminders r WHERE r.item_id=p_item)
$$;

RESET search_path;
RESET ROLE;
