SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Scheduler tests and jobs freeze the event clock transaction-locally; ordinary writers use now().
CREATE FUNCTION pl_notification_now() RETURNS timestamptz LANGUAGE sql STABLE
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
 SELECT CASE WHEN (session_user='porchlight_app' OR EXISTS(SELECT 1 FROM pg_roles WHERE rolname=session_user AND rolsuper))
 AND nullif(current_setting('porchlight.actor_member_id',true),'') IS NULL
 AND nullif(current_setting('porchlight.agent_label',true),'') IS NULL
 THEN coalesce(nullif(current_setting('porchlight.notification_now',true),'')::timestamptz,now()) ELSE now() END
$$;
ALTER TABLE notification_events ALTER COLUMN created_at SET DEFAULT pl_notification_now();

CREATE TABLE notification_prefs (
 member_id uuid PRIMARY KEY, household_id uuid NOT NULL,
 reminders boolean NOT NULL DEFAULT true, assignments boolean NOT NULL DEFAULT true,
 dependents boolean NOT NULL DEFAULT true, activity boolean NOT NULL DEFAULT true,
 digest boolean NOT NULL DEFAULT true, connection boolean NOT NULL DEFAULT true,
 quiet_start time, quiet_end time,
 default_todo_offsets int[] NOT NULL DEFAULT '{}',
 default_deadline_offsets int[] NOT NULL DEFAULT '{}',
 default_appointment_offsets int[] NOT NULL DEFAULT '{}',
 version int NOT NULL DEFAULT 1, created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now(),
 FOREIGN KEY(household_id,member_id) REFERENCES household_members(household_id,id),
 CHECK ((quiet_start IS NULL) = (quiet_end IS NULL)), CHECK (quiet_start IS NULL OR quiet_start <> quiet_end),
 CHECK (cardinality(default_todo_offsets)<=16 AND cardinality(default_deadline_offsets)<=16 AND cardinality(default_appointment_offsets)<=16),
 CHECK (0 <= ALL(default_todo_offsets) AND 525600 >= ALL(default_todo_offsets) AND array_position(default_todo_offsets,NULL) IS NULL),
 CHECK (0 <= ALL(default_deadline_offsets) AND 525600 >= ALL(default_deadline_offsets) AND array_position(default_deadline_offsets,NULL) IS NULL),
 CHECK (0 <= ALL(default_appointment_offsets) AND 525600 >= ALL(default_appointment_offsets) AND array_position(default_appointment_offsets,NULL) IS NULL)
);
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON notification_prefs FOR EACH ROW EXECUTE FUNCTION pl_version_guard('member_id','household_id');
CREATE FUNCTION pl_notification_prefs_seed() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
 INSERT INTO notification_prefs(member_id,household_id) VALUES(NEW.id,NEW.household_id); RETURN NULL;
END $$;
CREATE TRIGGER notification_prefs_seed AFTER INSERT ON household_members FOR EACH ROW EXECUTE FUNCTION pl_notification_prefs_seed();
INSERT INTO notification_prefs(member_id,household_id) SELECT id,household_id FROM household_members;

CREATE TABLE notifications (
 id uuid PRIMARY KEY DEFAULT uuidv7(), household_id uuid NOT NULL,
 recipient_member_id uuid NOT NULL, kind text NOT NULL,
 category text NOT NULL CHECK(category IN ('reminders','assignments','dependents','activity','digest','connection')),
 item_id uuid, occurrence_id uuid, entity_type text NOT NULL, entity_id uuid NOT NULL,
 actor_member_id uuid, actor_label text, count int NOT NULL DEFAULT 1 CHECK(count>0),
 summary jsonb NOT NULL DEFAULT '{}' CHECK(jsonb_typeof(summary)='object'),
 window_start timestamptz NOT NULL, release_at timestamptz NOT NULL,
 released_at timestamptz, read_at timestamptz,
 version int NOT NULL DEFAULT 1, created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now(),
 FOREIGN KEY(household_id,recipient_member_id) REFERENCES household_members(household_id,id),
 FOREIGN KEY(household_id,item_id) REFERENCES items(household_id,id)
);
CREATE INDEX notifications_inbox ON notifications(recipient_member_id,id DESC);
CREATE INDEX notifications_pending ON notifications(release_at,id) WHERE released_at IS NULL;
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON notifications FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id','household_id','recipient_member_id');
-- Source links retain authoritative ACLs for every item in a coalesced batch.
CREATE TABLE notification_sources (
 notification_id uuid NOT NULL REFERENCES notifications(id) ON DELETE CASCADE,
 event_id uuid NOT NULL UNIQUE REFERENCES notification_events(id) ON DELETE CASCADE,
 PRIMARY KEY(notification_id,event_id)
);
CREATE FUNCTION pl_notifications_outbox() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE event_kind text;
BEGIN
 IF NEW.released_at IS NULL THEN RETURN NULL; END IF;
 IF TG_OP='INSERT' THEN event_kind := 'notification.created';
 ELSIF OLD.read_at IS NULL AND NEW.read_at IS NOT NULL THEN event_kind := 'notification.dismissed';
 ELSIF OLD.released_at IS NULL THEN event_kind := 'notification.created';
 ELSE event_kind := 'notification.updated'; END IF;
 INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
 VALUES(NEW.household_id,event_kind,'notification',NEW.id,jsonb_build_object('member_id',NEW.recipient_member_id,'category',NEW.category));
 PERFORM pg_notify('porchlight_outbox',''); RETURN NULL;
