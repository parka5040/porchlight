-- Porchlight cluster bootstrap: roles and the porchlight database. Idempotent; run with psql as a superuser.
-- Required psql variables: migrator_password, app_password, agent_password, backup_password.
\set ON_ERROR_STOP on
-- Validate before creating any roles, including direct callers that bypass the init shell.
\if :{?backup_password}
SELECT length(:'backup_password') > 0 AS backup_password_present \gset
\else
\set backup_password_present false
\endif
\if :backup_password_present
\else
DO $$ BEGIN RAISE EXCEPTION 'PORCHLIGHT_BACKUP_PASSWORD is required; run scripts/dev-env.sh before cluster creation'; END $$;
\endif

SELECT format('CREATE ROLE %I NOLOGIN', r)
FROM unnest(ARRAY['porchlight_owner', 'porchlight_maintenance']) AS r
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = r) \gexec

SELECT format('CREATE ROLE %I LOGIN', r)
FROM unnest(ARRAY['porchlight_migrator', 'porchlight_app', 'porchlight_agent', 'porchlight_backup']) AS r
WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = r) \gexec

ALTER ROLE porchlight_migrator PASSWORD :'migrator_password';
ALTER ROLE porchlight_app PASSWORD :'app_password';
ALTER ROLE porchlight_agent PASSWORD :'agent_password';
ALTER ROLE porchlight_backup PASSWORD :'backup_password';
GRANT pg_read_all_data TO porchlight_backup;

GRANT porchlight_owner TO porchlight_migrator WITH INHERIT FALSE, SET TRUE;
GRANT porchlight_maintenance TO porchlight_owner WITH INHERIT FALSE, SET TRUE;

-- Defaults that end an agent's forgotten transaction or runaway statement (accidents only: a session can
-- override them).
ALTER ROLE porchlight_agent SET idle_in_transaction_session_timeout = '60s';
ALTER ROLE porchlight_agent SET lock_timeout = '10s';
ALTER ROLE porchlight_agent SET statement_timeout = '60s';

SELECT 'CREATE DATABASE porchlight OWNER porchlight_owner'
WHERE NOT EXISTS (SELECT 1 FROM pg_database WHERE datname = 'porchlight') \gexec

-- Advisory locks: only the migrator needs them (sqlx's migration lock); any other role could otherwise hold that
-- lock, or any other, indefinitely. Function privileges are per database: template1 first, so databases created
-- later (including sqlx test databases) inherit it, then porchlight itself.
\connect template1
SELECT format('REVOKE EXECUTE ON FUNCTION %s FROM PUBLIC', p.oid::regprocedure)
FROM pg_proc p WHERE p.pronamespace = 'pg_catalog'::regnamespace AND p.proname ~ '^pg_(try_)?advisory_' \gexec
SELECT format('GRANT EXECUTE ON FUNCTION %s TO porchlight_migrator', p.oid::regprocedure)
FROM pg_proc p WHERE p.pronamespace = 'pg_catalog'::regnamespace AND p.proname ~ '^pg_(try_)?advisory_' \gexec

\connect porchlight
SELECT format('REVOKE EXECUTE ON FUNCTION %s FROM PUBLIC', p.oid::regprocedure)
FROM pg_proc p WHERE p.pronamespace = 'pg_catalog'::regnamespace AND p.proname ~ '^pg_(try_)?advisory_' \gexec
SELECT format('GRANT EXECUTE ON FUNCTION %s TO porchlight_migrator', p.oid::regprocedure)
FROM pg_proc p WHERE p.pronamespace = 'pg_catalog'::regnamespace AND p.proname ~ '^pg_(try_)?advisory_' \gexec

-- sqlx keeps its bookkeeping table in public, created by the connecting migrator.
GRANT USAGE, CREATE ON SCHEMA public TO porchlight_migrator;
-- Database-level state the migrations also set (0001, 0008), repeated here so a dump restored into a freshly
-- bootstrapped database keeps it (pg_dump does not carry database settings or database ACLs by default).
ALTER DATABASE porchlight SET search_path = porchlight, public;
REVOKE TEMPORARY ON DATABASE porchlight FROM PUBLIC;
