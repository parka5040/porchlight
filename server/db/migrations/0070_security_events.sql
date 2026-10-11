SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Security history is deliberately separate from the device-visible business outbox.
-- Only fixed operation names, identifiers and transaction attribution are accepted; no request payloads.
CREATE TABLE security_events (
 id uuid PRIMARY KEY DEFAULT uuidv7(),
 household_id uuid NOT NULL REFERENCES households(id),
 occurred_at timestamptz NOT NULL DEFAULT clock_timestamp(),
 kind text NOT NULL CHECK (kind IN ('login_failed','login_succeeded','password_changed',
  'password_reset_issued','password_reset_redeemed','session_revoked','device_enrolled',
  'device_revoked','certificate_renewed','token_created','token_revoked','admin_token_recovered',
  'role_changed','member_removed')),
 actor jsonb NOT NULL DEFAULT pl_current_actor(),
 target_type text NOT NULL CHECK (target_type IN ('member','device','token')),
 target_id uuid NOT NULL,
 outcome text NOT NULL CHECK (outcome IN ('succeeded','failed'))
);
-- Actor and target IDs intentionally have no identity FKs: retention must not erase attribution.
CREATE INDEX security_events_household_time ON security_events(household_id,occurred_at,id);
CREATE INDEX security_events_household_kind_time ON security_events(household_id,kind,occurred_at,id);
CREATE FUNCTION pl_security_events_immutable() RETURNS trigger
 LANGUAGE plpgsql SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
 IF TG_OP <> 'DELETE' OR current_user <> 'porchlight_maintenance' THEN
  RAISE EXCEPTION 'security events are append-only' USING ERRCODE='PL403';
 END IF;
 RETURN NULL;
END $$;
CREATE TRIGGER append_only BEFORE UPDATE OR DELETE OR TRUNCATE ON security_events
 FOR EACH STATEMENT EXECUTE FUNCTION pl_security_events_immutable();
GRANT SELECT ON security_events TO porchlight_app;
GRANT INSERT(household_id,kind,target_type,target_id,outcome) ON security_events TO porchlight_app;
GRANT SELECT,DELETE ON security_events TO porchlight_maintenance;

-- Shared state-changing helpers (including member-removal cascades and local PAT recovery)
-- capture these events inside their caller's transaction, once per actual state transition.
CREATE FUNCTION pl_security_state_change() RETURNS trigger
 LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE h uuid := NEW.household_id; target uuid; target_type text; kind text;
BEGIN
 CASE TG_TABLE_NAME
 WHEN 'devices' THEN
  target := NEW.id; target_type := 'device';
  kind := CASE WHEN TG_OP='INSERT' THEN 'device_enrolled' ELSE 'device_revoked' END;
 WHEN 'sessions' THEN
  target := NEW.device_id; target_type := 'device'; kind := 'session_revoked';
 WHEN 'api_tokens' THEN
  target := NEW.id; target_type := 'token';
  kind := CASE WHEN TG_OP='INSERT' THEN 'token_created' ELSE 'token_revoked' END;
 WHEN 'password_resets' THEN
  SELECT id INTO target FROM household_members WHERE household_id=h AND user_id=NEW.user_id;
  target_type := 'member'; kind := 'password_reset_issued';
 WHEN 'household_members' THEN
  target := NEW.id; target_type := 'member'; kind := TG_ARGV[0];
 END CASE;
 INSERT INTO security_events(household_id,kind,target_type,target_id,outcome)
 VALUES(h,kind,target_type,target,'succeeded');
 RETURN NULL;
END $$;
CREATE TRIGGER security_enrolled AFTER INSERT ON devices FOR EACH ROW EXECUTE FUNCTION pl_security_state_change();
CREATE TRIGGER security_revoked AFTER UPDATE OF revoked_at ON devices FOR EACH ROW
 WHEN (OLD.revoked_at IS NULL AND NEW.revoked_at IS NOT NULL) EXECUTE FUNCTION pl_security_state_change();
CREATE TRIGGER security_revoked AFTER UPDATE OF revoked_at ON sessions FOR EACH ROW
 WHEN (OLD.revoked_at IS NULL AND NEW.revoked_at IS NOT NULL) EXECUTE FUNCTION pl_security_state_change();
CREATE TRIGGER security_created AFTER INSERT ON api_tokens FOR EACH ROW EXECUTE FUNCTION pl_security_state_change();
CREATE TRIGGER security_revoked AFTER UPDATE OF revoked_at ON api_tokens FOR EACH ROW
 WHEN (OLD.revoked_at IS NULL AND NEW.revoked_at IS NOT NULL) EXECUTE FUNCTION pl_security_state_change();
CREATE TRIGGER security_reset AFTER INSERT ON password_resets FOR EACH ROW EXECUTE FUNCTION pl_security_state_change();
CREATE TRIGGER security_role AFTER UPDATE OF role ON household_members FOR EACH ROW
 WHEN (OLD.role IS DISTINCT FROM NEW.role) EXECUTE FUNCTION pl_security_state_change('role_changed');
CREATE TRIGGER security_removed AFTER UPDATE OF deleted_at ON household_members FOR EACH ROW
 WHEN (OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL) EXECUTE FUNCTION pl_security_state_change('member_removed');

-- Issuance timestamps must reflect insertion under the device lock, not transaction start order.
ALTER TABLE device_certificates ALTER COLUMN created_at SET DEFAULT clock_timestamp();
CREATE INDEX device_certificates_issuance ON device_certificates(device_id,created_at,id);

GRANT USAGE,CREATE ON SCHEMA porchlight TO porchlight_maintenance;
SET LOCAL ROLE porchlight_maintenance;
CREATE OR REPLACE FUNCTION pl_retention(p_now timestamptz) RETURNS jsonb
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
   (9,'api_tokens','expires_at<=$2',true),
   (10,'security_events','occurred_at<$2-interval ''1 year''',true)
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
SET LOCAL ROLE porchlight_owner;
REVOKE CREATE ON SCHEMA porchlight FROM porchlight_maintenance;
RESET search_path;
RESET ROLE;
