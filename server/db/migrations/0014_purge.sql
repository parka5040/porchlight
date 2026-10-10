SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- No item FK: this audit survives the physical deletion it records. Neither app,
-- agent nor maintenance may rewrite it. Only the maintenance definer inserts.
CREATE TABLE purge_log (
 id uuid PRIMARY KEY DEFAULT uuidv7(),
 household_id uuid NOT NULL REFERENCES households(id),
 actor jsonb NOT NULL,
 item_ids uuid[] NOT NULL,
 counts jsonb NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE FUNCTION pl_purge_log_immutable() RETURNS trigger LANGUAGE plpgsql
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN RAISE EXCEPTION 'purge_log is append-only' USING ERRCODE='PL403'; END $$;
CREATE TRIGGER append_only BEFORE UPDATE OR DELETE OR TRUNCATE ON purge_log
 FOR EACH STATEMENT EXECUTE FUNCTION pl_purge_log_immutable();
GRANT SELECT ON purge_log TO porchlight_app, porchlight_agent;
GRANT USAGE, CREATE ON SCHEMA porchlight TO porchlight_maintenance;
GRANT SELECT, INSERT ON purge_log TO porchlight_maintenance;
GRANT SELECT, UPDATE, DELETE ON items TO porchlight_maintenance;
GRANT SELECT, DELETE ON item_assignees,item_viewers,item_reminders,item_occurrences,
 member_agenda,scheduled_reminders,item_derivation,item_revisions,task_events TO porchlight_maintenance;
GRANT UPDATE ON item_occurrences,item_derivation TO porchlight_maintenance;
GRANT EXECUTE ON FUNCTION pl_current_actor() TO porchlight_maintenance;

-- Member lifecycle writers include raw SQL. Zone events retain their existing trigger.
CREATE TRIGGER member_lifecycle_insert AFTER INSERT ON household_members
 FOR EACH ROW EXECUTE FUNCTION pl_row_outbox('member.changed','member');
CREATE TRIGGER member_lifecycle_update AFTER UPDATE OF display_name,role,color,deleted_at ON household_members
 FOR EACH ROW WHEN ((OLD.display_name,OLD.role,OLD.color,OLD.deleted_at)
  IS DISTINCT FROM (NEW.display_name,NEW.role,NEW.color,NEW.deleted_at))
 EXECUTE FUNCTION pl_row_outbox('member.changed','member');

-- One retention plan drives locks, catalog guard, counts and explicit deletes.
-- Predicates select exactly the rows removed, including multi-hop list children.
-- Extension names are resolved at execution (their migrations follow 0014).
CREATE FUNCTION pl_purge_relations() RETURNS TABLE(ord integer, table_name text, predicate text)
 LANGUAGE sql IMMUTABLE SET search_path = pg_catalog, porchlight, pg_temp AS $$
 VALUES
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
 (18,'notification_events','item_id=ANY($1)'),
 (19,'items','id=ANY($1)')
$$;
REVOKE ALL ON FUNCTION pl_purge_relations() FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pl_purge_relations() TO porchlight_maintenance;

-- R56b: owner-owned catalog probe covers the FK closure, not only immediate
-- item/occurrence dependents. No runtime grants or DDL, including for new tables.
-- It locks selected rows in EVERY relation the plan deletes before probing, so
-- concurrent FK inserts cannot race the guard (even into history/notification
-- rows). Locks do not write domain data or invoke update/audit triggers.
CREATE FUNCTION pl_purge_unhandled_relation(p_household uuid, p_ids uuid[]) RETURNS text
 LANGUAGE plpgsql SECURITY DEFINER
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE dep record; relation record; join_predicate text; has_rows boolean; order_by text; extra text;
BEGIN
 FOR relation IN SELECT * FROM pl_purge_relations()
  WHERE to_regclass('porchlight.'||table_name) IS NOT NULL
  ORDER BY CASE table_name WHEN 'items' THEN -3 WHEN 'item_occurrences' THEN -2
   WHEN 'item_derivation' THEN -1 ELSE ord END
 LOOP
  SELECT string_agg(format('%I',a.attname),',' ORDER BY k.n) INTO order_by
   FROM pg_index i CROSS JOIN LATERAL unnest(i.indkey) WITH ORDINALITY k(attnum,n)
   JOIN pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=k.attnum
   WHERE i.indrelid=to_regclass('porchlight.'||relation.table_name) AND i.indisprimary;
  EXECUTE format('SELECT 1 FROM porchlight.%I WHERE household_id=$2 AND (%s) ORDER BY %s FOR UPDATE',
   relation.table_name,relation.predicate,coalesce(order_by,'ctid')) USING p_ids,p_household;
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
     ('ics_uid_map','item_id','items'),('schedule_suggestions','item_id','items')
    ) AS handled(table_name,column_name,parent_name)
    WHERE ns.nspname='porchlight' AND r.relname=handled.table_name
     AND c.confrelid=to_regclass('porchlight.'||handled.parent_name)
     AND (SELECT array_agg(a.attname::text ORDER BY k.ordinality)
          FROM unnest(c.conkey) WITH ORDINALITY k(attnum,ordinality)
          JOIN pg_attribute a ON a.attrelid=c.conrelid AND a.attnum=k.attnum)
         =ARRAY['household_id',handled.column_name]
     AND (SELECT array_agg(a.attname::text ORDER BY k.ordinality)
          FROM unnest(c.confkey) WITH ORDINALITY k(attnum,ordinality)
          JOIN pg_attribute a ON a.attrelid=c.confrelid AND a.attnum=k.attnum)
         =ARRAY['household_id','id']
   ) AS handled
  FROM pg_constraint c JOIN closure ON closure.relid=c.confrelid
  JOIN pg_class r ON r.oid=c.conrelid JOIN pg_namespace ns ON ns.oid=r.relnamespace
  JOIN pg_class pr ON pr.oid=c.confrelid
  JOIN pl_purge_relations() plan ON c.confrelid=to_regclass('porchlight.'||plan.table_name)
  LEFT JOIN pl_purge_relations() child_plan ON c.conrelid=to_regclass('porchlight.'||child_plan.table_name)
  WHERE c.contype='f'
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
   extra := format(' AND NOT EXISTS (SELECT 1 FROM porchlight.%I selected WHERE selected.tableoid=d.tableoid AND selected.ctid=d.ctid AND selected.household_id=$2 AND (%s))',
    dep.relname,dep.child_predicate);
  END IF;
  EXECUTE format('SELECT EXISTS(SELECT 1 FROM %I.%I d JOIN (SELECT * FROM porchlight.%I WHERE household_id=$2 AND (%s)) p ON %s WHERE true%s)',
   dep.nspname,dep.relname,dep.parent_name,dep.parent_predicate,join_predicate,extra)
   INTO has_rows USING p_ids,p_household;
  IF has_rows THEN RETURN format('%I.%I',dep.nspname,dep.relname); END IF;
 END LOOP;
 RETURN NULL;
