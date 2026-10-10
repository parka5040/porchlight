SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

CREATE TABLE lists (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL REFERENCES households(id),
    item_id uuid,
    owner_id uuid NOT NULL,
    name text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 300),
    mode text NOT NULL DEFAULT 'persistent' CHECK (mode IN ('persistent','per_occurrence')),
    visibility text NOT NULL DEFAULT 'household' CHECK (visibility IN ('household','private','selected')),
    version int NOT NULL DEFAULT 1,
    deleted_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(household_id,id),
    CHECK (mode <> 'per_occurrence' OR item_id IS NOT NULL),
    CHECK (item_id IS NULL OR visibility='household'),
    FOREIGN KEY(household_id,item_id) REFERENCES items(household_id,id),
    FOREIGN KEY(household_id,owner_id) REFERENCES household_members(household_id,id)
);
CREATE INDEX lists_item ON lists(item_id) WHERE item_id IS NOT NULL;
CREATE INDEX lists_live ON lists(household_id,id) WHERE deleted_at IS NULL;
CREATE TABLE list_viewers (
    household_id uuid NOT NULL,
    list_id uuid NOT NULL,
    member_id uuid NOT NULL,
    PRIMARY KEY(list_id,member_id),
    FOREIGN KEY(household_id,list_id) REFERENCES lists(household_id,id),
    FOREIGN KEY(household_id,member_id) REFERENCES household_members(household_id,id)
);
CREATE TABLE list_entries (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL,
    list_id uuid NOT NULL,
    label text NOT NULL CHECK (length(btrim(label)) BETWEEN 1 AND 1000),
    qty double precision CHECK (qty > 0 AND qty < 'Infinity'::float8),
    unit text NOT NULL DEFAULT '' CHECK (length(unit) <= 100),
    url text CHECK (length(url) <= 2048 AND url ~ '^https?://[^[:space:]]+$'),
    notes text NOT NULL DEFAULT '' CHECK (length(notes) <= 10000),
    position text COLLATE "C" NOT NULL DEFAULT '80' CHECK (length(position) BETWEEN 2 AND 64 AND position ~ '^([0-9a-f]{2})*([0-9a-f][1-9a-f]|[1-9a-f][0-9a-f])$'),
    archived_at timestamptz,
    version int NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(household_id,id),
    FOREIGN KEY(household_id,list_id) REFERENCES lists(household_id,id)
);
CREATE INDEX list_entries_order ON list_entries(list_id,position COLLATE "C",id) WHERE archived_at IS NULL;
CREATE TABLE list_entry_checks (
    household_id uuid NOT NULL,
    entry_id uuid NOT NULL,
    occurrence_id uuid,
    checked_by uuid NOT NULL,
    checked_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE NULLS NOT DISTINCT(entry_id,occurrence_id),
    FOREIGN KEY(household_id,entry_id) REFERENCES list_entries(household_id,id),
    FOREIGN KEY(household_id,checked_by) REFERENCES household_members(household_id,id)
);
-- No occurrence FK: regeneration may remove open occurrences; historic checks/revisions survive.
CREATE TABLE list_revisions (
    id uuid PRIMARY KEY DEFAULT uuidv7(), household_id uuid NOT NULL, list_id uuid NOT NULL,
    rev int NOT NULL, snapshot jsonb NOT NULL, actor jsonb NOT NULL, created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(list_id,rev), FOREIGN KEY(household_id,list_id) REFERENCES lists(household_id,id)
);
CREATE TABLE list_entry_revisions (
    id uuid PRIMARY KEY DEFAULT uuidv7(), household_id uuid NOT NULL, entry_id uuid NOT NULL,
    rev int NOT NULL, snapshot jsonb NOT NULL, actor jsonb NOT NULL, created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE(entry_id,rev), FOREIGN KEY(household_id,entry_id) REFERENCES list_entries(household_id,id)
);
CREATE FUNCTION pl_list_metadata_guard() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
 IF (TG_OP='INSERT' OR NEW.owner_id IS DISTINCT FROM OLD.owner_id) AND EXISTS(SELECT 1 FROM household_members WHERE id=NEW.owner_id AND (role='dependent' OR deleted_at IS NOT NULL)) THEN
   RAISE EXCEPTION 'invalid owner' USING ERRCODE='PL422'; END IF;
 IF TG_OP='INSERT' AND NEW.mode='per_occurrence' AND NOT EXISTS(SELECT 1 FROM items WHERE id=NEW.item_id AND (rrule IS NOT NULL OR recurrence_mode='completion')) THEN
   RAISE EXCEPTION 'per-occurrence lists require recurring item' USING ERRCODE='PL422'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER metadata_guard BEFORE INSERT OR UPDATE ON lists FOR EACH ROW EXECUTE FUNCTION pl_list_metadata_guard();
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON lists FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id','household_id','item_id','mode');
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON list_entries FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id','household_id','list_id');
CREATE TRIGGER immutable_row BEFORE UPDATE ON list_viewers FOR EACH STATEMENT EXECUTE FUNCTION pl_immutable_row();
CREATE TRIGGER immutable_row BEFORE UPDATE ON list_entry_checks FOR EACH STATEMENT EXECUTE FUNCTION pl_immutable_row();
CREATE TRIGGER append_only BEFORE UPDATE OR DELETE ON list_revisions FOR EACH STATEMENT EXECUTE FUNCTION pl_append_only();
CREATE TRIGGER append_only BEFORE UPDATE OR DELETE ON list_entry_revisions FOR EACH STATEMENT EXECUTE FUNCTION pl_append_only();

