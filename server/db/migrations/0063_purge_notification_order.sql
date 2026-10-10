-- B3: remove event children first, then events while occurrence selection still exists.
-- R56/R56a/R56b catalog closure, counts, history severing and grants are unchanged.
SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
CREATE OR REPLACE FUNCTION pl_purge_relations() RETURNS TABLE(ord integer, table_name text, predicate text)
 LANGUAGE sql IMMUTABLE SET search_path = pg_catalog, porchlight, pg_temp AS $$
 VALUES
 (-5,'notification_event_delivery','event_id IN (SELECT id FROM porchlight.notification_events WHERE household_id=$2 AND (item_id=ANY($1) OR occurrence_id IN (SELECT id FROM porchlight.item_occurrences WHERE item_id=ANY($1))))'),
 (-4,'notification_sources','event_id IN (SELECT id FROM porchlight.notification_events WHERE household_id=$2 AND (item_id=ANY($1) OR occurrence_id IN (SELECT id FROM porchlight.item_occurrences WHERE item_id=ANY($1))))'),
 (-3,'notifications','item_id=ANY($1) OR occurrence_id IN (SELECT id FROM porchlight.item_occurrences WHERE item_id=ANY($1))'),
 (-2,'attachments','item_id=ANY($1)'),
 (-1,'comments','item_id=ANY($1)'),
 (0,'notification_events','item_id=ANY($1) OR occurrence_id IN (SELECT id FROM porchlight.item_occurrences WHERE item_id=ANY($1))'),
 (1,'ics_uid_map','item_id=ANY($1)'),
 (2,'schedule_suggestions','item_id=ANY($1)'),
 (3,'list_entry_checks','entry_id IN (SELECT e.id FROM porchlight.list_entries e JOIN porchlight.lists l ON l.id=e.list_id WHERE l.item_id=ANY($1)) OR occurrence_id IN (SELECT id FROM porchlight.item_occurrences WHERE item_id=ANY($1))'),
 (4,'list_viewers','list_id IN (SELECT id FROM porchlight.lists WHERE item_id=ANY($1))'),
 (5,'list_entry_revisions','entry_id IN (SELECT e.id FROM porchlight.list_entries e JOIN porchlight.lists l ON l.id=e.list_id WHERE l.item_id=ANY($1))'),
 (6,'list_revisions','list_id IN (SELECT id FROM porchlight.lists WHERE item_id=ANY($1))'),
 (7,'list_entries','list_id IN (SELECT id FROM porchlight.lists WHERE item_id=ANY($1))'),
 (8,'lists','item_id=ANY($1)'),
 (9,'member_agenda','item_id=ANY($1)'),
 (10,'scheduled_reminders','item_id=ANY($1)'),
 (11,'item_occurrences','item_id=ANY($1)'),
 (12,'item_assignees','item_id=ANY($1)'),
 (13,'item_viewers','item_id=ANY($1)'),
 (14,'item_reminders','item_id=ANY($1)'),
 (15,'item_derivation','item_id=ANY($1)'),
 (16,'item_revisions','item_id=ANY($1)'),
 (17,'task_events','item_id=ANY($1)'),
 (19,'items','id=ANY($1)')
$$;
REVOKE ALL ON FUNCTION pl_purge_relations() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pl_purge_relations() TO porchlight_maintenance;

RESET search_path;
RESET ROLE;