END $$;
REVOKE ALL ON FUNCTION pl_purge_unhandled_relation(uuid,uuid[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pl_purge_unhandled_relation(uuid,uuid[]) TO porchlight_maintenance;

CREATE FUNCTION pl_purge_items(p_household uuid, p_ids uuid[]) RETURNS jsonb
 LANGUAGE plpgsql SECURITY DEFINER
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE ids uuid[]; counts jsonb := '{}'; n bigint; relation record; log_id uuid; unhandled text;
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
 FOR relation IN SELECT * FROM pl_purge_relations() ORDER BY ord LOOP
  IF to_regclass('porchlight.'||relation.table_name) IS NOT NULL THEN
   IF NOT has_table_privilege(current_user,'porchlight.'||relation.table_name,'SELECT')
      OR NOT has_table_privilege(current_user,'porchlight.'||relation.table_name,'DELETE') THEN
    RAISE EXCEPTION 'purge dependent relation porchlight.% privileges require a retention migration',relation.table_name USING ERRCODE='PL409';
   END IF;
   EXECUTE format('SELECT count(*) FROM porchlight.%I WHERE household_id=$2 AND (%s)',
    relation.table_name,relation.predicate) INTO n USING ids,p_household;
   counts := counts || jsonb_build_object(relation.table_name,n);
  END IF;
 END LOOP;
 INSERT INTO purge_log(household_id,actor,item_ids,counts)
 VALUES(p_household,pl_current_actor(),ids,counts) RETURNING id INTO log_id;
 -- Remove projections first; child deletion triggers may add revisions/jobs, so
 -- history/jobs are removed LAST, immediately before the parent delete.
 FOR relation IN SELECT * FROM pl_purge_relations() ORDER BY ord LOOP
  IF to_regclass('porchlight.'||relation.table_name) IS NOT NULL THEN
   EXECUTE format('DELETE FROM porchlight.%I WHERE household_id=$2 AND (%s)',
    relation.table_name,relation.predicate) USING ids,p_household;
  END IF;
 END LOOP;
 RETURN jsonb_build_object('id',log_id,'item_ids',ids,'counts',counts);
END $$;
REVOKE ALL ON FUNCTION pl_purge_items(uuid,uuid[]) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pl_purge_items(uuid,uuid[]) TO porchlight_app;
ALTER FUNCTION pl_purge_items(uuid,uuid[]) OWNER TO porchlight_maintenance;
REVOKE CREATE ON SCHEMA porchlight FROM porchlight_maintenance;
RESET search_path;
RESET ROLE;
