SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;

-- Zero strikes represents an isolated transient failure within the six-hour
-- event-age grace. retry_at still records when this event may be attempted.
ALTER TABLE notification_event_delivery
 DROP CONSTRAINT notification_event_delivery_attempts_check,
 ADD CONSTRAINT notification_event_delivery_attempts_check CHECK (attempts BETWEEN 0 AND 5),
 DROP CONSTRAINT notification_event_delivery_last_error_check,
 ADD CONSTRAINT notification_event_delivery_last_error_check CHECK
  (last_error IN ('deadlock','serialization','lock_timeout','statement_cancel','constraint','database','database_client','invalid_data'));
-- No relation/column privilege changes: EXPECTED_GRANTS remains unchanged.

RESET search_path;
RESET ROLE;
