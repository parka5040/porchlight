-- O2a once-only S6a/S7b retention extension. Preserve coalesced inbox/history,
-- sever item references explicitly; remove only selected events' source links.
SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
GRANT SELECT, DELETE ON notification_sources,notification_event_delivery,notifications,notification_events,comments,attachments TO porchlight_maintenance;
GRANT UPDATE(item_id,occurrence_id) ON notifications TO porchlight_maintenance;
GRANT UPDATE(last_referenced_at) ON blobs TO porchlight_maintenance;
GRANT USAGE, CREATE ON SCHEMA porchlight TO porchlight_maintenance;
CREATE OR REPLACE FUNCTION pl_purge_relations() RETURNS TABLE(ord integer, table_name text, predicate text)
 LANGUAGE sql IMMUTABLE SET search_path = pg_catalog, porchlight, pg_temp AS $$
 VALUES
 (-5,'notification_event_delivery','event_id IN (SELECT id FROM porchlight.notification_events WHERE household_id=$2 AND (item_id=ANY($1) OR occurrence_id IN (SELECT id FROM porchlight.item_occurrences WHERE item_id=ANY($1))))'),
 (-4,'notification_sources','event_id IN (SELECT id FROM porchlight.notification_events WHERE household_id=$2 AND (item_id=ANY($1) OR occurrence_id IN (SELECT id FROM porchlight.item_occurrences WHERE item_id=ANY($1))))'),
 (-3,'notifications','item_id=ANY($1) OR occurrence_id IN (SELECT id FROM porchlight.item_occurrences WHERE item_id=ANY($1))'),
 (-2,'attachments','item_id=ANY($1)'),
 (-1,'comments','item_id=ANY($1)'),
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
 (18,'notification_events','item_id=ANY($1) OR occurrence_id IN (SELECT id FROM porchlight.item_occurrences WHERE item_id=ANY($1))'),
 (19,'items','id=ANY($1)')
$$;
REVOKE ALL ON FUNCTION pl_purge_relations() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pl_purge_relations() TO porchlight_maintenance;

