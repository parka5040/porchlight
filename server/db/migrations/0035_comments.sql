-- S7b: collaboration rows never enqueue item derivation or bump the item version.
SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
CREATE TABLE comments (
 id uuid PRIMARY KEY DEFAULT uuidv7(), household_id uuid NOT NULL, item_id uuid NOT NULL,
 author_id uuid, author_token_id uuid, body text NOT NULL CHECK (char_length(body) BETWEEN 1 AND 10000),
 mentions uuid[] NOT NULL DEFAULT '{}', edited_at timestamptz, deleted_at timestamptz,
 version int NOT NULL DEFAULT 1, created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now(),
 UNIQUE(household_id,id), UNIQUE(household_id,item_id,id),
 FOREIGN KEY(household_id,item_id) REFERENCES items(household_id,id),
 FOREIGN KEY(household_id,author_id) REFERENCES household_members(household_id,id),
 FOREIGN KEY(author_token_id) REFERENCES api_tokens(id) ON DELETE SET NULL,
 CHECK (author_id IS NULL OR author_token_id IS NULL)
);
-- Preserve attribution immutability, except FK token-purge erasure. A purged PAT has no member identity.
CREATE FUNCTION pl_collaboration_token_guard() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE member_col text := TG_ARGV[0]; token_col text := TG_ARGV[1]; old_token uuid;
BEGIN
 IF TG_OP = 'INSERT' THEN
  IF (to_jsonb(NEW)->>member_col IS NULL) = (to_jsonb(NEW)->>token_col IS NULL) THEN
   RAISE EXCEPTION 'one collaboration author required' USING ERRCODE='PL422';
  END IF;
 ELSE
  old_token := (to_jsonb(OLD)->>token_col)::uuid;
  IF to_jsonb(NEW)->token_col IS DISTINCT FROM to_jsonb(OLD)->token_col AND
   NOT (old_token IS NOT NULL AND to_jsonb(NEW)->>token_col IS NULL AND pg_trigger_depth()>1
        AND NOT EXISTS(SELECT 1 FROM api_tokens WHERE id=old_token)) THEN
   RAISE EXCEPTION 'token attribution cannot change' USING ERRCODE='PL422';
  END IF;
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER ab_token_guard BEFORE INSERT OR UPDATE ON comments FOR EACH ROW
 EXECUTE FUNCTION pl_collaboration_token_guard('author_id','author_token_id');
CREATE INDEX comments_page ON comments(item_id,id) WHERE deleted_at IS NULL;
CREATE INDEX comments_search ON comments USING gin(to_tsvector('simple',body)) WHERE deleted_at IS NULL;
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON comments FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id','household_id','item_id','author_id');
CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON comments FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();
CREATE TABLE collaboration_audit (
 id uuid PRIMARY KEY DEFAULT uuidv7(), household_id uuid NOT NULL REFERENCES households(id),
 entity_type text NOT NULL, entity_id uuid NOT NULL, action text NOT NULL, actor jsonb NOT NULL,
 created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TRIGGER append_only BEFORE UPDATE OR DELETE ON collaboration_audit FOR EACH STATEMENT EXECUTE FUNCTION pl_append_only();
CREATE FUNCTION pl_collaboration_changed() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
 INSERT INTO collaboration_audit(household_id,entity_type,entity_id,action,actor)
 VALUES(NEW.household_id,TG_ARGV[0],NEW.id,TG_OP,pl_current_actor());
 INSERT INTO outbox(household_id,kind,entity_type,entity_id,payload)
 VALUES(NEW.household_id,TG_ARGV[0]||'.changed',TG_ARGV[0],NEW.id,jsonb_build_object('item_id',NEW.item_id));
 PERFORM pg_notify('porchlight_outbox','');
 RETURN NULL;
END $$;
CREATE TRIGGER comments_changed AFTER INSERT OR UPDATE ON comments FOR EACH ROW EXECUTE FUNCTION pl_collaboration_changed('comment');
GRANT SELECT ON comments,collaboration_audit TO porchlight_app,porchlight_agent;
GRANT INSERT(id,household_id,item_id,author_id,author_token_id,body,mentions), UPDATE(body,mentions,edited_at,deleted_at) ON comments TO porchlight_app,porchlight_agent;
GRANT SELECT,DELETE ON comments,collaboration_audit TO porchlight_maintenance;
RESET search_path;
RESET ROLE;
