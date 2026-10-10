SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Maintains version/created_at/updated_at for every writer. TG_ARGV lists the row's immutable key columns.
CREATE FUNCTION pl_version_guard() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    col text;
BEGIN
    IF TG_OP = 'INSERT' THEN
        NEW.version := 1;
        NEW.created_at := now();
        NEW.updated_at := NEW.created_at;
        RETURN NEW;
    END IF;
    IF NEW.version IS DISTINCT FROM OLD.version THEN
        RAISE EXCEPTION 'version is maintained by the database; detect stale writes with WHERE version = <expected>'
            USING ERRCODE = 'PL409', TABLE = TG_TABLE_NAME;
    END IF;
    FOREACH col IN ARRAY TG_ARGV || ARRAY['created_at'] LOOP
        IF to_jsonb(NEW) -> col IS DISTINCT FROM to_jsonb(OLD) -> col THEN
            RAISE EXCEPTION '% cannot change', col USING ERRCODE = 'PL422', TABLE = TG_TABLE_NAME, COLUMN = col;
        END IF;
    END LOOP;
    NEW.version := OLD.version + 1;
    NEW.updated_at := now();
    RETURN NEW;
END $$;

CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON households
    FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id');
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id', 'household_id');
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON user_credentials
    FOR EACH ROW EXECUTE FUNCTION pl_version_guard('user_id', 'household_id');
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON household_members
    FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id', 'household_id');
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON devices
    FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id', 'household_id', 'member_id');
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON sessions
    FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id', 'household_id', 'device_id');
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON enrollment_tokens
    FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id', 'household_id');
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON password_resets
    FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id', 'household_id');
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON api_tokens
    FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id', 'household_id');
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON member_settings
    FOR EACH ROW EXECUTE FUNCTION pl_version_guard('member_id', 'household_id');
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON items
    FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id', 'household_id');
CREATE TRIGGER aa_version_guard BEFORE INSERT OR UPDATE ON item_occurrences
    FOR EACH ROW EXECUTE FUNCTION pl_version_guard('id', 'household_id', 'item_id', 'seq');

RESET search_path;
RESET ROLE;
