SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Child tables reference sessions by (household_id, id, device_id): a step-up's session must belong to its device.
ALTER TABLE sessions ADD UNIQUE (household_id, id, device_id);

-- A reset code is void after 5 wrong guesses.
ALTER TABLE password_resets
    ADD COLUMN failed_attempts int NOT NULL DEFAULT 0 CHECK (failed_attempts >= 0);

-- Step-up grants: a fresh password check bound to one session, its device and one action; single use, 5 minutes.
CREATE TABLE step_ups (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL,
    session_id uuid NOT NULL,
    device_id uuid NOT NULL,
    action text NOT NULL CHECK (action ~ '^[a-z]+(\.[a-z_]+)+$'),
    token_hash bytea NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    expires_at timestamptz NOT NULL,
    used_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (household_id, session_id, device_id) REFERENCES sessions (household_id, id, device_id)
        ON DELETE CASCADE,
    FOREIGN KEY (household_id, device_id) REFERENCES devices (household_id, id)
);
CREATE INDEX step_ups_session ON step_ups (session_id);

-- Replay protection for writes. The claim row is inserted first in the request's own transaction, so a concurrent
-- duplicate blocks on the unique index until the first commits (then replays its response) or rolls back (then
-- proceeds). request_hash is an HMAC (keyed by the API), never a plain digest of a body that may hold a password.
-- response_sealed holds the allow-listed headers and the body, sealed by the API (AES-256-GCM: nonce || ct+tag).
CREATE TABLE idempotency_keys (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL REFERENCES households (id),
    scope_kind text NOT NULL CHECK (scope_kind IN ('device', 'pat', 'enrollment')),
    scope_id uuid NOT NULL,
    idem_key text NOT NULL CHECK (idem_key ~ '^[\x21-\x7e]{1,255}$'),
    request_hash bytea NOT NULL CHECK (length(request_hash) = 32),
    response_status int CHECK (response_status BETWEEN 100 AND 599),
    response_sealed bytea,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    UNIQUE (scope_kind, scope_id, idem_key)
);
CREATE INDEX idempotency_keys_expiry ON idempotency_keys (expires_at);

-- Revocation and consumption are final: once set, these timestamps never change (TG_ARGV[0] = column).
CREATE FUNCTION pl_set_once() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF to_jsonb(OLD) -> TG_ARGV[0] <> 'null'::jsonb
       AND to_jsonb(NEW) -> TG_ARGV[0] IS DISTINCT FROM to_jsonb(OLD) -> TG_ARGV[0] THEN
        RAISE EXCEPTION '% is final once set', TG_ARGV[0]
            USING ERRCODE = 'PL422', TABLE = TG_TABLE_NAME, COLUMN = TG_ARGV[0];
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER ab_revoked_at_final BEFORE UPDATE ON devices
    FOR EACH ROW EXECUTE FUNCTION pl_set_once('revoked_at');
CREATE TRIGGER ab_revoked_at_final BEFORE UPDATE ON sessions
    FOR EACH ROW EXECUTE FUNCTION pl_set_once('revoked_at');
CREATE TRIGGER ab_revoked_at_final BEFORE UPDATE ON api_tokens
    FOR EACH ROW EXECUTE FUNCTION pl_set_once('revoked_at');
CREATE TRIGGER ab_used_at_final BEFORE UPDATE ON enrollment_tokens
    FOR EACH ROW EXECUTE FUNCTION pl_set_once('used_at');
CREATE TRIGGER ab_used_at_final BEFORE UPDATE ON password_resets
    FOR EACH ROW EXECUTE FUNCTION pl_set_once('used_at');
CREATE TRIGGER ab_used_at_final BEFORE UPDATE ON step_ups
    FOR EACH ROW EXECUTE FUNCTION pl_set_once('used_at');

-- Revocation hint for every API process (the handshake verifier's revoked set and open streams). The payload is a
-- hint: listeners re-read devices/device_certificates.
CREATE FUNCTION pl_device_revoked() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    PERFORM pg_notify('porchlight_revocation', NEW.id::text);
    RETURN NULL;
END $$;

CREATE TRIGGER device_revoked AFTER UPDATE OF revoked_at ON devices
    FOR EACH ROW WHEN (OLD.revoked_at IS NULL AND NEW.revoked_at IS NOT NULL)
    EXECUTE FUNCTION pl_device_revoked();

-- One row per install: ties the database to the CA in the data dir. `porchlight init` inserts it in the same
-- transaction as the first household (the primary key serializes concurrent inits); `serve` refuses a data dir whose
-- CA fingerprint differs.
CREATE TABLE install_state (
    id boolean PRIMARY KEY DEFAULT true CHECK (id),
    household_id uuid NOT NULL REFERENCES households (id),
    ca_fingerprint bytea NOT NULL CHECK (length(ca_fingerprint) = 32),
    initialized_at timestamptz NOT NULL DEFAULT now()
);

-- Members who can read an item given these authoritative values: owner, assignees, viewers when 'selected', every
-- live household member when 'household'. Eviction events are addressed to the readers *before* a change.
CREATE FUNCTION pl_item_readers(p_household_id uuid, p_item_id uuid, p_owner_id uuid, p_visibility text)
    RETURNS uuid[]
    LANGUAGE sql STABLE
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
    SELECT coalesce(array_agg(DISTINCT r.m ORDER BY r.m), '{}')
    FROM (
        SELECT p_owner_id AS m
        UNION SELECT member_id FROM item_assignees WHERE item_id = p_item_id
        UNION SELECT member_id FROM item_viewers WHERE item_id = p_item_id AND p_visibility = 'selected'
        UNION SELECT id FROM household_members
            WHERE household_id = p_household_id AND deleted_at IS NULL AND p_visibility = 'household'
    ) r
    WHERE r.m IS NOT NULL
$$;

-- Supersedes 0007's version: names the former readers, and soft deletion now also emits the event.
CREATE OR REPLACE FUNCTION pl_item_access_changed() RETURNS trigger
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path = pg_catalog, porchlight, pg_temp
    AS $$
BEGIN
    INSERT INTO outbox (household_id, kind, entity_type, entity_id, payload)
    VALUES (NEW.household_id, 'item.access_changed', 'item', NEW.id,
            jsonb_strip_nulls(jsonb_build_object(
                'previous_owner_id', CASE WHEN OLD.owner_id IS DISTINCT FROM NEW.owner_id THEN OLD.owner_id END,
                'previous_visibility', CASE WHEN OLD.visibility IS DISTINCT FROM NEW.visibility THEN OLD.visibility END,
                'deleted', CASE WHEN OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL THEN true END,
                'recipients', to_jsonb(pl_item_readers(OLD.household_id, OLD.id, OLD.owner_id, OLD.visibility)))));
    PERFORM pg_notify('porchlight_outbox', '');
    RETURN NULL;
END $$;

DROP TRIGGER items_access_changed ON items;
CREATE TRIGGER items_access_changed AFTER UPDATE ON items
    FOR EACH ROW WHEN (OLD.owner_id IS DISTINCT FROM NEW.owner_id OR OLD.visibility IS DISTINCT FROM NEW.visibility
                       OR (OLD.deleted_at IS NULL AND NEW.deleted_at IS NOT NULL))
    EXECUTE FUNCTION pl_item_access_changed();

GRANT SELECT, INSERT, UPDATE, DELETE ON idempotency_keys, step_ups TO porchlight_app;
GRANT SELECT, INSERT ON install_state TO porchlight_app;

RESET search_path;
RESET ROLE;
