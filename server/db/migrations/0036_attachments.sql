SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
-- Fixed sharded filesystem gates survive blob-row deletion. GC holds a gate in an independent
-- transaction until AFTER its metadata transaction commits and bytes are unlinked.
CREATE TABLE blob_storage_locks (shard text PRIMARY KEY CHECK(shard ~ '^[0-9a-f]{2}$'));
INSERT INTO blob_storage_locks SELECT lpad(to_hex(i),2,'0') FROM generate_series(0,255) i;
GRANT SELECT,UPDATE(shard) ON blob_storage_locks TO porchlight_app,porchlight_agent;
CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON blob_storage_locks FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();
-- UPDATE privilege exists only for SELECT FOR UPDATE; the permanent gate keys cannot be edited.
CREATE TRIGGER ab_fixed_gates BEFORE UPDATE OR DELETE ON blob_storage_locks FOR EACH STATEMENT EXECUTE FUNCTION pl_append_only();
CREATE TABLE blobs (
 household_id uuid NOT NULL REFERENCES households(id), sha256 text NOT NULL CHECK(sha256 ~ '^[0-9a-f]{64}$'),
 size bigint NOT NULL CHECK(size BETWEEN 0 AND 26214400),
 content_type text NOT NULL CHECK(content_type IN ('image/jpeg','image/png','image/gif','image/webp','application/pdf','text/plain')),
 created_at timestamptz NOT NULL DEFAULT now(), last_referenced_at timestamptz NOT NULL DEFAULT now(),
 PRIMARY KEY(household_id,sha256)
);
CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON blobs FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();
CREATE TABLE attachments (
 id uuid PRIMARY KEY DEFAULT uuidv7(), household_id uuid NOT NULL, item_id uuid NOT NULL, comment_id uuid,
 blob_sha256 text NOT NULL, thumbnail_sha256 text, original_name text NOT NULL CHECK(char_length(original_name) BETWEEN 1 AND 180 AND original_name !~ '[[:cntrl:]/\\]'),
 uploader_id uuid, uploader_token_id uuid, deleted_at timestamptz,
 created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now(), version int NOT NULL DEFAULT 1,
 UNIQUE(household_id,id), FOREIGN KEY(household_id,item_id) REFERENCES items(household_id,id),
 FOREIGN KEY(household_id,item_id,comment_id) REFERENCES comments(household_id,item_id,id),
 FOREIGN KEY(household_id,uploader_id) REFERENCES household_members(household_id,id),
 FOREIGN KEY(uploader_token_id) REFERENCES api_tokens(id) ON DELETE SET NULL, CHECK(uploader_id IS NULL OR uploader_token_id IS NULL)
);
CREATE INDEX attachments_item ON attachments(item_id,id) WHERE deleted_at IS NULL;
CREATE TRIGGER aa_require_agent_label BEFORE INSERT OR UPDATE OR DELETE ON attachments FOR EACH STATEMENT EXECUTE FUNCTION pl_require_agent_label();
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON attachments FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id','household_id','item_id','comment_id','blob_sha256','thumbnail_sha256','original_name','uploader_id');
CREATE TRIGGER ab_token_guard BEFORE INSERT OR UPDATE ON attachments FOR EACH ROW EXECUTE FUNCTION pl_collaboration_token_guard('uploader_id','uploader_token_id');
CREATE TRIGGER attachments_changed AFTER INSERT OR UPDATE ON attachments FOR EACH ROW EXECUTE FUNCTION pl_collaboration_changed('attachment');
-- All references lock blobs in hash order. Soft deletion resets the 30-day clock.
CREATE FUNCTION pl_attachment_reference_clock() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
 PERFORM 1 FROM blobs WHERE household_id=NEW.household_id AND sha256 IN (NEW.blob_sha256,NEW.thumbnail_sha256) ORDER BY sha256 FOR UPDATE;
 IF NEW.deleted_at IS NULL AND (NOT EXISTS(SELECT 1 FROM blobs WHERE household_id=NEW.household_id AND sha256=NEW.blob_sha256) OR (NEW.thumbnail_sha256 IS NOT NULL AND NOT EXISTS(SELECT 1 FROM blobs WHERE household_id=NEW.household_id AND sha256=NEW.thumbnail_sha256))) THEN
 RAISE EXCEPTION 'missing live blob' USING ERRCODE='23503'; END IF;
 UPDATE blobs SET last_referenced_at=clock_timestamp() WHERE household_id=NEW.household_id AND sha256 IN (NEW.blob_sha256,NEW.thumbnail_sha256);
 RETURN NEW;
