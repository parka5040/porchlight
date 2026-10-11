SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Issuance history remains immutable; only monotonic use/revocation metadata can change.
-- Existing leaves have unknown usage. Only leaves issued after tracking began are
-- confirmed unused while first_seen_at is NULL; never revoke legacy unknowns on renewal.
ALTER TABLE device_certificates ADD COLUMN usage_tracked boolean NOT NULL DEFAULT false;
ALTER TABLE device_certificates ALTER COLUMN usage_tracked SET DEFAULT true;
ALTER TABLE device_certificates ADD COLUMN first_seen_at timestamptz;
ALTER TABLE device_certificates ADD COLUMN revoked_at timestamptz;
DROP TRIGGER immutable_history ON device_certificates;
CREATE TRIGGER immutable_history BEFORE DELETE ON device_certificates
 FOR EACH STATEMENT EXECUTE FUNCTION pl_immutable_history();
CREATE FUNCTION pl_certificate_metadata_only() RETURNS trigger
 LANGUAGE plpgsql SET search_path = pg_catalog, porchlight, pg_temp AS $$
BEGIN
 IF (to_jsonb(NEW) - 'first_seen_at' - 'revoked_at') IS DISTINCT FROM
    (to_jsonb(OLD) - 'first_seen_at' - 'revoked_at') OR
    (OLD.first_seen_at IS NOT NULL AND NEW.first_seen_at IS DISTINCT FROM OLD.first_seen_at) OR
    (OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS DISTINCT FROM OLD.revoked_at) THEN
  RAISE EXCEPTION 'certificate history is immutable' USING ERRCODE='PL422';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER metadata_only BEFORE UPDATE ON device_certificates
 FOR EACH ROW EXECUTE FUNCTION pl_certificate_metadata_only();
GRANT UPDATE(first_seen_at,revoked_at) ON device_certificates TO porchlight_app;
-- Existing issuance rows have no per-leaf use history; tracking starts with this migration.
-- The same durable issuance budget still includes every old and revoked certificate.
CREATE TRIGGER certificate_revoked AFTER UPDATE OF revoked_at ON device_certificates
 FOR EACH ROW WHEN (OLD.revoked_at IS NULL AND NEW.revoked_at IS NOT NULL)
 EXECUTE FUNCTION pl_device_revoked();

RESET search_path;
RESET ROLE;
