SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Internal consumer state, not preferences: retries must not bump a user's
-- settings version or emit settings.changed. No event contents/errors are stored.
CREATE TABLE notification_delivery_retry (
 member_id uuid PRIMARY KEY REFERENCES household_members(id) ON DELETE CASCADE,
 attempts int NOT NULL CHECK (attempts BETWEEN 1 AND 6),
 retry_at timestamptz NOT NULL
);
GRANT SELECT, INSERT, UPDATE, DELETE ON notification_delivery_retry TO porchlight_app;

RESET search_path;
RESET ROLE;
