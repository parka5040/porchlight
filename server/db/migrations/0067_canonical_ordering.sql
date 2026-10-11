-- no-transaction
-- Household-scale repair: one atomic ordering snapshot, excluding concurrent SQL
-- writers BEFORE reading it. Only noncanonical collections are rewritten (normally
-- none on 0.1.6). Commit before validation scans so constraint-add ACCESS EXCLUSIVE
-- locks are not retained during validation. Both phases are restartable.
BEGIN;
SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
SET LOCAL lock_timeout = '5s';
DO $$
BEGIN
    LOCK TABLE item_occurrences, list_entries IN SHARE ROW EXCLUSIVE MODE;
EXCEPTION WHEN lock_not_available THEN
    RAISE EXCEPTION 'Canonical ordering migration could not lock ordering tables within 5 seconds; pause agent SQL writers and retry upgrade'
        USING ERRCODE = '55P03';
END $$;
-- This is maintenance respreading, not an explicit user board move (R31).
SET LOCAL porchlight.board_mover = '00000000-0000-0000-0000-000000000000';

CREATE TEMPORARY TABLE canonical_order_repair (
    kind text NOT NULL,
    id uuid NOT NULL,
    position text NOT NULL,
    PRIMARY KEY(kind,id)
) ON COMMIT DROP;

-- Rebalance only collections containing invalid keys; include archived/history
-- rows and ties so their old C-collated position/id order is retained as well.
INSERT INTO canonical_order_repair
SELECT 'board',id,lpad(to_hex(floor(rank/4294967296)::bigint),8,'0') ||
                 lpad(to_hex(mod(rank,4294967296)::bigint),8,'0') || '80'
FROM (
    SELECT id,div(row_number() OVER w * 18446744073709551615::numeric,
                  (count(*) OVER (PARTITION BY household_id)+1)::numeric) AS rank
    FROM item_occurrences
    WHERE household_id IN (
        SELECT household_id FROM item_occurrences
        WHERE board_position !~ '^([0-9a-f]{2})*80$' OR octet_length(board_position)>64
    )
    WINDOW w AS (PARTITION BY household_id ORDER BY board_position COLLATE "C",id)
) ranked;

INSERT INTO canonical_order_repair
SELECT 'list',id,lpad(to_hex(floor(rank/4294967296)::bigint),8,'0') ||
                lpad(to_hex(mod(rank,4294967296)::bigint),8,'0') || '80'
FROM (
    SELECT id,div(row_number() OVER w * 18446744073709551615::numeric,
                  (count(*) OVER (PARTITION BY list_id)+1)::numeric) AS rank
    FROM list_entries
    WHERE list_id IN (
        SELECT list_id FROM list_entries
        WHERE position !~ '^([0-9a-f]{2})*80$' OR octet_length(position)>64
    )
    WINDOW w AS (PARTITION BY list_id ORDER BY position COLLATE "C",id)
) ranked;

UPDATE item_occurrences o SET board_position=r.position
FROM canonical_order_repair r
WHERE r.kind='board' AND o.id=r.id AND o.board_position IS DISTINCT FROM r.position;
UPDATE list_entries e SET position=r.position
FROM canonical_order_repair r
WHERE r.kind='list' AND e.id=r.id AND e.position IS DISTINCT FROM r.position;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conrelid='item_occurrences'::regclass AND conname='item_occurrences_board_position_canonical') THEN
        ALTER TABLE item_occurrences ADD CONSTRAINT item_occurrences_board_position_canonical
            CHECK (octet_length(board_position) BETWEEN 2 AND 64 AND board_position ~ '^([0-9a-f]{2})*80$') NOT VALID;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conrelid='list_entries'::regclass AND conname='list_entries_position_canonical') THEN
        ALTER TABLE list_entries ADD CONSTRAINT list_entries_position_canonical
            CHECK (octet_length(position) BETWEEN 2 AND 64 AND position ~ '^([0-9a-f]{2})*80$') NOT VALID;
    END IF;
END $$;
RESET ROLE;
COMMIT;

BEGIN;
SET LOCAL ROLE porchlight_owner;
SET LOCAL search_path = porchlight, pg_catalog;
SET LOCAL lock_timeout = '5s';
ALTER TABLE item_occurrences VALIDATE CONSTRAINT item_occurrences_board_position_canonical;
ALTER TABLE list_entries VALIDATE CONSTRAINT list_entries_position_canonical;
RESET ROLE;
COMMIT;
