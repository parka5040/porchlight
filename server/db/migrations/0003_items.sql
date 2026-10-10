SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

CREATE TABLE items (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL REFERENCES households (id),
    title text NOT NULL CHECK (length(btrim(title)) BETWEEN 1 AND 200),
    notes text NOT NULL DEFAULT '' CHECK (length(notes) <= 20000),
    priority smallint NOT NULL DEFAULT 0 CHECK (priority BETWEEN 0 AND 3),
    owner_id uuid NOT NULL,
    subject_member_id uuid,
    kind text NOT NULL CHECK (kind IN ('todo', 'deadline', 'appointment')),
    time_mode text NOT NULL DEFAULT 'fixed' CHECK (time_mode IN ('fixed', 'floating')),
    tzid text CHECK (tzid IS NULL OR pl_valid_tzid(tzid)),
    all_day boolean NOT NULL DEFAULT false,
    due_date date,
    due_at timestamptz,
    starts_at timestamptz,
    ends_at timestamptz,
    local_due timestamp,
    local_start timestamp,
    local_end timestamp,
    address text CHECK (length(address) <= 500),
    buffer_before_min int NOT NULL DEFAULT 0 CHECK (buffer_before_min BETWEEN 0 AND 1440),
    buffer_after_min int NOT NULL DEFAULT 0 CHECK (buffer_after_min BETWEEN 0 AND 1440),
    est_minutes int CHECK (est_minutes BETWEEN 1 AND 10080),
    rrule text CHECK (rrule ~ '^FREQ=(DAILY|WEEKLY|MONTHLY|YEARLY)(;(INTERVAL=[1-9][0-9]{0,2}|BYDAY=[+-]?[1-5]?(MO|TU|WE|TH|FR|SA|SU)(,[+-]?[1-5]?(MO|TU|WE|TH|FR|SA|SU))*|BYMONTHDAY=-?([1-9]|[12][0-9]|3[01])(,-?([1-9]|[12][0-9]|3[01]))*|COUNT=[1-9][0-9]{0,3}|UNTIL=[0-9]{8}(T[0-9]{6}Z?)?))*$'),
    recurrence_mode text CHECK (recurrence_mode IN ('calendar', 'completion')),
    completion_interval interval CHECK (completion_interval > interval '0'),
    missed_policy text NOT NULL DEFAULT 'carry_over' CHECK (missed_policy IN ('carry_over', 'skip', 'stack')),
    visibility text NOT NULL DEFAULT 'household' CHECK (visibility IN ('household', 'private', 'selected')),
    source text CHECK (length(source) <= 200),
    version int NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    deleted_at timestamptz,
    UNIQUE (household_id, id),
    FOREIGN KEY (household_id, owner_id) REFERENCES household_members (household_id, id),
    FOREIGN KEY (household_id, subject_member_id) REFERENCES household_members (household_id, id),
    -- CHECK passes on NULL (recurrence_mode = 'calendar' is NULL when the mode is NULL), hence the coalesce.
    CONSTRAINT items_recurrence_shape CHECK (coalesce((
        (recurrence_mode IS NULL AND rrule IS NULL AND completion_interval IS NULL)
        OR (recurrence_mode = 'calendar' AND rrule IS NOT NULL AND completion_interval IS NULL)
        OR (recurrence_mode = 'completion' AND rrule IS NULL AND completion_interval IS NOT NULL)), false)),
    -- CHECK passes on NULL, so every branch spells out IS NOT NULL explicitly.
    CONSTRAINT items_time_shape CHECK (coalesce(
        CASE kind
            WHEN 'todo' THEN
                due_at IS NULL AND starts_at IS NULL AND ends_at IS NULL
                AND local_due IS NULL AND local_start IS NULL AND local_end IS NULL
            WHEN 'deadline' THEN
                starts_at IS NULL AND ends_at IS NULL AND local_start IS NULL AND local_end IS NULL AND due_date IS NULL
                AND ((time_mode = 'fixed' AND due_at IS NOT NULL AND tzid IS NOT NULL AND local_due IS NULL)
                  OR (time_mode = 'floating' AND local_due IS NOT NULL AND due_at IS NULL AND tzid IS NULL))
            WHEN 'appointment' THEN
                due_at IS NULL AND local_due IS NULL AND due_date IS NULL
                AND ((time_mode = 'fixed' AND starts_at IS NOT NULL AND ends_at IS NOT NULL AND starts_at < ends_at
                      AND tzid IS NOT NULL AND local_start IS NULL AND local_end IS NULL)
                  OR (time_mode = 'floating' AND local_start IS NOT NULL AND local_end IS NOT NULL
                      AND local_start < local_end AND starts_at IS NULL AND ends_at IS NULL AND tzid IS NULL))
        END, false))
);
CREATE INDEX items_owner ON items (owner_id);

CREATE TABLE item_assignees (
    household_id uuid NOT NULL,
    item_id uuid NOT NULL,
    member_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (item_id, member_id),
    FOREIGN KEY (household_id, item_id) REFERENCES items (household_id, id),
    FOREIGN KEY (household_id, member_id) REFERENCES household_members (household_id, id)
);
CREATE INDEX item_assignees_member ON item_assignees (member_id);

