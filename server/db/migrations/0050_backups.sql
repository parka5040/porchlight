-- O1 operational metadata, not domain writes: no actor/version/outbox (O2 exposes read-only status).
SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
GRANT USAGE ON SCHEMA porchlight TO porchlight_backup;

CREATE TABLE porchlight.backup_runs (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    started_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    finished_at timestamptz,
    status text NOT NULL CHECK (status IN ('running', 'success', 'failed')),
    bundle_file text CHECK (bundle_file ~ '^porchlight-[0-9]{8}T[0-9]{12}Z[.]tar[.]age$'),
    size bigint CHECK (size > 0),
    sha256 text CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    error text,
    next_due_at timestamptz,
    CHECK (finished_at IS NULL OR finished_at >= started_at),
    CHECK (status <> 'success' OR (finished_at IS NOT NULL AND bundle_file IS NOT NULL
                                  AND size IS NOT NULL AND sha256 IS NOT NULL AND next_due_at IS NOT NULL)),
    CHECK (status <> 'failed' OR (finished_at IS NOT NULL AND error IS NOT NULL))
);
CREATE INDEX backup_runs_latest_success ON porchlight.backup_runs (finished_at DESC) WHERE status = 'success';
GRANT SELECT ON porchlight.backup_runs TO porchlight_app, porchlight_agent;
GRANT INSERT, UPDATE ON porchlight.backup_runs TO porchlight_backup;
RESET search_path;
RESET ROLE;