END $$;
CREATE TRIGGER ab_reference_clock BEFORE INSERT OR UPDATE ON attachments FOR EACH ROW EXECUTE FUNCTION pl_attachment_reference_clock();
CREATE TABLE blob_leases (
 id uuid PRIMARY KEY DEFAULT uuidv7(), holder text NOT NULL CHECK(char_length(holder) BETWEEN 1 AND 200),
 created_at timestamptz NOT NULL, expires_at timestamptz NOT NULL CHECK(expires_at>created_at)
);
CREATE TABLE blob_lease_items (
 lease_id uuid NOT NULL REFERENCES blob_leases(id) ON DELETE CASCADE,
 household_id uuid NOT NULL, sha256 text NOT NULL, PRIMARY KEY(lease_id,household_id,sha256),
 FOREIGN KEY(household_id,sha256) REFERENCES blobs(household_id,sha256) ON DELETE CASCADE
);
CREATE FUNCTION pl_take_blob_lease(p_holder text,p_duration interval DEFAULT interval '6 hours',p_now timestamptz DEFAULT clock_timestamp()) RETURNS uuid
 LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
DECLARE lease uuid := uuidv7();
BEGIN
 IF p_duration <= interval '0' OR p_duration > interval '24 hours' THEN RAISE EXCEPTION 'invalid lease duration' USING ERRCODE='PL422'; END IF;
 -- Serialize GC first, then read references in a new statement/snapshot.
 PERFORM 1 FROM blobs ORDER BY household_id,sha256 FOR SHARE;
 INSERT INTO blob_leases(id,holder,created_at,expires_at) VALUES(lease,p_holder,p_now,p_now+p_duration);
 INSERT INTO blob_lease_items(lease_id,household_id,sha256)
 SELECT lease,b.household_id,b.sha256 FROM blobs b WHERE EXISTS(SELECT 1 FROM attachments a WHERE a.household_id=b.household_id AND a.deleted_at IS NULL AND (a.blob_sha256=b.sha256 OR a.thumbnail_sha256=b.sha256));
 RETURN lease;
END $$;
CREATE FUNCTION pl_blob_delete_guard() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER
 SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM attachments WHERE household_id=OLD.household_id AND deleted_at IS NULL AND (blob_sha256=OLD.sha256 OR thumbnail_sha256=OLD.sha256)) OR EXISTS(SELECT 1 FROM blob_lease_items li JOIN blob_leases l ON l.id=li.lease_id WHERE li.household_id=OLD.household_id AND li.sha256=OLD.sha256 AND l.expires_at>clock_timestamp()) THEN
 RAISE EXCEPTION 'referenced blob' USING ERRCODE='23503'; END IF;
 RETURN OLD;
END $$;
CREATE TRIGGER ab_blob_delete_guard BEFORE DELETE ON blobs FOR EACH ROW EXECUTE FUNCTION pl_blob_delete_guard();
-- Crash/rollback recovery: adopt old canonical files only for collection, never expose them.
CREATE FUNCTION pl_adopt_orphan_blob(p_household uuid,p_hash text,p_size bigint,p_modified timestamptz,p_now timestamptz) RETURNS void
 LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
 IF p_modified > p_now-interval '30 days' THEN RAISE EXCEPTION 'orphan is not old enough' USING ERRCODE='PL422'; END IF;
 INSERT INTO blobs(household_id,sha256,size,content_type,last_referenced_at) VALUES(p_household,p_hash,p_size,'text/plain',p_modified) ON CONFLICT DO NOTHING;
END $$;
REVOKE ALL ON FUNCTION pl_adopt_orphan_blob(uuid,text,bigint,timestamptz,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pl_adopt_orphan_blob(uuid,text,bigint,timestamptz,timestamptz) TO porchlight_app;
REVOKE ALL ON FUNCTION pl_take_blob_lease(text,interval,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION pl_take_blob_lease(text,interval,timestamptz) TO porchlight_backup,porchlight_app;
GRANT SELECT ON blobs,attachments TO porchlight_app,porchlight_agent;
GRANT INSERT(household_id,sha256,size,content_type), UPDATE(last_referenced_at,size,content_type) ON blobs TO porchlight_app,porchlight_agent;
GRANT INSERT(id,household_id,item_id,comment_id,blob_sha256,thumbnail_sha256,original_name,uploader_id,uploader_token_id), UPDATE(deleted_at) ON attachments TO porchlight_app,porchlight_agent;
GRANT SELECT,DELETE ON blobs,attachments TO porchlight_maintenance;
GRANT DELETE ON blobs TO porchlight_app;
GRANT SELECT,DELETE ON blob_leases,blob_lease_items TO porchlight_app,porchlight_backup,porchlight_maintenance;
RESET search_path;
RESET ROLE;