-- R56b: owner-owned catalog probe covers the FK closure, not only immediate
-- item/occurrence dependents. No runtime grants or DDL, including for new tables.
-- It locks selected rows in EVERY relation the plan deletes before probing, so
-- concurrent FK inserts cannot race the guard (even into history/notification
-- rows). Locks do not write domain data or invoke update/audit triggers.
CREATE OR REPLACE FUNCTION pl_purge_unhandled_relation(p_household uuid, p_ids uuid[]) RETURNS text
 LANGUAGE plpgsql SECURITY DEFINER
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE dep record; relation record; join_predicate text; has_rows boolean; order_by text; extra text; scope text;
BEGIN
 FOR relation IN SELECT * FROM pl_purge_relations()
  WHERE to_regclass('porchlight.'||table_name) IS NOT NULL
  ORDER BY CASE table_name WHEN 'items' THEN -100 WHEN 'item_occurrences' THEN -99
   WHEN 'item_derivation' THEN -98 WHEN 'notification_events' THEN -97
   WHEN 'notifications' THEN -96 WHEN 'comments' THEN -95 ELSE ord END
 LOOP
  SELECT string_agg(format('%I',a.attname),',' ORDER BY k.n) INTO order_by
   FROM pg_index i CROSS JOIN LATERAL unnest(i.indkey) WITH ORDINALITY k(attnum,n)
   JOIN pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=k.attnum
   WHERE i.indrelid=to_regclass('porchlight.'||relation.table_name) AND i.indisprimary;
  scope := CASE WHEN relation.table_name IN ('notification_sources','notification_event_delivery') THEN 'true' ELSE 'household_id=$2' END;
  EXECUTE format('SELECT 1 FROM porchlight.%I WHERE %s AND (%s) ORDER BY %s FOR UPDATE',
   relation.table_name,scope,relation.predicate,coalesce(order_by,'ctid')) USING p_ids,p_household;
 END LOOP;
 FOR dep IN
  -- UNION (not UNION ALL) terminates self-references/cycles. Walk every FK,
  -- irrespective of schema or ON DELETE action, to arbitrary depth; probe all
  -- inbound edges whose parent is actually removed by the shared retention plan.
  WITH RECURSIVE closure(relid) AS (
   SELECT to_regclass('porchlight.'||table_name)::oid FROM pl_purge_relations()
    WHERE to_regclass('porchlight.'||table_name) IS NOT NULL
   UNION
   SELECT c.conrelid FROM pg_constraint c JOIN closure ON closure.relid=c.confrelid
    WHERE c.contype='f'
  )
  SELECT c.conrelid,c.confrelid,c.conkey,c.confkey,r.relname,ns.nspname,
   pr.relname AS parent_name,plan.predicate AS parent_predicate,
   child_plan.predicate AS child_predicate,
   EXISTS (
    SELECT 1 FROM (VALUES
     ('items','series_id','items'),('items','split_from_item_id','items'),
     ('item_assignees','item_id','items'),('item_viewers','item_id','items'),
     ('item_reminders','item_id','items'),('item_occurrences','item_id','items'),
     ('member_agenda','item_id','items'),('member_agenda','occurrence_id','item_occurrences'),
     ('scheduled_reminders','item_id','items'),('scheduled_reminders','occurrence_id','item_occurrences'),
     ('scheduled_reminders','reminder_id','item_reminders'),
     ('item_derivation','item_id','items'),('item_revisions','item_id','items'),
     ('task_events','item_id','items'),('notification_events','item_id','items'),
     ('lists','item_id','items'),('list_entries','list_id','lists'),
     ('list_viewers','list_id','lists'),('list_revisions','list_id','lists'),
     ('list_entry_checks','entry_id','list_entries'),('list_entry_revisions','entry_id','list_entries'),
     ('ics_uid_map','item_id','items'),('schedule_suggestions','item_id','items'),
     ('comments','item_id','items'),('attachments','item_id','items'),
     ('attachments','comment_id','comments'),('notifications','item_id','items'),
     ('notification_sources','event_id','notification_events'),
     ('notification_event_delivery','event_id','notification_events')
    ) AS handled(table_name,column_name,parent_name)
    WHERE ns.nspname='porchlight' AND r.relname=handled.table_name
     AND c.confrelid=to_regclass('porchlight.'||handled.parent_name)
     AND (SELECT array_agg(a.attname::text ORDER BY k.ordinality)
          FROM unnest(c.conkey) WITH ORDINALITY k(attnum,ordinality)
          JOIN pg_attribute a ON a.attrelid=c.conrelid AND a.attnum=k.attnum)
         =CASE WHEN handled.table_name IN ('notification_sources','notification_event_delivery') THEN ARRAY[handled.column_name] WHEN handled.table_name='attachments' AND handled.parent_name='comments' THEN ARRAY['household_id','item_id',handled.column_name] ELSE ARRAY['household_id',handled.column_name] END
     AND (SELECT array_agg(a.attname::text ORDER BY k.ordinality)
          FROM unnest(c.confkey) WITH ORDINALITY k(attnum,ordinality)
          JOIN pg_attribute a ON a.attrelid=c.confrelid AND a.attnum=k.attnum)
         =CASE WHEN handled.table_name IN ('notification_sources','notification_event_delivery') THEN ARRAY['id'] WHEN handled.table_name='attachments' AND handled.parent_name='comments' THEN ARRAY['household_id','item_id','id'] ELSE ARRAY['household_id','id'] END
   ) AS handled
  FROM pg_constraint c JOIN closure ON closure.relid=c.confrelid
  JOIN pg_class r ON r.oid=c.conrelid JOIN pg_namespace ns ON ns.oid=r.relnamespace
  JOIN pg_class pr ON pr.oid=c.confrelid
  JOIN pl_purge_relations() plan ON c.confrelid=to_regclass('porchlight.'||plan.table_name)
  LEFT JOIN pl_purge_relations() child_plan ON c.conrelid=to_regclass('porchlight.'||child_plan.table_name)
  WHERE c.contype='f' AND plan.table_name <> 'notifications'
  ORDER BY ns.nspname,r.relname,c.oid
 LOOP
  SELECT string_agg(format('d.%I=p.%I',src.attname,dst.attname),' AND ' ORDER BY k.n)
   INTO join_predicate
   FROM generate_subscripts(dep.conkey,1) k(n)
   JOIN pg_attribute src ON src.attrelid=dep.conrelid AND src.attnum=dep.conkey[k.n]
   JOIN pg_attribute dst ON dst.attrelid=dep.confrelid AND dst.attnum=dep.confkey[k.n];
  -- Even a known FK cannot cascade a row outside the explicit child selection.
  -- Exact FK shapes remain required: a new shape on a handled table is unknown.
  extra := '';
  IF dep.handled AND dep.child_predicate IS NOT NULL THEN
   extra := format(' AND NOT EXISTS (SELECT 1 FROM porchlight.%I selected WHERE selected.tableoid=d.tableoid AND selected.ctid=d.ctid AND %s AND (%s))',
    dep.relname,CASE WHEN dep.relname IN ('notification_sources','notification_event_delivery') THEN 'true' ELSE 'selected.household_id=$2' END,dep.child_predicate);
  END IF;
  EXECUTE format('SELECT EXISTS(SELECT 1 FROM %I.%I d JOIN (SELECT * FROM porchlight.%I WHERE %s AND (%s)) p ON %s WHERE true%s)',
   dep.nspname,dep.relname,dep.parent_name,CASE WHEN dep.parent_name IN ('notification_sources','notification_event_delivery') THEN 'true' ELSE 'household_id=$2' END,dep.parent_predicate,join_predicate,extra)
   INTO has_rows USING p_ids,p_household;
  IF has_rows THEN RETURN format('%I.%I',dep.nspname,dep.relname); END IF;
 END LOOP;
 RETURN NULL;
