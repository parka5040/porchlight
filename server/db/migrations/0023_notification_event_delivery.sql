SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Consumer-only metadata, separate from immutable producer events. Store safe
-- error classes, never raw database errors or payloads. A terminal row remains
-- inspectable even though processed_at marks the event skipped.
CREATE TABLE notification_event_delivery (
 event_id uuid PRIMARY KEY REFERENCES notification_events(id) ON DELETE CASCADE,
 attempts int NOT NULL CHECK (attempts BETWEEN 1 AND 5),
 retry_at timestamptz NOT NULL,
 last_error text NOT NULL CHECK (last_error IN ('deadlock','serialization','lock_timeout','constraint','database','database_client','invalid_data')),
 poisoned_at timestamptz,
 CHECK ((attempts = 5) = (poisoned_at IS NOT NULL))
);
GRANT SELECT, INSERT, UPDATE, DELETE ON notification_event_delivery TO porchlight_app;
-- Agent administrators can inspect skipped events without access to unsafe errors.
GRANT SELECT ON notification_event_delivery TO porchlight_agent;

RESET search_path;
RESET ROLE;
