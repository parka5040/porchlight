SET LOCAL ROLE porchlight_owner;

-- The board's anti-join must skip closed history when finding a series head.
CREATE INDEX item_occurrences_board_open ON porchlight.item_occurrences (item_id, seq, id)
    WHERE disposition <> 'expired' AND status NOT IN ('done', 'skipped');
CREATE INDEX item_occurrences_board_done ON porchlight.item_occurrences (household_id, completed_at)
    WHERE disposition <> 'expired' AND status = 'done';

-- Agenda membership is an OR of spans, instants and local dates.
CREATE INDEX member_agenda_end ON porchlight.member_agenda (household_id, member_id, ends_at, starts_at);
CREATE INDEX member_agenda_due ON porchlight.member_agenda (household_id, member_id, due_at);
CREATE INDEX member_agenda_date ON porchlight.member_agenda (household_id, member_id, due_date);

-- These are the same conservative UTC wall boxes used by snapshot membership.
CREATE INDEX item_occurrences_window_end ON porchlight.item_occurrences
    (household_id, (greatest(starts_at AT TIME ZONE 'UTC', ends_at AT TIME ZONE 'UTC',
        due_at AT TIME ZONE 'UTC', local_start, local_end, local_due,
        due_date::timestamp + interval '1 day')));
CREATE INDEX item_occurrences_window_undated ON porchlight.item_occurrences (household_id, item_id, seq)
    WHERE least(starts_at AT TIME ZONE 'UTC', due_at AT TIME ZONE 'UTC', local_start, local_due, due_date::timestamp) IS NULL;
CREATE INDEX item_occurrences_window_override ON porchlight.item_occurrences
    (household_id, (greatest(
        coalesce(override_local_start, local_start, starts_at AT TIME ZONE 'UTC'),
        coalesce(override_local_end, local_end, ends_at AT TIME ZONE 'UTC'),
        coalesce(override_local_due, local_due, due_at AT TIME ZONE 'UTC'),
        coalesce(override_due_date, due_date)::timestamp + interval '1 day')))
    WHERE is_detached;

RESET ROLE;