END $$;
CREATE TRIGGER notifications_outbox AFTER INSERT OR UPDATE ON notifications FOR EACH ROW EXECUTE FUNCTION pl_notifications_outbox();
CREATE FUNCTION pl_notification_prefs_changed() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
 INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
 VALUES(NEW.household_id,'settings.changed','notification_prefs',NEW.member_id,jsonb_build_object('member_id',NEW.member_id,'section','notification_prefs'));
 PERFORM pg_notify('porchlight_outbox','');
 IF TG_OP='UPDATE' AND (NEW.default_todo_offsets,NEW.default_deadline_offsets,NEW.default_appointment_offsets)
 IS DISTINCT FROM (OLD.default_todo_offsets,OLD.default_deadline_offsets,OLD.default_appointment_offsets) THEN
  -- Item/job locks must already be held by the API's enqueue-only scope. Raw SQL follows the same documented order.
  PERFORM pl_enqueue_derivation(i.household_id,i.id) FROM items i WHERE i.deleted_at IS NULL
   AND (i.owner_id=NEW.member_id OR i.subject_member_id=NEW.member_id OR EXISTS(SELECT 1 FROM item_assignees a WHERE a.item_id=i.id AND a.member_id=NEW.member_id))
   AND NOT EXISTS(SELECT 1 FROM item_reminders r WHERE r.item_id=i.id) ORDER BY i.id;
 END IF;
 RETURN NULL;
END $$;
CREATE TRIGGER notification_prefs_changed AFTER INSERT OR UPDATE ON notification_prefs FOR EACH ROW EXECUTE FUNCTION pl_notification_prefs_changed();

CREATE FUNCTION pl_task_notification() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE recipient uuid; event_kind text;
BEGIN
 IF NEW.kind='assigned' THEN recipient := (NEW.data->>'member_id')::uuid; event_kind := 'assigned';
 ELSIF NEW.kind IN ('status_changed','completed','reopened') THEN
  SELECT owner_id INTO recipient FROM items WHERE id=NEW.item_id;
  event_kind := CASE WHEN NEW.kind='reopened' THEN 'status_changed' ELSE NEW.kind END;
 ELSE RETURN NULL; END IF;
 IF EXISTS(SELECT 1 FROM household_members WHERE id=recipient AND deleted_at IS NULL AND role<>'dependent') THEN
  PERFORM pl_emit_notification_event(NEW.household_id,recipient,event_kind,'item',NEW.item_id,'{}',NEW.item_id,NEW.occurrence_id,'task:'||NEW.id::text);
 END IF;
 RETURN NULL;
END $$;
CREATE TRIGGER task_notification AFTER INSERT ON task_events FOR EACH ROW EXECUTE FUNCTION pl_task_notification();
CREATE FUNCTION pl_terminal_notification() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE m uuid;
BEGIN
 IF NEW.attempts>=5 AND OLD.attempts<5 THEN
  FOR m IN SELECT id FROM household_members WHERE household_id=NEW.household_id AND role='super_admin' AND deleted_at IS NULL ORDER BY id LOOP
   PERFORM pl_emit_notification_event(NEW.household_id,m,'derivation_error','derivation',uuidv7(),'{}',NULL,NULL,'derivation:'||NEW.item_id::text||':'||NEW.requested_gen::text||':'||m::text);
  END LOOP;
 END IF;
 RETURN NULL;
END $$;
CREATE TRIGGER terminal_notification AFTER UPDATE OF attempts ON item_derivation FOR EACH ROW EXECUTE FUNCTION pl_terminal_notification();

-- Default reminders use stable synthetic UUIDs, not authoritative item_reminders rows.
ALTER TABLE scheduled_reminders DROP CONSTRAINT scheduled_reminders_household_id_reminder_id_fkey;
CREATE FUNCTION pl_reminder_offsets(p_item uuid,p_member uuid) RETURNS TABLE(id uuid,offset_minutes int)
 LANGUAGE sql STABLE SET search_path = pg_catalog, porchlight, pg_temp AS $$
 SELECT r.id,r.offset_minutes FROM item_reminders r WHERE r.item_id=p_item
 UNION ALL
 SELECT md5(p_item::text||':'||p_member::text||':'||v::text)::uuid,v FROM items i
 JOIN notification_prefs p ON p.member_id=p_member AND p.household_id=i.household_id
 CROSS JOIN LATERAL unnest(CASE i.kind WHEN 'todo' THEN p.default_todo_offsets WHEN 'deadline' THEN p.default_deadline_offsets ELSE p.default_appointment_offsets END) v
 WHERE i.id=p_item AND NOT EXISTS(SELECT 1 FROM item_reminders r WHERE r.item_id=p_item)
$$;
REVOKE ALL ON FUNCTION pl_reminder_offsets(uuid,uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pl_reminder_offsets(uuid,uuid) TO porchlight_app,porchlight_agent;
GRANT SELECT,INSERT,UPDATE ON notifications TO porchlight_app;
GRANT SELECT,INSERT ON notification_sources TO porchlight_app;
GRANT SELECT,UPDATE ON notification_prefs TO porchlight_app;
GRANT SELECT ON notifications,notification_sources,notification_prefs TO porchlight_agent;
RESET search_path;
RESET ROLE;
