SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

CREATE TABLE ics_sources (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL REFERENCES households(id),
    owner_member_id uuid NOT NULL,
    target_member_id uuid,
    kind text NOT NULL CHECK (kind IN ('import','feed')),
    import_key text NOT NULL CHECK (length(import_key) BETWEEN 1 AND 200),
    name text NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    url text,
    allow_http boolean NOT NULL DEFAULT false,
    refresh_seconds integer NOT NULL DEFAULT 86400 CHECK (refresh_seconds BETWEEN 60 AND 2592000),
    enabled boolean NOT NULL DEFAULT true,
    next_fetch_at timestamptz NOT NULL DEFAULT now(),
    last_fetch_at timestamptz,
    last_status text CHECK (last_status IN ('ok','not_modified','failed','queued')),
    last_error text,
    etag text,
    last_modified text,
    version integer NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    UNIQUE(household_id,id),
    FOREIGN KEY(household_id,owner_member_id) REFERENCES household_members(household_id,id),
    FOREIGN KEY(household_id,target_member_id) REFERENCES household_members(household_id,id),
    CHECK ((kind='feed' AND url IS NOT NULL) OR (kind='import' AND url IS NULL))
);
CREATE UNIQUE INDEX ics_import_scope ON ics_sources(household_id,owner_member_id,coalesce(target_member_id,'00000000-0000-0000-0000-000000000000'::uuid),import_key) WHERE kind='import';
CREATE INDEX ics_due ON ics_sources(next_fetch_at,id) WHERE kind='feed' AND enabled AND deleted_at IS NULL;
ALTER TABLE items ADD COLUMN ics_source_id uuid;
ALTER TABLE items ADD CONSTRAINT items_ics_source_fk FOREIGN KEY(household_id,ics_source_id) REFERENCES ics_sources(household_id,id);
CREATE TABLE ics_uid_map (
    household_id uuid NOT NULL,
    source_id uuid NOT NULL,
    uid text NOT NULL CHECK(length(uid) BETWEEN 1 AND 4096),
    recurrence_key text NOT NULL,
    item_id uuid NOT NULL,
    occurrence_id uuid,
    expanded_start timestamptz,
    PRIMARY KEY(source_id,uid,recurrence_key),
    FOREIGN KEY(household_id,source_id) REFERENCES ics_sources(household_id,id),
    FOREIGN KEY(household_id,item_id) REFERENCES items(household_id,id)
    -- No occurrence FK: regeneration can remove/recreate an open instance. Adapter re-resolves the typed key.
);
CREATE TABLE ics_source_revisions (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL,
    source_id uuid NOT NULL REFERENCES ics_sources(id),
    version integer NOT NULL,
    snapshot jsonb NOT NULL,
    actor jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON ics_sources FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id','household_id','kind','owner_member_id','import_key');
CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON ics_sources FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();
CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON ics_uid_map FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();
CREATE TRIGGER append_only BEFORE UPDATE OR DELETE ON ics_source_revisions FOR EACH STATEMENT EXECUTE FUNCTION pl_append_only();
CREATE FUNCTION pl_ics_source_changed() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=pg_catalog, porchlight, pg_temp AS $$
BEGIN
    INSERT INTO ics_source_revisions(household_id,source_id,version,snapshot,actor)
    VALUES(NEW.household_id,NEW.id,NEW.version,to_jsonb(NEW),pl_current_actor());
    -- Configuration/status tickles are private to super admins, not to feed targets.
    IF NEW.kind='feed' THEN
        INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
        SELECT NEW.household_id,'ics_source.changed','ics_source',NEW.id,jsonb_build_object('source_id',NEW.id,'member_id',m.id)
        FROM household_members m WHERE m.household_id=NEW.household_id AND m.role='super_admin' AND m.deleted_at IS NULL;
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER ics_source_changed AFTER INSERT OR UPDATE ON ics_sources FOR EACH ROW EXECUTE FUNCTION pl_ics_source_changed();
GRANT SELECT,INSERT,UPDATE ON ics_sources,ics_uid_map TO porchlight_app;
GRANT SELECT,INSERT,UPDATE ON ics_sources TO porchlight_agent;
GRANT SELECT ON ics_uid_map,ics_source_revisions TO porchlight_agent;
GRANT SELECT ON ics_source_revisions TO porchlight_app;
GRANT SELECT,DELETE ON ics_sources,ics_uid_map,ics_source_revisions TO porchlight_maintenance;
RESET search_path;
RESET ROLE;
