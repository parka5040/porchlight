-- Additive recipient-local projection; NULL means a pre-upgrade row awaiting derivation.
SET LOCAL ROLE porchlight_owner;
ALTER TABLE porchlight.member_agenda ADD COLUMN disposition text
    CHECK (disposition IN ('open','overdue','expired'));
-- Existing table grants cover this generated column. The normal worker emits derived
-- events, refreshes agenda/reminders, and applies authoritative recipient-zone policy.
DO $$
DECLARE r record;
BEGIN
    FOR r IN SELECT id,household_id FROM porchlight.items WHERE deleted_at IS NULL ORDER BY id FOR UPDATE LOOP
        INSERT INTO porchlight.item_derivation AS d(item_id,household_id) VALUES(r.id,r.household_id)
        ON CONFLICT(item_id) DO UPDATE SET requested_gen=d.requested_gen+1,retry_at=now(),updated_at=now();
    END LOOP;
    PERFORM pg_notify('porchlight_derive','');
END $$;
RESET ROLE;
