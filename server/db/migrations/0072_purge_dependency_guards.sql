-- BL-server: explicit retention-plan inbound edges, unreadable-child fail-closed mapping,
-- and stage-aware checks against trigger-produced dependencies. Released migrations stay immutable.
SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
GRANT USAGE, CREATE ON SCHEMA porchlight TO porchlight_maintenance;

-- The app's locked WriteScope and trusted migration/owner operations may identify
-- respread bystanders. SQL agents cannot suppress move history with a custom GUC.
-- Keep INVOKER: the raw-agent role cannot assume the app/owner roles.
CREATE OR REPLACE FUNCTION pl_record_board_move() RETURNS trigger LANGUAGE plpgsql
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
 IF current_user <> 'porchlight_agent'
    AND nullif(current_setting('porchlight.board_mover',true),'') IS NOT NULL
    AND current_setting('porchlight.board_mover',true) <> NEW.id::text THEN
  NEW.board_moved := OLD.board_moved;
 ELSE
  NEW.board_moved := true;
 END IF;
 RETURN NEW;
END;
$$;

-- A meaningful tie-respread may retain the mover's own key while changing its
-- marker/version. Publish that change too; unchanged complete plans are no-ops.
DROP TRIGGER occurrence_outbox ON item_occurrences;
CREATE TRIGGER occurrence_outbox AFTER UPDATE ON item_occurrences
 FOR EACH ROW WHEN (
  (OLD.status, OLD.board_position, OLD.board_moved, OLD.completed_by, OLD.completed_at,
   OLD.override_local_start, OLD.override_local_end, OLD.override_local_due, OLD.override_due_date)
  IS DISTINCT FROM
  (NEW.status, NEW.board_position, NEW.board_moved, NEW.completed_by, NEW.completed_at,
   NEW.override_local_start, NEW.override_local_end, NEW.override_local_due, NEW.override_due_date))
 EXECUTE FUNCTION pl_row_outbox('occurrence.changed', 'occurrence');

CREATE OR REPLACE FUNCTION pl_purge_dependency(p_household uuid, p_ids uuid[], p_before text) RETURNS text
 LANGUAGE plpgsql SECURITY DEFINER
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE dep record; relation record; join_predicate text; has_rows boolean; order_by text; extra text; scope text;
BEGIN
 IF p_before IS NULL THEN
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
 END IF;
 FOR dep IN
  -- Inspect direct inbound FKs of every explicitly selected deletion parent.
  -- No recursive expansion: unknown dependent rows fail closed, never extend the plan.
  SELECT c.conrelid,c.confrelid,c.conkey,c.confkey,r.relname,ns.nspname,
   pr.relname AS parent_name,plan.predicate AS parent_predicate,
   child_plan.predicate AS child_predicate,child_plan.ord AS child_ord,plan.ord AS parent_ord,
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
  FROM pg_constraint c
  JOIN pg_class r ON r.oid=c.conrelid JOIN pg_namespace ns ON ns.oid=r.relnamespace
  JOIN pg_class pr ON pr.oid=c.confrelid
  JOIN pl_purge_relations() plan ON c.confrelid=to_regclass('porchlight.'||plan.table_name)
  LEFT JOIN pl_purge_relations() child_plan ON c.conrelid=to_regclass('porchlight.'||child_plan.table_name)
  WHERE c.contype='f' AND plan.table_name <> 'notifications'
   AND (p_before IS NULL OR plan.table_name=p_before)
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
  IF dep.handled AND dep.child_predicate IS NOT NULL
   AND (p_before IS NULL OR dep.child_ord>=dep.parent_ord) THEN
   extra := format(' AND NOT EXISTS (SELECT 1 FROM porchlight.%I selected WHERE selected.tableoid=d.tableoid AND selected.ctid=d.ctid AND %s AND (%s))',
    dep.relname,CASE WHEN dep.relname IN ('notification_sources','notification_event_delivery') THEN 'true' ELSE 'selected.household_id=$2' END,dep.child_predicate);
  END IF;
  BEGIN
  EXECUTE format('SELECT EXISTS(SELECT 1 FROM %I.%I d JOIN (SELECT * FROM porchlight.%I WHERE %s AND (%s)) p ON %s WHERE true%s)',
   dep.nspname,dep.relname,dep.parent_name,CASE WHEN dep.parent_name IN ('notification_sources','notification_event_delivery') THEN 'true' ELSE 'household_id=$2' END,dep.parent_predicate,join_predicate,extra)
   INTO has_rows USING p_ids,p_household;
  EXCEPTION WHEN insufficient_privilege THEN
   -- An unreadable child cannot be proved empty: report the same fail-closed dependency.
   RETURN format('%I.%I',dep.nspname,dep.relname);
  END;
  IF has_rows THEN RETURN format('%I.%I',dep.nspname,dep.relname); END IF;
 END LOOP;
 RETURN NULL;
END $$;
REVOKE ALL ON FUNCTION pl_purge_dependency(uuid,uuid[],text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pl_purge_dependency(uuid,uuid[],text) TO porchlight_maintenance;

CREATE OR REPLACE FUNCTION pl_purge_unhandled_relation(p_household uuid, p_ids uuid[]) RETURNS text
 LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
 SELECT pl_purge_dependency(p_household,p_ids,NULL)
$$;
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
 -- Lock every planned parent before catalog/row probes; the initial check precedes audit.
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
   -- Earlier deletion triggers may have created dependents. Children whose planned
   -- deletion already ran must now be absent, even when their FK shape is known.
   unhandled := pl_purge_dependency(p_household,ids,relation.table_name);
   IF unhandled IS NOT NULL THEN
    RAISE EXCEPTION 'purge dependent relation % has rows; retention migration required',unhandled USING ERRCODE='PL409';
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
