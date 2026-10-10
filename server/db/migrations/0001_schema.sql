-- Schema bootstrap. Test databases are created by a superuser (sqlx::test); hand them to the owner role so
-- every database looks like production before any DDL runs. sqlx 0.9 names them "_sqlx_test_" followed by 52
-- URL-safe base64 characters with '-' replaced by '_' (sqlx-core testing::TestSupport::db_name). Any other
-- database must already belong to porchlight_owner: a superuser must not take one over by accident.
DO $$
BEGIN
    IF (SELECT rolsuper FROM pg_roles WHERE rolname = current_user) THEN
        IF current_database() ~ '^_sqlx_test_[A-Za-z0-9_]{52}$' THEN
            EXECUTE format('ALTER DATABASE %I OWNER TO porchlight_owner', current_database());
        ELSIF (SELECT datdba FROM pg_database WHERE datname = current_database())
              <> 'porchlight_owner'::regrole THEN
            RAISE EXCEPTION 'database % is not owned by porchlight_owner: run migrations as porchlight_migrator '
                'against the porchlight database', current_database();
        END IF;
    END IF;
END $$;

SET LOCAL ROLE porchlight_owner;

CREATE SCHEMA porchlight;
DO $$ BEGIN EXECUTE format('ALTER DATABASE %I SET search_path = porchlight, public', current_database()); END $$;
CREATE EXTENSION IF NOT EXISTS btree_gist WITH SCHEMA porchlight;
GRANT USAGE ON SCHEMA porchlight TO porchlight_app, porchlight_agent;

-- Valid IANA zone names (rejects POSIX-style strings and abbreviations). STABLE: reads the tz database.
CREATE FUNCTION porchlight.pl_valid_tzid(z text) RETURNS boolean
    LANGUAGE sql STABLE
    AS $$ SELECT z !~ '^(posix|right)/' AND EXISTS (SELECT 1 FROM pg_catalog.pg_timezone_names WHERE name = z) $$;

RESET ROLE;