END $$;
REVOKE ALL ON FUNCTION pl_purge_unhandled_relation(uuid,uuid[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pl_purge_unhandled_relation(uuid,uuid[]) TO porchlight_maintenance;

SET LOCAL ROLE porchlight_maintenance;
CREATE OR REPLACE FUNCTION pl_purge_items(p_household uuid, p_ids uuid[]) RETURNS jsonb
 LANGUAGE plpgsql SECURITY DEFINER
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE ids uuid[]; counts jsonb := '{}'; n bigint; relation record; log_id uuid; unhandled text; scope text;
BEGIN
 IF p_ids IS NULL OR cardinality(p_ids)=0 OR cardinality(p_ids)>200 THEN
  RAISE EXCEPTION 'invalid purge selection' USING ERRCODE='PL422';
 END IF;
 -- Parent locks prevent concurrent restore or relationship changes. Physical
 -- deletes use UPDATE (not NKU); child locks follow parents in id order.
 PERFORM id FROM items WHERE household_id=p_household AND id=ANY(p_ids) ORDER BY id FOR UPDATE;
 SELECT array_agg(id ORDER BY id) INTO ids FROM items
  WHERE household_id=p_household AND id=ANY(p_ids) AND deleted_at IS NOT NULL;
 IF cardinality(ids) IS DISTINCT FROM cardinality(p_ids) THEN
  RAISE EXCEPTION 'purge requires unique deleted items' USING ERRCODE='PL422';
 END IF;
 -- Lineage references from a survivor cannot be silently severed.
 IF EXISTS(SELECT 1 FROM items WHERE household_id=p_household AND NOT(id=ANY(ids)) AND
    (series_id=ANY(ids) OR split_from_item_id=ANY(ids))) THEN
  RAISE EXCEPTION 'purge requires the referenced lineage' USING ERRCODE='PL422';
 END IF;
 PERFORM id FROM item_occurrences WHERE item_id=ANY(ids) ORDER BY id FOR UPDATE;
 PERFORM item_id FROM item_derivation WHERE item_id=ANY(ids) ORDER BY item_id FOR UPDATE;
 -- The owner probe locks ALL selected deletion parents before inspecting the FK
 -- closure. S6a/S7b remain unhandled; O2 alone extends counted retention.
 unhandled := pl_purge_unhandled_relation(p_household,ids);
 IF unhandled IS NOT NULL THEN
  RAISE EXCEPTION 'purge dependent relation % has rows; retention migration required',unhandled USING ERRCODE='PL409';
 END IF;
 -- Physical removal resets the unreferenced-blob 30-day GC clock. Blobs and
 -- leases are NOT removed by item purge. Pin/serialize hashes in global order.
 PERFORM 1 FROM blobs b WHERE household_id=p_household AND EXISTS(SELECT 1 FROM attachments a WHERE a.household_id=b.household_id AND a.item_id=ANY(ids) AND b.sha256 IN (a.blob_sha256,a.thumbnail_sha256)) ORDER BY household_id,sha256 FOR UPDATE;
 UPDATE blobs b SET last_referenced_at=clock_timestamp() WHERE household_id=p_household AND EXISTS(SELECT 1 FROM attachments a WHERE a.household_id=b.household_id AND a.item_id=ANY(ids) AND b.sha256 IN (a.blob_sha256,a.thumbnail_sha256));
 GET DIAGNOSTICS n = ROW_COUNT;
 counts := counts || jsonb_build_object('blob_references',n);
 FOR relation IN SELECT * FROM pl_purge_relations() ORDER BY ord LOOP
  IF to_regclass('porchlight.'||relation.table_name) IS NOT NULL THEN
   scope := CASE WHEN relation.table_name IN ('notification_sources','notification_event_delivery') THEN 'true' ELSE 'household_id=$2' END;
   IF NOT has_table_privilege(current_user,'porchlight.'||relation.table_name,'SELECT')
      OR NOT has_table_privilege(current_user,'porchlight.'||relation.table_name,'DELETE') THEN
    RAISE EXCEPTION 'purge dependent relation porchlight.% privileges require a retention migration',relation.table_name USING ERRCODE='PL409';
   END IF;
   EXECUTE format('SELECT count(*) FROM porchlight.%I WHERE %s AND (%s)',
    relation.table_name,scope,relation.predicate) INTO n USING ids,p_household;
   counts := counts || jsonb_build_object(relation.table_name,n);
  END IF;
 END LOOP;
 INSERT INTO purge_log(household_id,actor,item_ids,counts)
 VALUES(p_household,pl_current_actor(),ids,counts) RETURNING id INTO log_id;
 -- Remove projections first; child deletion triggers may add revisions/jobs, so
 -- history/jobs are removed LAST, immediately before the parent delete.
 FOR relation IN SELECT * FROM pl_purge_relations() ORDER BY ord LOOP
  IF to_regclass('porchlight.'||relation.table_name) IS NOT NULL THEN
   scope := CASE WHEN relation.table_name IN ('notification_sources','notification_event_delivery') THEN 'true' ELSE 'household_id=$2' END;
   IF relation.table_name='notifications' THEN
    UPDATE notifications SET item_id=NULL,occurrence_id=NULL WHERE household_id=p_household AND (item_id=ANY(ids) OR occurrence_id IN (SELECT id FROM item_occurrences WHERE item_id=ANY(ids)));
    CONTINUE;
   END IF;
   EXECUTE format('DELETE FROM porchlight.%I WHERE %s AND (%s)',
    relation.table_name,scope,relation.predicate) USING ids,p_household;
  END IF;
 END LOOP;
 RETURN jsonb_build_object('id',log_id,'item_ids',ids,'counts',counts);
END $$;
REVOKE ALL ON FUNCTION pl_purge_items(uuid,uuid[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pl_purge_items(uuid,uuid[]) TO porchlight_app;
ALTER FUNCTION pl_purge_items(uuid,uuid[]) OWNER TO porchlight_maintenance;
SET LOCAL ROLE porchlight_owner;
REVOKE CREATE ON SCHEMA porchlight FROM porchlight_maintenance;

RESET search_path;
RESET ROLE;
