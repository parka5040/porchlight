SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

CREATE TABLE households (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    name text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
    tzid text NOT NULL DEFAULT 'America/New_York' CHECK (pl_valid_tzid(tzid)),
    version int NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz
);

CREATE TABLE users (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL REFERENCES households (id),
    username text NOT NULL CHECK (username ~ '^[a-z0-9][a-z0-9._-]{1,31}$'),
    version int NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    UNIQUE (household_id, id),
    UNIQUE (household_id, username)
);

-- Password hashes live apart from users so no agent grant can ever reach them.
CREATE TABLE user_credentials (
    user_id uuid PRIMARY KEY,
    household_id uuid NOT NULL,
    password_hash text NOT NULL,
    password_changed_at timestamptz NOT NULL DEFAULT now(),
    failed_attempts int NOT NULL DEFAULT 0 CHECK (failed_attempts >= 0),
    locked_until timestamptz,
    version int NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (household_id, user_id) REFERENCES users (household_id, id)
);

CREATE TABLE household_members (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL REFERENCES households (id),
    user_id uuid UNIQUE,
    role text NOT NULL CHECK (role IN ('super_admin', 'user', 'dependent')),
    display_name text NOT NULL CHECK (length(btrim(display_name)) BETWEEN 1 AND 60),
    color text NOT NULL CHECK (color ~ '^#[0-9A-F]{6}$'),
    tzid text NOT NULL CHECK (pl_valid_tzid(tzid)),
    tz_source text NOT NULL DEFAULT 'device' CHECK (tz_source IN ('device', 'manual')),
    tz_authority_device_id uuid,
    version int NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    UNIQUE (household_id, id),
    FOREIGN KEY (household_id, user_id) REFERENCES users (household_id, id),
    CHECK ((role = 'dependent') = (user_id IS NULL))
);

CREATE TABLE devices (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL,
    member_id uuid NOT NULL,
    label text NOT NULL CHECK (length(btrim(label)) BETWEEN 1 AND 60),
    platform text NOT NULL CHECK (platform IN ('android', 'desktop')),
    revoked_at timestamptz,
    last_seen_at timestamptz,
    version int NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (household_id, id),
    FOREIGN KEY (household_id, member_id) REFERENCES household_members (household_id, id)
);
CREATE INDEX devices_member ON devices (member_id);

ALTER TABLE household_members
    ADD FOREIGN KEY (household_id, tz_authority_device_id) REFERENCES devices (household_id, id);

-- Every certificate ever issued to a device; revocation covers all of them. Rows are immutable.
CREATE TABLE device_certificates (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL,
    device_id uuid NOT NULL,
    fingerprint_sha256 bytea NOT NULL UNIQUE CHECK (length(fingerprint_sha256) = 32),
    serial text NOT NULL,
    not_before timestamptz NOT NULL,
    not_after timestamptz NOT NULL CHECK (not_after > not_before),
    created_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (household_id, device_id) REFERENCES devices (household_id, id)
);
CREATE INDEX device_certificates_device ON device_certificates (device_id);

CREATE TABLE sessions (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL,
    device_id uuid NOT NULL,
    token_hash bytea NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    expires_at timestamptz NOT NULL,
    last_used_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz,
    version int NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (household_id, device_id) REFERENCES devices (household_id, id)
);
CREATE INDEX sessions_device ON sessions (device_id);

CREATE TABLE enrollment_tokens (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL,
    member_id uuid NOT NULL,
    device_label text NOT NULL CHECK (length(btrim(device_label)) BETWEEN 1 AND 60),
    token_hash bytea NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    expires_at timestamptz NOT NULL,
    used_at timestamptz,
    created_by uuid NOT NULL,
    version int NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (household_id, member_id) REFERENCES household_members (household_id, id),
    FOREIGN KEY (household_id, created_by) REFERENCES household_members (household_id, id)
);

CREATE TABLE password_resets (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL,
    user_id uuid NOT NULL,
    code_hash bytea NOT NULL CHECK (length(code_hash) = 32),
    expires_at timestamptz NOT NULL,
    used_at timestamptz,
    created_by uuid NOT NULL,
    version int NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (household_id, user_id) REFERENCES users (household_id, id),
    FOREIGN KEY (household_id, created_by) REFERENCES household_members (household_id, id)
);

CREATE TABLE api_tokens (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL,
    owner_member_id uuid NOT NULL,
    name text NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 60),
    token_hash bytea NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    scopes text[] NOT NULL CHECK (cardinality(scopes) > 0 AND scopes <@ ARRAY['read', 'write', 'admin']),
    expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    last_used_at timestamptz,
    version int NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (household_id, owner_member_id) REFERENCES household_members (household_id, id)
);

CREATE TABLE member_settings (
    member_id uuid PRIMARY KEY,
    household_id uuid NOT NULL,
    settings jsonb NOT NULL DEFAULT '{}' CHECK (jsonb_typeof(settings) = 'object'),
    version int NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (household_id, member_id) REFERENCES household_members (household_id, id)
);

RESET search_path;
RESET ROLE;
