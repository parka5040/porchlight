SET LOCAL ROLE porchlight_owner;

-- Item derivation and FK cascades must not scan other series' generated rows.
CREATE INDEX member_agenda_item ON porchlight.member_agenda (item_id);
CREATE INDEX member_agenda_occurrence ON porchlight.member_agenda (household_id, occurrence_id);
CREATE INDEX scheduled_reminders_item ON porchlight.scheduled_reminders (item_id);
CREATE INDEX scheduled_reminders_occurrence ON porchlight.scheduled_reminders (household_id, occurrence_id);
CREATE INDEX scheduled_reminders_reminder ON porchlight.scheduled_reminders (household_id, reminder_id);
-- The notifier scans a time range across members.
CREATE INDEX scheduled_reminders_fire ON porchlight.scheduled_reminders (fire_at);
-- Creation order differs from publication order; stream expiry needs both.
CREATE INDEX outbox_created_stream ON porchlight.outbox (created_at, stream_seq);

RESET ROLE;