-- Standalone scope authority; attached scope delegates to the shared item ACL.
CREATE OR REPLACE FUNCTION pl_list_can_read(p_list uuid,p_member uuid) RETURNS boolean
LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
 SELECT EXISTS (
 SELECT 1 FROM lists l JOIN household_members m ON m.id=p_member AND m.household_id=l.household_id
 WHERE l.id=p_list AND l.deleted_at IS NULL AND m.deleted_at IS NULL AND m.role<>'dependent'
 AND CASE WHEN l.item_id IS NULL THEN
   l.owner_id=m.id OR l.visibility='household' OR (l.visibility='selected' AND EXISTS(SELECT 1 FROM list_viewers v WHERE v.list_id=l.id AND v.member_id=m.id))
 ELSE pl_item_can_read(l.item_id,m.id) END)
$$;
GRANT EXECUTE ON FUNCTION pl_list_can_read(uuid,uuid) TO porchlight_app,porchlight_agent;

CREATE FUNCTION pl_list_record() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE scope jsonb; recipients jsonb;
BEGIN
 scope := CASE WHEN NEW.item_id IS NULL THEN jsonb_build_object('list_id',NEW.id) ELSE jsonb_build_object('item_id',NEW.item_id,'list_id',NEW.id) END;
 INSERT INTO list_revisions(household_id,list_id,rev,snapshot,actor)
 VALUES(NEW.household_id,NEW.id,NEW.version,(to_jsonb(NEW)-'search_document')||jsonb_build_object('viewers',coalesce((SELECT jsonb_agg(member_id ORDER BY member_id) FROM list_viewers WHERE list_id=NEW.id),'[]')),pl_current_actor());
 IF TG_OP='UPDATE' AND NEW.item_id IS NULL AND (OLD.owner_id,OLD.visibility,OLD.deleted_at) IS DISTINCT FROM (NEW.owner_id,NEW.visibility,NEW.deleted_at) THEN
   SELECT coalesce(jsonb_agg(m.id ORDER BY m.id),'[]') INTO recipients FROM household_members m
   WHERE m.household_id=OLD.household_id AND m.deleted_at IS NULL AND m.role<>'dependent'
   AND (m.id=OLD.owner_id OR OLD.visibility='household' OR (OLD.visibility='selected' AND EXISTS(SELECT 1 FROM list_viewers v WHERE v.list_id=OLD.id AND v.member_id=m.id)));
   scope := scope || jsonb_build_object('eviction',true,'recipients',recipients);
 END IF;
 INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload) VALUES(NEW.household_id,'list.changed','list',NEW.id,scope);
 PERFORM pg_notify('porchlight_outbox',''); RETURN NULL;
END $$;
CREATE TRIGGER list_record AFTER INSERT OR UPDATE ON lists FOR EACH ROW EXECUTE FUNCTION pl_list_record();

CREATE FUNCTION pl_list_entry_record() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE l lists;
BEGIN
 SELECT * INTO l FROM lists WHERE id=NEW.list_id;
 INSERT INTO list_entry_revisions(household_id,entry_id,rev,snapshot,actor)
 VALUES(NEW.household_id,NEW.id,NEW.version,(to_jsonb(NEW)-'search_document')||jsonb_build_object('checks',coalesce((SELECT jsonb_agg(to_jsonb(c) ORDER BY c.occurrence_id NULLS FIRST) FROM list_entry_checks c WHERE c.entry_id=NEW.id),'[]')),pl_current_actor());
 INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
 VALUES(NEW.household_id,'list_entry.changed','list_entry',NEW.id,CASE WHEN l.item_id IS NULL THEN jsonb_build_object('list_id',l.id) ELSE jsonb_build_object('item_id',l.item_id,'list_id',l.id) END);

 PERFORM pg_notify('porchlight_outbox',''); RETURN NULL;
END $$;
CREATE TRIGGER entry_record AFTER INSERT OR UPDATE ON list_entries FOR EACH ROW EXECUTE FUNCTION pl_list_entry_record();

