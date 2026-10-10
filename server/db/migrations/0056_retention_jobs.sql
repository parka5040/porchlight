SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
-- Atomic with prefix pruning. Held streams must compare this generation in the
-- SAME statement as every scan/queued delivery; a change means resnapshot.
CREATE TABLE stream_retention (
 id boolean PRIMARY KEY DEFAULT true CHECK(id),
 epoch bigint NOT NULL DEFAULT 0 CHECK(epoch>=0),
 pruned_through bigint NOT NULL DEFAULT 0 CHECK(pruned_through>=0)
);
INSERT INTO stream_retention DEFAULT VALUES;
GRANT SELECT ON stream_retention TO porchlight_app,porchlight_agent;
GRANT SELECT,UPDATE ON stream_retention TO porchlight_maintenance;
GRANT SELECT,DELETE ON sessions,enrollment_tokens,password_resets,api_tokens,step_ups,outbox TO porchlight_maintenance;
GRANT SELECT,UPDATE ON stream_assign_mutex TO porchlight_maintenance;
GRANT SELECT ON households TO porchlight_maintenance;
-- Row locks require UPDATE; column grants keep the definer's writes narrow.
GRANT UPDATE(id) ON notification_events,sessions TO porchlight_maintenance;
GRANT USAGE,CREATE ON SCHEMA porchlight TO porchlight_maintenance;
SET LOCAL ROLE porchlight_maintenance;
CREATE FUNCTION pl_retention(p_now timestamptz) RETURNS jsonb
 LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE h uuid; counts jsonb; totals jsonb := '{}'; per_household jsonb := '{}'; r record; n bigint; prefix bigint;
BEGIN
 IF p_now IS NULL THEN RAISE EXCEPTION 'retention clock required' USING ERRCODE='PL422'; END IF;
 FOR h IN SELECT id FROM households ORDER BY id LOOP
  -- Lock deletion parents BEFORE counting/deleting second-order CASCADE rows.
  -- Concurrent FK inserts cannot turn counted explicit removal into silent cascade.
  PERFORM 1 FROM notification_events WHERE household_id=h AND created_at<p_now-interval '90 days' ORDER BY id FOR UPDATE;
  PERFORM 1 FROM notifications WHERE household_id=h AND created_at<p_now-interval '90 days' ORDER BY id FOR UPDATE;
  PERFORM 1 FROM sessions WHERE household_id=h AND expires_at<=p_now ORDER BY id FOR UPDATE;
  counts := '{}';
  FOR r IN SELECT * FROM (VALUES
   (1,'notification_sources','event_id IN (SELECT id FROM porchlight.notification_events WHERE household_id=$1 AND created_at<$2-interval ''90 days'') OR notification_id IN (SELECT id FROM porchlight.notifications WHERE household_id=$1 AND created_at<$2-interval ''90 days'')',false),
   (2,'notification_event_delivery','event_id IN (SELECT id FROM porchlight.notification_events WHERE household_id=$1 AND created_at<$2-interval ''90 days'')',false),
   (3,'notifications','created_at<$2-interval ''90 days''',true),
   (4,'notification_events','created_at<$2-interval ''90 days''',true),
   (5,'step_ups','expires_at<=$2 OR session_id IN (SELECT id FROM porchlight.sessions WHERE household_id=$1 AND expires_at<=$2)',true),
   (6,'sessions','expires_at<=$2',true),
   (7,'enrollment_tokens','expires_at<=$2',true),
   (8,'password_resets','expires_at<=$2',true),
   (9,'api_tokens','expires_at<=$2',true)
  ) plan(ord,table_name,predicate,scoped) ORDER BY ord LOOP
   EXECUTE format('DELETE FROM porchlight.%I WHERE %s AND (%s)',r.table_name,CASE WHEN r.scoped THEN 'household_id=$1' ELSE 'true' END,r.predicate) USING h,p_now,prefix;
   GET DIAGNOSTICS n = ROW_COUNT;
   counts := counts || jsonb_build_object(r.table_name,n);
   totals := totals || jsonb_build_object(r.table_name,coalesce((totals->>r.table_name)::bigint,0)+n);
  END LOOP;
  per_household := per_household || jsonb_build_object(h::text,counts);
 END LOOP;
 -- Only publication/pruning needs this mutex. Event/auth cleanup must not stall
 -- live delivery. No short lock_timeout survives to the eventual NOTIFY commit.
 PERFORM 1 FROM stream_assign_mutex FOR UPDATE;
 SELECT coalesce(min(stream_seq) FILTER (WHERE created_at>=p_now-interval '7 days'),max(stream_seq)) INTO prefix FROM outbox;
 totals := totals || jsonb_build_object('outbox',0);
 FOR h,counts IN SELECT key::uuid,value FROM jsonb_each(per_household) ORDER BY key LOOP
  DELETE FROM outbox WHERE household_id=h AND stream_seq<prefix AND created_at<p_now-interval '7 days';
  GET DIAGNOSTICS n = ROW_COUNT;
  counts := counts || jsonb_build_object('outbox',n);
  totals := totals || jsonb_build_object('outbox',(totals->>'outbox')::bigint+n);
  INSERT INTO purge_log(household_id,actor,item_ids,counts) VALUES(h,pl_current_actor(),'{}',counts);
 END LOOP;
 IF (totals->>'outbox')::bigint>0 THEN
  UPDATE stream_retention SET epoch=epoch+1,pruned_through=greatest(pruned_through,prefix-1);
  PERFORM pg_notify('porchlight_stream','');
 END IF;
 RETURN totals;
END $$;
REVOKE ALL ON FUNCTION pl_retention(timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pl_retention(timestamptz) TO porchlight_app;
SET LOCAL ROLE porchlight_owner;
REVOKE CREATE ON SCHEMA porchlight FROM porchlight_maintenance;
-- Backup lease renewal serializes against GC's blob row locks, never revives an
-- expired lease. A stuck backup fails closed; healthy byte progress renews it.
CREATE FUNCTION pl_renew_blob_lease(p_id uuid,p_duration interval DEFAULT interval '6 hours') RETURNS boolean
 LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
 IF p_duration<=interval '0' OR p_duration>interval '24 hours' THEN RAISE EXCEPTION 'invalid lease duration' USING ERRCODE='PL422'; END IF;
 PERFORM 1 FROM blobs b JOIN blob_lease_items li ON li.household_id=b.household_id AND li.sha256=b.sha256 WHERE li.lease_id=p_id ORDER BY b.household_id,b.sha256 FOR SHARE OF b;
 UPDATE blob_leases SET expires_at=clock_timestamp()+p_duration WHERE id=p_id AND expires_at>clock_timestamp();
 RETURN FOUND;
END $$;
REVOKE ALL ON FUNCTION pl_renew_blob_lease(uuid,interval) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pl_renew_blob_lease(uuid,interval) TO porchlight_app,porchlight_backup;
RESET search_path;
RESET ROLE;