CREATE TABLE item_viewers (
    household_id uuid NOT NULL,
    item_id uuid NOT NULL,
    member_id uuid NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (item_id, member_id),
    FOREIGN KEY (household_id, item_id) REFERENCES items (household_id, id),
    FOREIGN KEY (household_id, member_id) REFERENCES household_members (household_id, id)
);
CREATE INDEX item_viewers_member ON item_viewers (member_id);

CREATE TABLE item_reminders (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL,
    item_id uuid NOT NULL,
    offset_minutes int NOT NULL CHECK (offset_minutes BETWEEN 0 AND 40320),
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (household_id, id),
    UNIQUE (item_id, offset_minutes),
    FOREIGN KEY (household_id, item_id) REFERENCES items (household_id, id)
);

-- Concrete instances. Authoritative columns: status, board_position, completed_by, completed_at and the
-- override_* columns ("this occurrence" edits). Every other column is generated by derivation; the two sets never
-- overlap (see docs/specs/derivation-matrix.md).
CREATE TABLE item_occurrences (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL,
    item_id uuid NOT NULL,
    seq int NOT NULL CHECK (seq >= 0),
    local_start timestamp,
    local_end timestamp,
    local_due timestamp,
    due_date date,
    starts_at timestamptz,
    ends_at timestamptz,
    due_at timestamptz,
    during tstzrange GENERATED ALWAYS AS (
        CASE WHEN starts_at IS NULL OR ends_at IS NULL THEN NULL ELSE tstzrange(starts_at, ends_at, '[)') END) STORED,
    override_local_start timestamp,
    override_local_end timestamp,
    override_local_due timestamp,
    override_due_date date,
    is_detached boolean GENERATED ALWAYS AS (
        override_local_start IS NOT NULL OR override_local_end IS NOT NULL
        OR override_local_due IS NOT NULL OR override_due_date IS NOT NULL) STORED,
    disposition text NOT NULL DEFAULT 'open' CHECK (disposition IN ('open', 'overdue', 'expired')),
    status text NOT NULL DEFAULT 'todo' CHECK (status IN ('backlog', 'todo', 'doing', 'done', 'skipped')),
    board_position text NOT NULL DEFAULT 'a0' CHECK (length(board_position) BETWEEN 1 AND 64),
    completed_by uuid,
    completed_at timestamptz,
    version int NOT NULL DEFAULT 1,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (household_id, id),
    UNIQUE (item_id, seq),
    FOREIGN KEY (household_id, item_id) REFERENCES items (household_id, id),
    FOREIGN KEY (household_id, completed_by) REFERENCES household_members (household_id, id),
    CHECK ((status = 'done') = (completed_at IS NOT NULL)),
    CHECK (starts_at IS NULL OR ends_at IS NULL OR starts_at < ends_at),
    CHECK (override_local_start IS NULL OR override_local_end IS NULL OR override_local_start < override_local_end)
);
CREATE INDEX item_occurrences_during ON item_occurrences USING gist (during);

-- Per-member resolved agenda rows. Generated by derivation; visibility is never read from here.
CREATE TABLE member_agenda (
    household_id uuid NOT NULL,
    member_id uuid NOT NULL,
    occurrence_id uuid NOT NULL,
    item_id uuid NOT NULL,
    starts_at timestamptz,
    ends_at timestamptz,
    due_at timestamptz,
    due_date date,
    busy tstzrange,
    derived_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (member_id, occurrence_id),
    FOREIGN KEY (household_id, member_id) REFERENCES household_members (household_id, id),
    FOREIGN KEY (household_id, occurrence_id) REFERENCES item_occurrences (household_id, id) ON DELETE CASCADE,
    FOREIGN KEY (household_id, item_id) REFERENCES items (household_id, id)
);
CREATE INDEX member_agenda_busy ON member_agenda USING gist (member_id, busy);

CREATE TABLE scheduled_reminders (
    id uuid PRIMARY KEY DEFAULT uuidv7(),
    household_id uuid NOT NULL,
    member_id uuid NOT NULL,
    occurrence_id uuid NOT NULL,
    item_id uuid NOT NULL,
    reminder_id uuid NOT NULL,
    fire_at timestamptz NOT NULL,
    derived_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (member_id, occurrence_id, reminder_id),
    FOREIGN KEY (household_id, member_id) REFERENCES household_members (household_id, id),
    FOREIGN KEY (household_id, occurrence_id) REFERENCES item_occurrences (household_id, id) ON DELETE CASCADE,
    FOREIGN KEY (household_id, item_id) REFERENCES items (household_id, id),
    FOREIGN KEY (household_id, reminder_id) REFERENCES item_reminders (household_id, id) ON DELETE CASCADE
);
CREATE INDEX scheduled_reminders_member_fire ON scheduled_reminders (member_id, fire_at);

-- Derivation bookkeeping: pending while applied_gen < requested_gen.
CREATE TABLE item_derivation (
    item_id uuid PRIMARY KEY,
    household_id uuid NOT NULL,
    requested_gen bigint NOT NULL DEFAULT 1 CHECK (requested_gen >= 1),
    applied_gen bigint NOT NULL DEFAULT 0 CHECK (applied_gen >= 0),
    attempts int NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    retry_at timestamptz NOT NULL DEFAULT now(),
    last_error text,
    updated_at timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (household_id, item_id) REFERENCES items (household_id, id),
    CHECK (applied_gen <= requested_gen)
);
CREATE INDEX item_derivation_pending ON item_derivation (retry_at) WHERE applied_gen < requested_gen;

RESET search_path;
RESET ROLE;