CREATE FUNCTION pl_list_viewer_guard() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
 -- A raw relationship DELETE already owns a child lock: never wait for its parent.
 -- HTTP writers own the parent first; raw bulk writers acquire sorted parents themselves or retry 55P03.
 PERFORM 1 FROM lists WHERE id=CASE WHEN TG_OP='DELETE' THEN OLD.list_id ELSE NEW.list_id END FOR NO KEY UPDATE NOWAIT;
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;
 IF EXISTS(SELECT 1 FROM household_members WHERE id=NEW.member_id AND (role='dependent' OR deleted_at IS NOT NULL)) THEN
 RAISE EXCEPTION 'invalid viewer' USING ERRCODE='PL422'; END IF;
 IF EXISTS(SELECT 1 FROM lists WHERE id=NEW.list_id AND item_id IS NOT NULL) THEN
 RAISE EXCEPTION 'attached list inherits viewers' USING ERRCODE='PL422'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER viewer_guard BEFORE INSERT OR DELETE ON list_viewers FOR EACH ROW EXECUTE FUNCTION pl_list_viewer_guard();
CREATE FUNCTION pl_list_viewer_record() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE r record;
BEGIN
 FOR r IN SELECT DISTINCT list_id FROM changed_rows ORDER BY list_id LOOP
   PERFORM 1 FROM lists WHERE id=r.list_id FOR NO KEY UPDATE;
   UPDATE lists SET updated_at=now() WHERE id=r.list_id;
 END LOOP;
 IF TG_OP='DELETE' THEN
   INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
   SELECT v.household_id,'list.changed','list',v.list_id,jsonb_build_object('list_id',v.list_id,'eviction',true,'recipients',jsonb_build_array(v.member_id))
   FROM changed_rows v JOIN lists l ON l.id=v.list_id WHERE l.visibility='selected';
   PERFORM pg_notify('porchlight_outbox','');
 END IF;
 RETURN NULL;
END $$;
CREATE TRIGGER viewer_record_insert AFTER INSERT ON list_viewers REFERENCING NEW TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_list_viewer_record();
CREATE TRIGGER viewer_record_delete AFTER DELETE ON list_viewers REFERENCING OLD TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_list_viewer_record();

-- A raw DELETE owns its check row before the AFTER-statement revision bump.
-- Never wait for an entry held by an HTTP writer that may need that same check.
-- API writers already own the entry; bulk SQL writers prelock sorted entries or retry.
CREATE FUNCTION pl_list_check_guard() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE l lists;
BEGIN
 PERFORM 1 FROM list_entries WHERE id=CASE WHEN TG_OP='DELETE' THEN OLD.entry_id ELSE NEW.entry_id END
 FOR NO KEY UPDATE NOWAIT;
 IF TG_OP='DELETE' THEN RETURN OLD; END IF;
 SELECT x.* INTO l FROM lists x JOIN list_entries e ON e.list_id=x.id WHERE e.id=NEW.entry_id;
 IF l.mode='persistent' AND NEW.occurrence_id IS NOT NULL OR l.mode='per_occurrence' AND NEW.occurrence_id IS NULL THEN
 RAISE EXCEPTION 'occurrence does not match list mode' USING ERRCODE='PL422'; END IF;
 IF NEW.occurrence_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM item_occurrences WHERE id=NEW.occurrence_id AND household_id=l.household_id AND item_id IN (WITH RECURSIVE descendants AS (SELECT id FROM items WHERE id=l.item_id AND household_id=l.household_id UNION ALL SELECT i.id FROM items i JOIN descendants d ON i.split_from_item_id=d.id AND i.household_id=l.household_id) CYCLE id SET is_cycle USING path SELECT id FROM descendants WHERE NOT is_cycle)) THEN
 RAISE EXCEPTION 'invalid occurrence' USING ERRCODE='PL422'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER check_guard BEFORE INSERT OR DELETE ON list_entry_checks FOR EACH ROW EXECUTE FUNCTION pl_list_check_guard();

CREATE FUNCTION pl_list_check_record() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE r record;
BEGIN
 FOR r IN SELECT DISTINCT entry_id FROM changed_rows ORDER BY entry_id LOOP
 UPDATE list_entries SET updated_at=now() WHERE id=r.entry_id;
 END LOOP;
 RETURN NULL;
END $$;
CREATE TRIGGER check_record_insert AFTER INSERT ON list_entry_checks REFERENCING NEW TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_list_check_record();
CREATE TRIGGER check_record_delete AFTER DELETE ON list_entry_checks REFERENCING OLD TABLE AS changed_rows FOR EACH STATEMENT EXECUTE FUNCTION pl_list_check_record();

CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON lists FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();
CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON list_entries FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();
CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON list_viewers FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();
CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON list_entry_checks FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();

GRANT SELECT,INSERT,UPDATE ON lists,list_entries TO porchlight_app,porchlight_agent;
GRANT SELECT,INSERT,DELETE ON list_viewers,list_entry_checks TO porchlight_app,porchlight_agent;
GRANT SELECT ON list_revisions,list_entry_revisions TO porchlight_app,porchlight_agent;
GRANT DELETE ON lists,list_entries,list_viewers,list_entry_checks,list_revisions,list_entry_revisions TO porchlight_maintenance;
RESET search_path;
RESET ROLE;
