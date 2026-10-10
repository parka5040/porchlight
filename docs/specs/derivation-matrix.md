# Derivation matrix

Which writes trigger which bookkeeping. **Authoritative** columns are written by the API, `porch` and raw agent
SQL; **generated** columns are written only by derivation (Rust, inside the API transaction or by the derivation
worker). The two sets never overlap, so column grants alone keep agents off generated values, and enqueue triggers
fire only on authoritative columns — derived writes can never re-enqueue. Every milestone that adds a table or
column extends this file (master plan "Definition of done").

Pending derivation: `item_derivation.applied_gen < requested_gen`. Enqueueing bumps `requested_gen`; derivation
acknowledges with `UPDATE … SET applied_gen = <seen> WHERE item_id = … AND requested_gen = <seen>`, so a request that
arrived meanwhile stays pending.

## Authoritative inputs

| Table · columns | Enqueue derivation | Revision + `task_events` | Bumps parent `version` | Outbox event |
| --- | --- | --- | --- | --- |
| `items` · `kind`, `time_mode`, `tzid`, `all_day`, `due_date`, `due_at`, `starts_at`, `ends_at`, `local_due`, `local_start`, `local_end`, `buffer_before_min`, `buffer_after_min`, `est_minutes`, `rrule`, `recurrence_mode`, `completion_interval`, `missed_policy`, `subject_member_id` | yes | yes | — (is the parent) | `item.changed` |
| `items` · `deleted_at` | yes | yes | — | `item.changed` + on transition to soft-deleted, `item.access_changed` (payload: `deleted: true`, `recipients` = former readers) |
| `items` · `series_id`, `split_from_item_id`, `split_from_seq` | yes | yes (full item snapshot) | — | `item.changed` |
| `items` · `nominal_local_anchor`, `nominal_wall_duration` | yes | yes (full item snapshot) | — | `item.changed` |
| `items` · `owner_id` | yes (agenda membership) | yes | — | `item.changed` + `item.access_changed` (payload: `previous_owner_id`, `recipients` = former readers) |
| `items` · `visibility` | no | yes | — | `item.changed` + `item.access_changed` (payload: `previous_visibility`, `recipients` = former readers) |
| `items` · `title`, `notes`, `priority`, `address`, `source` | no | yes | — | `item.changed` |
| `item_assignees` INSERT | yes | yes (`assigned`) | yes | `item.changed` |
| `item_assignees` DELETE | yes | yes (`unassigned`) | yes | `item.changed` + `item.access_changed` (removed member) |
| `item_viewers` INSERT | no | revision only | yes | `item.changed` |
| `item_viewers` DELETE | no | revision only | yes | `item.changed` + `item.access_changed` (removed member, only when visibility is `selected`) |
| `item_reminders` INSERT / DELETE | yes | revision only | yes | `item.changed` |
| `item_occurrences` · `status`, `completed_by`, `completed_at` | yes (reminders/agenda drop done instances; completion-based series get their next instance from `completed_at`) | `task_events` on `status` changes (`completed` / `reopened` / `status_changed`); a `completed_at`/`completed_by` correction alone writes none | no | `occurrence.changed` |
| `item_occurrences` · `skipped_at` (status-recording metadata, maintained by trigger, not directly writable by agents) | through `status`, not independently | included in the status event snapshot | no | through `status` |
| `item_occurrences` · `closed_at`, `board_moved` (trigger bookkeeping, not agent-writable) | through status/board writes only | no independent event | no | through status/key changes |
| `item_occurrences` · `item_id`, `seq` (trusted lineage-checked reparent only) | explicit head-before-tail derive | version/audit guards retained | no | `occurrence.changed` for each moved row |
| `item_occurrences` · `board_position` | no | no | no | `occurrence.changed` |
| `item_occurrences` · `override_local_start`, `override_local_end`, `override_local_due`, `override_due_date` ("this occurrence" edits) | yes | no | no | `occurrence.changed` |
| `household_members` · `tzid` | yes, for every non-deleted item the member owns, is the subject of, or is assigned to | no | no | `member.changed` |
| `household_members` · `tz_source`, `tz_authority_device_id` | no (unless `tzid` changes too) | no | no | `member.changed` |

### S7b collaboration (0035–0036)

| Table · authoritative columns | Enqueue derivation | Audit | Bumps parent `version` | Outbox event |
| --- | --- | --- | --- | --- |
| `comments` · `id`, `household_id`, `item_id`, `author_id`, `author_token_id`, `body`, `mentions`, `edited_at`, `deleted_at` | no | append-only `collaboration_audit` with transaction actor on INSERT/UPDATE | no; own version only | `comment.changed` (`item_id` scope) |
| `attachments` · `id`, `household_id`, `item_id`, `comment_id`, `blob_sha256`, `thumbnail_sha256`, `original_name`, `uploader_id`, `uploader_token_id`, `deleted_at` | no | append-only `collaboration_audit` with transaction actor on INSERT/UPDATE | no; own version only | `attachment.changed` (`item_id` scope) |
| `blobs` · `household_id`, `sha256`, `size`, `content_type` | no | content-addressed metadata refreshed on dedupe; agent label required | no | none |

`comments` and `attachments` have immutable identity/parent/author/uploader keys guarded by `pl_version_guard`.
Attachment hash/name fields cannot change after creation. Live blob links are validated under sorted blob UPDATE
locks by `pl_attachment_reference_clock`; soft-deleted links retain hashes without a blob FK so GC can collect bytes.
Deleting a comment through HTTP also soft-deletes its live attachments in the same savepoint. Comment body is plain
text, 1–10000 characters; HTTP rejects whitespace-only text and resolves unique live-member display-name mentions.
HTTP uses S4a's injected operation clock for the inclusive 15-minute edit deadline and deletion/edit timestamps.
Creation timestamps retain database defaults. Raw agent SQL is integrity-checked and attributed, but bypasses the
HTTP author/edit-window/mention parsing policy (the agent is a host administrator).

`emit_comment_notifications_s4b` now emits immutable `comment` (new comment → owner/assignees) and `mention`
(newly introduced mentions on create/edit) events in **one batched statement per comment**, excluding the actor,
dependents/deleted members and non-readers through `pl_item_can_read`. Dedupe key is
`comment:<comment_id>:<version>:<recipient_id>:<kind>`; payload is `{item_id, comment_id}`; no body text.
A recipient who is both assignee and newly mentioned receives both kinds; repeated mentions and unchanged body
patches do not emit. Domain savepoint rollback and idempotent replay do not leak/repeat events.
`snapshot_comments_s4b` and `snapshot_attachments_s4b` contribute live metadata in the existing `/v1/snapshot`
REPEATABLE READ transaction/cursor view. `COMMENT_SEARCH_SOURCE` shares the generated simple-language comment FTS
index used by `/v1/search`'s item/list/comment UNION in one read-only REPEATABLE READ transaction. All sources use shared SQL `VISIBLE`; Rust capability policy remains
S4a's. Search cursors bind caller/query and the bytewise `(entity_type,id)` composite key. Soft-deleted comments,
attachments and attachment parents are excluded. `comment.changed` and `attachment.changed` have entity IDs and
payload `{item_id}` only; the closed stream allow-list routes both with current authoritative item visibility.

| Generated column | Producer | Enqueue/version/event |
| --- | --- | --- |
| `comments` · `search_document` (0037) | stored generated simple-language tsvector from `body`, partial GIN | no independent enqueue/version/event; body writes follow comment rules |

Relationship rows (`item_assignees`, `item_viewers`, `item_reminders`) are immutable: change them by DELETE + INSERT.
UPDATE on them is rejected (`PL422`), so the parent bump, audit, enqueue and eviction rules above cover every change.

Item-row `item.access_changed` events carry `recipients` as an ascending array of UUID strings, computed from the
old owner/visibility and authoritative relationship rows: owner, assignees, viewers for `selected`, and all live
members for `household`. Combined changes share one event with the applicable previous-value/deletion keys.
Relationship DELETE events keep their `member_id` payload (the removed member).

## S8a calendar sources and identity

| Table · columns | Classification / writer | Enqueue derivation | Revision / version / outbox |
| --- | --- | --- | --- |
| `ics_sources` · `owner_member_id`, `target_member_id`, `kind`, `import_key`, `name`, `url`, `allow_http`, `refresh_seconds`, `enabled`, `visibility`, `deleted_at` | Authoritative configuration; app service or labelled agent SQL (identity fields immutable) | source settings propagate owner/subject/visibility/badge to live feed items and derive through the retained WriteScope (raw trigger enqueues); removal soft-deletes them and retires UID maps; fetch writes ordinary appointment inputs | Every insert/update bumps `version`, records actor + `ics_source_revisions` snapshot excluding `last_warnings`; feeds emit `ics_source.changed` with source ID and recipient `member_id` only |
| `ics_sources` · `next_fetch_at`, `last_fetch_at`, `last_status`, `last_error`, `last_warnings`, `failure_count`, `apply_timeout_count`, `etag`, `last_modified` | Generated operational fetch state; scheduler and explicit refresh enqueue | none | Same version/audit/tickle as configuration; `last_error` is a static category, including `calendar_too_large` for admission and `apply_retryable` for the first two deadline failures and `apply_timeout` for three consecutive deadline failures on identical fetched bytes. `last_warnings` contains sanitized remote UIDs/messages, capped at 50 entries × 200 total text characters per entry with an omitted count; excluded from append-only revision snapshots |
| `items` · `ics_source_id` | Authoritative feed marker (household-scoped source FK), set by the retained import WriteScope. Existing raw-agent item grants permit administrative correction; one-off imports keep NULL | no (capabilities, not timing) | Existing full item revision/version and `item.changed`; never infer read-only from editable `source` badge text |
| `ics_uid_map` · `source_id`, `uid`, `recurrence_key`, `item_id`, `occurrence_id`, `expanded_start`, `content_hash` | Generated adapter identity bookkeeping (app insert/update/delete, agent read-only) | none | Source-local typed key, no independent version/revision/outbox. Domain writes retain normal item/occurrence bookkeeping and actor; removed exceptions relinquish source ownership |
| `ics_source_revisions` · `source_id`, `source_version`, `snapshot`, `actor`, `created_at` | Generated append-only definer-trigger history | none | No recursive bookkeeping; maintenance-only physical delete |

All rows include household-scoped keys/FKs. Sources and mappings are included in normal database dumps/backups;
member-visible export (O2) must exclude subscription URL/validators/revision snapshots except household-admin export.
One-off import scopes are `(household, caller, target, import_key)`; feeds each own a source ID. Typed recurrence keys
are `D:` date, `L:` floating local, `U:` UTC instant; the master uses an empty key. Expanded identity is the nominal
instance start, not a moved DTSTART. Retained EXDATE/cancelled instances are skipped; true overrides edit the occurrence.
Retained occurrences support timing/status overrides only; differing title/notes/location retain master details and
produce UID-scoped informational `UnsupportedOverrideDetails` on every import/refresh (retained on 304;
not partial and no daily feed_error, per controller R68).
Expanded overrides are standalone items and retain their own details. Calendar planning explicitly prelocks ALL
closed occurrences of existing target items; newly introduced EXDATEs/overrides do not require an old mapping.
Controller rulings R3/R69: EXDATE means skipped even on a previously done anchor; the existing row identity is preserved.
A complete representation change reassigns keys without editing an old master as a standalone event.

Before acquiring domain/member locks, imports and feeds enforce a cumulative **4 000-unit work budget**
(controller R76 supersedes R67 sizing; R67 counting remains): one unit per parsed event, plus projected rows
and EXDATE anchors only for new/changed items, plus existing occurrence rows only for changed/restored/removed
items (deduplicated by item ID), plus **2 units per removed item** (R80/M3, see below). Mappings of items that earlier refreshes already soft-deleted are retained but charge nothing (I-new-3). Independently changed/removed retained exceptions charge their master's
projection/history; unchanged hashes do not charge mappings, stored rows or a moving horizon. A per-UID
changed-exception set is built once, avoiding a quadratic scan across all masters. Retained projections count
from DTSTART with an allocation-free bounded visitor and explicit operation clock. R12's 10 000-instance limit
is **parse/expansion only**, not an apply guarantee. The review's 12 weekly series + 200 one-offs cost 3 244
on import and 684 on a one-series-change refresh.

Removal and change at the cap (M3, same build, `--full` tests `fix3_full_cap_removal_apply` / `fix3_full_cap_change_apply`):
with removed items charged 1 unit (their occurrence row only), a full-cap removal of 3 999 standalone items took
128 005 statements / 15 728 ms (3.93 ms/item), above the 10 s sizing allowance, so a removed item is re-weighted to
**2 units plus its rows** (3 for a standalone item): the full-cap removal is now 1 333 items = 4 000 units,
42 725 statements / 3 348 ms. A full-cap change (3 units each, 1 333 items, all updated) took 70 696 statements /
6 798 ms (5.10 ms/item), inside the allowance, so changes keep their weight.

Measured here (laptop debug build, task-scoped SQLx statement execution events, non-overlapping new standalone
feed items): before cuts, 200 items took 6 638 statements and 622 ms (33.19 statements, 3.110 ms/item);
after cuts, 4 839 statements and 385 ms (24.20 statements, 1.929 ms/item). Creation inserts feed identity in
one write, skips empty relationship SQL, avoids pure reloads, and batches mappings by 500. The full chosen
cap is 2 000 new items × 2 units = 4 000 units: **48 042 statements / 4 660 ms**, or 2.330 ms/item; the subsequent full-only fixture run under concurrent
fixture load measured 5 139 ms (2.570 ms/item).
At that measured cost a 10-second allowance fits floor(10 000 / 2.570) = 3 891 new items; choosing 2 000
leaves a 1.94× cost margin. Thus a full-cap new-item apply uses at most 5.139 s here, below half the fixed 20 s
deadline. This is not a universal overlap bound: costly conflicts/changed history still have deadline rollback.
Default CI checks actual statements ≤ 12 × admitted units + 40 fixed statements + ceil(new items / 500),
and verifies 2 units per new standalone item. Large row fixtures and full-cap wall-clock measurement run
only under `ci.sh --full`, using the **production** deadline (no long-deadline admission injection).

Budget rejection preserves previous items/mappings and has problem `calendar-too-large` (422), or feed
`last_error=calendar_too_large`. The independent fixed **20-second** async apply deadline rolls back the domain
savepoint and returns an **unsealed retryable 409 conflict** (`Retry-After: 1`) on imports. Client Writes.kt
has bounded auto-retry (default three attempts, permitted 1–5), never an infinite retry loop. Only deadlines
back off feeds (5/10/20… minutes capped at refresh_seconds). **R80:** the first two *consecutive* deadline failures
of a source, regardless of the fetched bytes (publishers re-stamp DTSTAMP on every download), record `retrying` /
`apply_retryable`, without an alert; the third records **failed / apply_timeout**, emitting the usual
daily-deduplicated `feed_error`; further deadlines keep the count at 3 (CHECK 0..3) and stay failed. Previous item
state/validators remain intact. The streak (`apply_timeout_count`) resets on a successful/partial apply, a 304, a
manual refresh or a source PUT; non-deadline failures (transport, invalid or too-large calendar) neither advance nor
reset it. Manual refresh clears only the streak (it sets `queued` and the fetch time as before; `last_error` and
`failure_count` are untouched); PUT keeps its broader configuration reset.
Contention (RetryableScope, planned version conflict, 40001/40P01) retains immediate next-poll retry with **no
source state change**, including no change to the streak. SQL retains server statement timeout; no short
connection lock_timeout is introduced. Deterministic cancellation tests advance the injected clock only after
a real domain write, proving rollback, escalation, reset and replay without relying on elapsed wall time.

Feed items are appointments/busy blocks. App callers retain only View/Comment capabilities, including super admins;
agents retain existing item capabilities subject to PAT write/admin scope. Feed removal stops fetching and keeps
read-only markers on soft-deleted history; all its items are soft-deleted and UID mappings retired. Failed/malformed refreshes roll back the domain savepoint. UID-scoped semantic warnings suppress deletion/exception resets only for that UID; UID-less warnings and incomplete expansion suppress them globally. Per-event identity/item-validation failures (UID must be 1–1024 UTF-8 bytes with no NUL, including retained overrides) produce UID-scoped `InvalidEvent` warnings in feeds and one-off import summaries; invalid UIDs are skipped without rewriting/removing their old items. Any committed semantic partial apply lists bounded warnings (`last_warnings`), reports `partial`, retains validators and the normal schedule, except both `ExpansionLimited` and `ExpandedRule` clear validators to retry/extend expansion even for an unchanged file (including legacy validators). Warning truncation preserves expansion signals. `DurationNormalized`, `ExpandedRule` and `UnsupportedOverrideDetails` are informational, not partial. NUL in SUMMARY/DESCRIPTION/LOCATION is a per-event InvalidEvent. Warnings and real partial status clear on the next valid full apply. HTTP 304 keeps existing warnings/partial status. Transport/whole-calendar parse failures retain old events and back off 5, 10, 20… minutes capped at refresh_seconds (nightly by default); successful/partial/304 resets failure_count. Normal default polling resolves household-local 02:30 across DST, not creation+24h.
Absence can delete expanded instances **only inside the exact parse window** `[now - 180 days, horizon_end)`.
App-capable member-targeted items are owned by that target (default private). R49a dependent targets use the importing caller
(feed: creating super admin) as owner, subject_member_id = dependent, default household visibility. Household feed items
are owned by the creating super admin with default household visibility and **no assignees**. R2 agenda membership follows
owner/subject, not household visibility: later joins need no feed synchronization and can read the household calendar;
a member-target feed is how a calendar becomes someone's busy time. Feed visibility is source-controlled and maps directly
to item visibility; API enum, CHECK and raw trigger accept exactly `private | household`, never selected/explicit.
Import scopes retain their independent 1–200 character/no-control limit. Source names are limited to 196 characters so the `ics:` badge fits item validation (API 422 name field error; SQL CHECK). Source setting changes apply only to live feed items (soft-deleted history stays unchanged). PUT resets validators, failure_count/backoff, last_error and last_warnings and queues an immediate fetch. Agent-added assignees survive rename/visibility/URL/interval changes, including the raw config trigger; target changes clear assignees to follow the new subject/owner agenda. A changed remote event also resets feed relationships to the source defaults; unchanged refreshes preserve agent corrections. A removed creator disables
fetching with member_removed; recreate the source under an active admin. Normalized content hashes prevent unchanged refreshes/reimports from bumping item/occurrence versions, revisions, outbox or EXDATE skipped_at. Import request visibility affects only new editable items and is not in the content hash. Feed badge/name changes are applied by configuration, not the content hash (no double write on refresh). Shared calendar_changed producer receives only changed scope outcome IDs, never unchanged summary IDs; it emits one batch for live readable owners/assignees with ABI business self suppression. Failed/partial feed status emits feed_error to live super admins via the source trigger, deduped per source/recipient/UTC fetch date. Import preview rolls back all domain/source/mapping/revision/outbox effects, retaining only the wrapper's replay record.

## Generated outputs

| Table · columns | Written by | Outbox event |
| --- | --- | --- |
| `item_occurrences` · `item_id`, `seq`, `local_start`, `local_end`, `local_due`, `due_date`, `starts_at`, `ends_at`, `due_at`, `disposition` (and the generated `is_detached`, `during`) | derivation (rows inserted, updated or deleted) | `derived` (item, applied gen) |
| `member_agenda` (all) | derivation | `derived` |
| `scheduled_reminders` (all) | derivation | `derived` |
| `item_derivation` · `applied_gen`, `attempts`, `retry_at`, `last_error`, `calendar_horizon` | derivation; enqueue triggers bump `requested_gen` and reset `attempts`, `retry_at`, `last_error` | `derived` on apply or terminal error |
| Conflict and reminder notifications | S6 (using S3 conflict outcomes/reminder rows), deduplicated per (occurrence, member, kind) | `notification.created` |
| `scheduler_jobs` · `name`, `last_run_at`, `last_success_at`, `next_due_at`, `last_error`, `attempts` | scheduler job registration/runner, app read/insert/update; agent SELECT only; generated operational state (no domain revision, parent version or business outbox) | none |

### S7b generated and operational state

| Table · columns | Written by | Outbox event |
| --- | --- | --- |
| `comments`, `attachments` · `version`, `created_at`, `updated_at` | version guard / database defaults; clients and agent column grants cannot set them | parent row's collaboration event |
| `collaboration_audit` · `id`, `household_id`, `entity_type`, `entity_id`, `action`, `actor`, `created_at` | definer collaboration trigger; append-only; app/agent SELECT only | none (source row emits) |
| `blob_storage_locks` · `shard` | migration seeds 256 fixed rows; HTTP/GC lock only; no deletion or domain effect | none |
| `blobs` · `created_at`, `last_referenced_at` | database default / ingestion upsert / reference trigger; orphan-adoption function may seed old unreferenced metadata for GC | none |
| `blob_leases` · `id`, `holder`, `created_at`, `expires_at`; `blob_lease_items` · `lease_id`, `household_id`, `sha256` | `pl_take_blob_lease` definer captures current live main/thumbnail references, max 24-hour expiry; backup role/app release, daily GC expiry cleanup | none |

The API updates the parent comment `edited_at` on comment attachment INSERT and soft-delete, bumping its version
and collaboration event. This version bump is API-only: raw SQL attachment inserts/deletes do not bump the comment; it never bumps the item. Unchanged-body PATCH is a no-op. Attribution token FKs use SET NULL on purge,
while immutable attribution remains enforced on ordinary writes; purged PAT comments retain body/audit history.

Daily `blob_gc` runs at 03:30 in the household zone with an injected scheduler clock. It collects blobs with no live
main/thumbnail reference, no active lease, and a `last_referenced_at` at least 30 days old. Old canonical orphan
files from rolled-back uploads are adopted only for collection; old `.tmp` files are removed, never downloaded.
Uploads are durable before attachment metadata commits (same-directory temporary write, file fsync, atomic rename,
and shard directory fsync). A failed DB commit may leave an unreachable orphan, not a downloadable partial blob.
O2 owns export/backup-bundle integration and lease acquisition/release around the dump/copy; the lease function and
backup-role grants are available here, but the existing O1 job has not been changed in this workstream.

## S3 persistence and job semantics

- A split's head keeps its ID and BOTH returned specs are stored verbatim. The tail has `series_id` = lineage root,
  `split_from_item_id` = its parent and `split_from_seq` = the parent sequence; tail sequences restart at 0. A split at
  sequence 0 keeps the original item (no empty head/duplicate series). Composite FKs keep lineage within a household.
  Migration 0012 permits only lineage-checked reparenting of occurrence keys: head rows at/after the split move to
  tail `(item_id, seq - at_seq)`, keeping IDs, status, board position, completion and overrides. This authoritative
  scope operation removes moved rows' agenda/reminder projections under held locks, then explicitly derives head BEFORE tail regardless of UUID order; it does not change raw enqueue trigger column lists or agent
  grants. Occurrence version/audit guards remain active; arbitrary key edits still fail PL422.
- Nominal metadata is optional only for fixed **timed** calendar tails whose resolved start/due OR end loses nominal wall
  time. `nominal_wall_duration` is an exact positive **bigint microsecond length**, never a month-bearing interval;
  deadline metadata has no duration. Rehydrate every row with `SeriesSpec::new_with_calendar_cursor`. Clear BOTH
  values on all-edits changing timing anchors, kind, zone, time mode, all_day, rule or recurrence mode. Non-timing
  edits retain metadata. Core rejects raw fixed all-day rows on skipped local midnights: API validation is 422;
  raw SQL reaches terminal `last_error` instead.
- The 18-month horizon bounds ONLY calendar recurrence expansion. Non-recurring items always materialize seq 0,
  including undated todos and arbitrarily distant dated appointments; completion series always materialize the
  first/current successor independent of the horizon. Timing edits retain occurrence IDs.
- `item_occurrences.skipped_at` is status-recording metadata maintained by `pl_record_skip`: synchronous app operations
  supply their injected operation time; raw-SQL status transitions record `statement_timestamp()` (not transaction
  start, derivation time or the skipped anchor). Agents cannot directly write the column. Every transition INTO
  skipped records a fresh timestamp, even after reopen; app timestamps are injected. Historical pre-0012 skipped rows
  use `skipped_at = updated_at` as an approximation because the original instant cannot be reconstructed.
  Completion successors use the LATEST closure time (max completed_at/skipped_at among closed rows, sequence as
  tie-breaker), not the highest sequence. Skips without completed_by use the household zone. Reopen retains the
  original anchor and `closed_at` marker; an open row's old closure is not considered closed history.
  **R31:** reopening the latest closed occurrence removes its untouched open successor (todo, no overrides,
  not detached, never board-moved). A touched successor retains identity/anchor, leaving two open occurrences;
  this is the explicit exception to the spec's exactly-one-open rule. `board_moved` remains true even after a move
  back to the default key. Automatic tie-respread bystanders are NOT board-moved. Latest uses `(closed_at, seq)`
  for both N1 and R31, including the reopened row's retained closure instant for R31; reclosed older rows can be latest.
  An older reopened closure can coexist with its successor until reclosed.
- **R63 carry-over contract (Ruling R63).** Carry-over guarantees are invariants over current persisted state,
  not identity under history. `core/tests/items_carry_model.rs` checks them after every operation (P1–P8 probes;
  256 fixed-seed and 256 random-seed operation sequences per run). The engine must also equal the rule below, ignore
  input order and be idempotent. Skip and stack stay stateless; disposition stays generated.
  - Invariants: (1) at most one open row is overdue. Exactly one is overdue while some missed open row is not
    covered (below). (2) Closing carried work never resurrects the peers it covered. (3) A rescheduled row is judged
    by its effective boundary. Moving younger work never displaces the carried row, and moving work back never
    yields a second overdue. After a move, nothing is overdue only while postponed work covers every missed row.
    (4) Switching into carry_over yields exactly one overdue unless every missed row is covered. (5) The carried row
    is the deterministic function of current rows below.
  - Terms. Boundaries are missed boundaries (effective = overridden). A detached row's generated boundary uses the
    same zone/DST/R10 resolution. Due-since is the earlier of the generated and effective boundary, with sequence
    breaking ties. Postponed means detached later than its generated boundary, once that boundary has passed.
    Retained means a missed open row persisted overdue. A claim is a retained or postponed row.
  - Coverage: missed rows that expire whatever their rank.
    - Closure: a closed row persisted overdue (closed carried work) covers rows whose boundary precedes its
      closure, refreshed or not. A closed row persisted open that was postponed at closure (moved carried work
      closed early) also covers them, except older protected work (below). Retained rows are never closure-covered:
      reopened carried work, stack-era overdue rows. An expired closure is not evidence, because skip expires
      statelessly.
    - Postponement: open postponed work that is not closure-covered covers missed rows with an earlier boundary
      (equality stays eligible). Older work that is postponed or not expired is protected and is never covered by
      younger postponed work. Coverage is therefore transitive.
  - Election among uncovered missed rows: claims first, oldest due-since. Otherwise non-expired rows, then expired
    rows, each by latest boundary and then lowest sequence.
  - Why this rule:
    - Retaining the overdue row implements "one overdue instance persists until done" (A1, Doing cards included).
      It also makes refresh cadence irrelevant once a row is elected.
    - After the move's own derivation, postponement is the only persisted evidence that carried work was moved:
      the moved row is then Open.
    - Ranking claims by due-since keeps older carried work when a younger instance is postponed (P1, P6–P8, NB-1).
      It also lets the moved row win its new boundary against siblings at the same or a later boundary, with or
      without an intermediate refresh (P3, P3b).
    - Non-expired before expired keeps a carried row moved forward and back (P2, P5) instead of reviving
      superseded peers.
    - The latest boundary keeps the established initial latest-only election; ties go to the lowest sequence (R63).
    - Both derivation and clock refresh pass anchors, persisted disposition and closure markers from rows they
      already load: no added statements.
  - Documented choices where histories are indistinguishable:
    - Postponing older superseded work hands it the carry. It covers younger missed work, so nothing is overdue
      until its new boundary, then it carries. This state is identical to a postponed carried row whose younger
      peers a stack era made overdue, and that case must stay zero-overdue (DB regression).
    - A stack era erases supersession. Its overdue rows are retained claims, so returning to carry_over keeps the
      oldest (P7). After stack→skip the rows are expired, and earlier closures cover them (N-2), exactly as after
      carry-over supersession.
    - A future instance postponed before its generated time counts as postponed work once that time passes. It
      covers younger missed work, but never older retained or non-expired work.
    - When several rows become missed between two derivations with no claim present, the latest is elected. An
      intermediate derivation would have retained the earliest.
    - Round 3's identity-exact model is withdrawn. Its proof histories now reach identical inputs and carry seq0
      by rule.
- Occurrence regeneration upserts generated anchors only, preserving detached/completed/skipped anchors and all
  authoritative state. Open dispositions are recomputed even for detached rows. Local overrides resolve with the
  shared R10 `resolve_span`; collapsed/inverted gap spans keep resolved start plus nominal wall duration.
- `board_position` defaults to `80`. Migration 0012 rebalances every household board containing any noncanonical
  legacy key in `(status, board_position COLLATE "C", id)` order. Every runtime board order is
  `(board_position COLLATE "C", id)`, so equal-key inserts remain deterministic. Runtime moves rebalance only the
  minimal contiguous tied/exhausted run INSIDE the target status's household Board projection domain (next open
  occurrence per series, plus Done completed in the last seven days). Visibility never narrows the domain; future
  occurrences and expired Done keep their keys. The free board wrapper uses its injected `now` throughout planning,
  rechecks and the move; direct WriteScope begin captures server time once (its public API is unchanged).
  Only the mover is marked touched/force-updated; respread bystanders update and emit only on actual key changes.
  `move_on_board(occ,before?,after?)`: before means immediately AFTER
  that card (using its next domain neighbour), only after means immediately BEFORE that card, neither means end.
  Any ordered pair is accepted; after validates order but does not select the insertion gap when before is supplied.
  Board-only moves lock only mover + rewritten run NKU, never parents/members/jobs/peers and never derive.
  Neighbourhood-only revalidation uses at most three acquisition attempts, then typed `RetryableScope` (S4 transient 409).
- Enqueue increments `requested_gen`, resets attempts/retry/error and NOTIFYs `porchlight_derive` (payload is only a
  wake-up hint). Worker candidates are unlocked, oldest due first, at most 50, with attempts < 5. Item-first
  `FOR UPDATE SKIP LOCKED`; derive in a savepoint; failure retries at `min(5 s × 2^attempts, 15 min)`; the fifth
  failure emits a terminal event, leaving pending until repaired/re-enqueued. Derivation failures record sanitized
  last_error and warn with ID/attempt/category. Acquisition contention rolls back only its candidate, logs a redacted
  category at DEBUG for 55P03 only (no warning storm or retry-state mutation), and retries on a later batch.
  Unexpected acquisition errors are redacted WARNs, never database-message logs. Worker locks only open
  occurrences and affected authoritative/stale-agenda members, not every member or closed history. Full batches drain immediately.
- Every successful derive emits `derived` with `{item_id, applied_gen}`. Terminal errors additionally have
  `error: true` and the still-applied generation. The app cannot insert outbox directly: `pl_emit_derived` is the
  narrow definer entry point. Stream delivery/replay filters `derived` with authoritative item ACLs.
- Synchronous composable writes use one transaction-only `items::WriteScope`. Affected members include old/new core
  membership AND existing target agenda membership; pending peers are found through current core relationships OR
  stale agenda rows. Membership is assignees + subject, with owner only as the empty-set fallback; viewers/visibility
  never grant agenda membership. Locks cover only targets + pending peers and open/edited occurrences, then jobs,
  then affected members; see `lock-order.md` for acquisition-savepoint restart. `finish` isolates each peer in its
  own savepoint: failures log only ID/category, remain pending with retry/last_error unchanged, and stale agenda is
  excluded from conflicts. Terminal peers (attempts >= 5) are neither locked nor redrained; they are also excluded.
  Dirty targets derive once, followed by the final conflict computation. A transaction-local marker rejects a second
  begin even after finish; finish consumes the scope, so using it afterward is impossible in Rust. Errors require
  rollback of the full domain savepoint; free wrappers are never composable. `set_time_zone` / `make_authority` retain
  injected `now` signatures but now enqueue only, like device reports: no occurrence/co-member/peer locks or synchronous
  derivation. Enqueue-only requests reject item edits, occurrence edits/reopens, splits, board moves and extra members;
  their finish seals the transaction marker only. S5 must complete WriteScope work BEFORE device revocation or use separate transactions
  because the revocation trigger locks the member row.
- Daily `extend_horizon` (02:30 current household zone, UTC before household initialization) and five-minute
  `refresh_dispositions` enqueue using the same core engine. Horizon candidates must be live calendar series,
  current/non-poisoned, with their latest generated anchor before the new end AND their persisted `calendar_horizon`
  before it. Expansion confirms there is a new sequence: completed COUNT/UNTIL series and split heads do not enqueue.
  The conditional derive ack records calendar_horizon. Refresh computes core dispositions inside independent
  per-item SAVEPOINTs; bad overrides log only ID and leave the worker's bounded retry/backoff untouched. Every item's
  transaction commits/releases locks before the next. One extra one-slot operational pool prevents a scheduler
  election from starving item transactions even with a one-connection request pool. Generic callbacks retain atomic
  effects/bookkeeping; built-in sweeps intentionally retain earlier successful items on interruption and are
  idempotent on retry. Failed jobs use min(5s × 2^attempts,15min), not every poll. `running_since` was removed because
  uncommitted job-row election made it permanently invisible; observers see committed last_run/success/error/attempts.
- Lineage indexes support both composite FKs. Physical purge (S5) deletes dependent occurrence/relationship/derived
  children and history, then notification_events just before items, then descendant tails before heads/roots; clear lineage on survivors if retaining them. Self-root references
  may be cleared before purge. No cascading domain deletion is granted to app/agent roles.
- Device TZ reports first read state unlocked; a no-op takes no row lock. Changes commit in a separate best-effort
  authenticated transaction before any mutation claim, with device NKU/recheck then an enqueue-only member-zone scope
  (related items/jobs, INCLUDING soft-deleted references for undelete safety, then zone member UPDATE; no occurrence,
  co-member or pending-peer locks). Zone UPDATE blocks new FK references before the unlocked recheck and trigger.
  Explicit
  member mutations use that scope and isolated finish; the unsafe `members::derive_pending` API is removed. Migration
  0012 clears a revoked device's authority (member.changed, no tzid change/enqueue); the next live reporter can claim
  it. Dependents' effective zones always come from the household. S4 household-zone changes must enqueue affected
  dependent items (no household settings writer exists in S3).
- All new tables/columns are automatically included by full database dumps/backups; no selective export projection
  is introduced here. Item revisions use `to_jsonb`, so new authoritative fields are captured automatically.

## Ordering and gaps

- Global [lock order](lock-order.md): idempotency claim → device → session → user credentials → items → occurrences → derivation → affected `household_members` rows `FOR NO KEY UPDATE` in id order (R7; the app role cannot use advisory locks); never `households … FOR UPDATE`. Domain order remains items → item_occurrences → item_derivation (see the master plan's derivation contract).
- `stream_seq` is publication order (assigned after commit), not producer commit order, and may have gaps.
- S6b migration 0025 adds no columns/tables. `pl_item_readers` excludes deleted/dependent members;
  `pl_list_can_read(uuid,uuid)` is a fail-closed stub only when the S7 signature is absent. Item/list
  former-reader tickles remain deliverable after access loss/soft deletion, without entity content.
  Delivery/replay scopes, the seven-day replay window, session sliding and drain: [stream.md](stream.md).
- Member removal, role and display changes are modelled in the S5 lifecycle section below (including raw SQL); settings are implemented by S4b.
- M2 trust tables have no derivation inputs (credentials, devices/certificates, enrollment/reset tokens, sessions, PATs, step-up grants, idempotency records and install state).

## O1 operational backup metadata

`backup_runs` is operational/generated, not a domain mutation. The backup job alone INSERTs/UPDATEs `id`,
`started_at`, `finished_at`, `status`, `bundle_file`, `size`, `sha256`, `error`, `next_due_at`. UUID/time defaults
are generated by PostgreSQL; job output/scheduler produce the remaining columns. No derivation, domain revision,
actor, parent version or outbox tickle applies; app/agent are SELECT-only, backup role has pg_read_all_data plus
INSERT/UPDATE of this table. Rows are included in dump, manifest table counts and restore. O2 owns read-only
status endpoints/CLI/client plumbing and export/retention integration. A restored in-flight row is historical metadata,
not proof of a currently running process; only `status='success'` drives persisted catch-up/next-due decisions.

## S8b availability and planning

| Table · columns | Class / writer | Derivation and bookkeeping |
| --- | --- | --- |
| `availability_windows` · `household_id`, `member_id`, `weekday`, `starts_local`, `ends_local` | authoritative; API/raw agent | version guard; member NKU NOWAIT (0046; whole-transaction retry on 55P03); statement-level per-member profile bump (0048); planning_audit actor; member_load_jobs enqueue; member.changed with member_id scope |
| `availability_profiles` · `member_id`, `household_id`, `version`, `created_at`, `updated_at` | collection-version bookkeeping; API/definer | profile bumps for replacement including empty; actor audit and member-scoped outbox |
| `schedule_suggestions` · IDs, `proposed_start`, `proposed_end`, `run_id`, `created_at` | generated by nightly/on-demand EDF | version guard; planning_audit actor; suggestion.changed with member_id scope; no item enqueue |
| `schedule_suggestions` · `status` | authoritative accept/dismiss; raw agent | version guard/audit/outbox; terminal status transition guard (0047); acceptance separately writes occurrence local overrides through WriteScope |
| `planning_audit` (all) | append-only definer audit | actor captured by pl_current_actor; maintenance purge only |
| `member_load_jobs` · `requested_gen`, `updated_at` | enqueue trigger/function | availability and member tz changes enqueue; no item locks |
| `member_load_jobs` · `applied_gen` | member-only worker / synchronous recompute | conditional ack of captured generation |
| `member_load_jobs` · `attempts`, `retry_at`, `last_error` | member-only worker retry state | safe category, backoff, poison after five attempts; authoritative enqueue resets; deleted jobs acked |
| `member_load_signals` · `id`, `member_id` | immutable peer wakeup; definer insert, member-only worker delete | fresh UUID insert-if-absent for unheld peers; no shared queue-row upsert; folded by worker under member lock |
| `member_daily_load` (all) | generated core daily_load projection | once after scope peers/targets converge, only held members and changed local dates; unheld peers queued; availability/worker full refresh; excludes pending/poisoned tasks; empty windows mean untracked |

Occurrence local start/end overrides may now schedule todo/deadline work while retaining its kind/deadline.
0047 rejects half-span overrides by DB CHECK for due-only task anchors (S4a/C6 must retain the complete pair);
appointment anchors keep inherited-bound partial overrides. Generated
member_agenda then includes a buffered busy span for that task; this is the same occurrence input/output split and
enqueue trigger, not a new item field or bypass of WriteScope. Load counts scheduled start day before deadline day.
Notification producers use migration 0015 pl_emit_notification_event; ONE statement over enter/leave
(member,date) transitions only in member-local [today,today + 14 days], no events for untracked members (R47).
Full dumps/backups include all seven new tables (purge member_load_signals before members); O2 member export should include availability + suggestions, not queue/audit.
See [planning.md](planning.md) for wire/version semantics and the 90-day EDF horizon.

## Lists (S7a, migration 0030)

These tables never enqueue item derivation or alter agenda/conflicts. Attached lists inherit authoritative item
visibility/capabilities; standalone lists use owner + household/private/selected + selected viewers (subjects and
assignees are not standalone relationships). Standalone readers may manage/check entries; metadata edit, reassign,
delete and visibility/viewers remain owner/super-admin-only (R51). App super admins follow visibility.
Agents still require a label for every raw write. Attached visibility is constrained to household (inherited).
Owners/viewers cannot be dependent or deleted members; per-occurrence creation requires a recurring item.

| Relation | Authoritative columns | Generated/bookkeeping columns | Side effects |
| --- | --- | --- | --- |
| `lists` | `household_id`, `item_id` (nullable, immutable), `owner_id`, `name`, `mode` (immutable), `visibility`, `deleted_at` | `id`, `version`, `created_at`, `updated_at` | Snapshot/actor in `list_revisions`; scoped `list.changed`. Standalone ACL shrink/deletion records prior recipients for eviction. |
| `list_viewers` | `household_id`, `list_id`, `member_id` (immutable rows) | none | Parent version/revision bump; removed selected viewers receive an eviction tickle. Attached lists cannot own viewers. |
| `list_entries` | `household_id`, `list_id` (immutable), `label`, `qty`, `unit`, `url`, `notes`, `position`, `archived_at` | `id`, `version`, `created_at`, `updated_at` | Snapshot/actor in `list_entry_revisions`; scoped `list_entry.changed`. Entry writes do not bump their parent list: their own version is authoritative. |
| `list_entry_checks` | `household_id`, `entry_id`, `occurrence_id` (nullable), `checked_by`, `checked_at` (immutable rows) | none | INSERT/DELETE bumps entry version/revision, snapshots checks, emits entry tickle. Persistent NULL is unique; per-occurrence requires a valid parent occurrence at insertion. |
| `list_revisions` | none (append-only history) | `id`, `household_id`, `list_id`, `rev`, `snapshot`, `actor`, `created_at` | One snapshot per list version; includes viewers. Maintenance-only deletion. |
| `list_entry_revisions` | none (append-only history) | `id`, `household_id`, `entry_id`, `rev`, `snapshot`, `actor`, `created_at` | One snapshot per entry version; includes checks. Maintenance-only deletion. |

`pl_list_can_read(list_id, member_id)` excludes deleted lists, parents and members, and dependent app members.
Outbox payloads have `item_id` for attached lists/entries or `list_id` for standalone lists/entries. Standalone
`list.changed` eviction tickles also carry `eviction: true` and `recipients`; delivery must recheck the current ACL.
Inherited item eviction must evict attached list caches too. Trigger versions/created/updated timestamps are DB-owned;
HTTP uses its injected clock for checked/deleted/archived instants. Per-occurrence checks deliberately have no occurrence
FK: regeneration can delete/recreate open occurrences without destroying revision/check history. Each fresh occurrence
has independent checks. R52: HTTP and raw check validation traverse S3's `items.split_from_item_id` descendant
chain, so a list attached to a head follows split tails, including their existing checks; sibling/head occurrences
are not valid for a tail-only list. Filtering lists by a tail returns its ancestor per-occurrence templates.
No reparenting or copying of lists is needed. Template ACL and outbox scope remain the original parent item.
Persistent checked entries render under Got it, and clear archives them (it does not erase history). Clear's list
If-Match protects metadata/ACL only, not the checked-entry set; checks bump entry versions, and clear archives all
checked entries at execution time. Empty/same-value PATCH and empty clear emit nothing.
Attached lists with deleted parents are excluded from default collection/snapshot/search callers, including agents;
per-row Rust ACL rejections are skipped, not propagated as a page error. The dedicated `include_orphaned=true`
collection filter exposes cleanup candidates only to their list owner/super admin, with view/delete capabilities.
Such lists remain versioned-deletable (204); parent deletion does not cascade.

Backup's complete `pg_dump -Fc` includes all six tables; O2's export and purge integration must include viewers, checks,
entry revisions then entries, and list revisions then lists. Shared snapshot calls `lists::snapshot_contribution`
on its existing RR connection; shared search UNIONs `lists::SEARCH_SOURCE` before applying signed source/id keysets.
Stored generated simple-language `lists.search_document` (name) and `list_entries.search_document` (label/notes),
with live-row GIN indexes, are search projections only; they never authorize. `pl_list_can_read` delegates attached
visibility to `pl_item_can_read`. Series lineage queries use household-indexed joins and explicit CYCLE guards.
Checks intentionally produce no notification event: S7a registers no business kind in R53's data registry.
List and entry revision snapshots exclude the derived `search_document` tokens.

## Planned rows

- **M5:** `notification_prefs` default reminder times → enqueue every item that uses defaults for that member.
- **M6:** lists are implemented below; comments/attachments belong to S7b.
- **M7:** availability windows → enqueue load-conflict recomputation for that member; ICS-sourced items follow the
  `items` rows above.

M2 Trust fix: removing an `item_viewers` row emits `item.access_changed` with `member_id` only when the item is `selected` at removal time; a private/household viewer row grants no selected ACL. Assignee removal evictions are unchanged.

## S4b reads and notification producers (0015)

| Table · columns | Authority / writer | Enqueue / version / outbox |
| --- | --- | --- |
| `items` · `search_document` | generated stored tsvector from title/notes/address, simple config; GIN | no independent enqueue/version/event; input writes follow existing items rows |
| `member_settings` · `settings` | authoritative Today document; self API PUT, existing agent JSON object write | no item derivation; version guard; `settings.changed` private payload member_id |
| `member_settings` · `member_id`, `household_id` | immutable keys; new-member definer seeds defaults, existing rows preserved | insert version 1 and `settings.changed` |
| `notification_events` · `household_id`, `recipient_member_id`, `kind`, `item_id`, `occurrence_id`, `entity_type`, `entity_id`, `payload`, `dedupe_key` | producers via pl_emit_notification_event only | append-only immutable per-write notification inputs; no item enqueue/version/outbox |
| `notification_events` · `id`, `actor_member_id`, `actor_label`, `created_at` | generated UUID/time + actor captured by definer | actor/agent attribution; household/dedupe_key unique; business self suppression |
| `notification_events` · `processed_at` | generated operational state, app column UPDATE reserved for S6 | no independent event; S6 owns notifications/outbox |
| `notification_kinds` · `kind`, `self_suppressed`, `description` | authoritative static registry (R53); owner migrations only, app/agent SELECT | FK validates producer kinds; suppression flag read by producer; no item enqueue/version/audit/outbox |

No notification delivery or notification rows are created by S4b. Every new table/column is included in full
database dump/restore/backup, and snapshot is a read projection, not a selective export. The common authoritative
ACL `pl_item_can_read` is applied to search, snapshot, agenda, board, scheduled-reminder caches, conflict masking
and stream delivery. Settings shape, cursor/cache bounds, producer SQL ABI and kinds: `items-reads.md`.

## HTTP write service (S4a)

S4a adds no authoritative/generated database columns or tables; migration 0013 is unused. Item, relationship and
occurrence routes write the existing inputs above through one retained `items::WriteScope` inside the S1 IdemTx
transaction. A domain savepoint rolls back ALL partial writes/derivation on any error before a business problem is
sealed. RetryableScope and SQL lock/serialization failures use the unstored transient 409 path. Domain clocks are
injected; `pending_derivation` is computed from requested_gen > applied_gen, not a new persisted flag.

Capabilities are computed from the authoritative item ACL in stable core order, with one default-no-op capability
filter for S8 feed items. GET item/list/occurrence reads use a repeatable-read read-only snapshot. Conflict responses
mask inaccessible counterpart titles AND both item/occurrence ids; derived membership never authorizes reads.
R37 status no-ops (complete done, reopen any open state, PATCH current status including skipped) return the current
resource without a scope, version bump, closure restamp, events or enqueue. Composed override/board writes suppress
only the unchanged status. All HTTP scopes use the same injected `begin_at`/mutation/finish clock. Signed occurrence
pages honor limit and bind cursors to caller, item, window and resolved zone; floating filters prefer Porchlight-TZ.
Collection ACL/relationship/pending assembly is batched; occurrence pages share parent metadata without per-row SQL.
`this_and_future` validates/applies timing against the re-anchored tail; `all` on a split tail edits only that series. Board-only moves retain
S3's occurrence-only footprint, map wire after_id to the predecessor / before_id to the successor, and recheck
capability after scope acquisition/finish without taking late parent locks. A successful transfer that removes the
writer's access, or a reopen that regenerates away a rule-removed historical occurrence, returns an empty 204.

## S5 lifecycle, revert and retention

| Authoritative writer / inputs | Generated output / events | Writer and invariant |
| --- | --- | --- |
| Member INSERT; UPDATE display_name, role, color, deleted_at | `member.changed` outbox tickle (member id only) | Column-scoped lifecycle triggers; covers API + raw SQL, suppresses identical updates. Existing zone/source/authority events remain independent. |
| User deletion | devices/certificates/sessions, PATs and enrollment tokens revoked; member authority cleared | S5 transactional device → member sequence; owned items stay. Role changes do not alter item visibility grants (no `item.access_changed` eviction); `member.changed` instructs clients to refresh role/capabilities. |
| Revert revision authoritative fields, deleted_at, series_id, split_from_item_id, split_from_occurrence_id, nominal anchors, assignees/viewers/reminders | new revision/agent actor, version, occurrences, agenda, reminders, conflicts, ack and outbox | Restored through WriteScope; never replaces generated rows from an old snapshot. A stale post-lock recheck includes current resource. A head with surviving split descendants rejects timing/nominal-anchor restoration with 422 `scope` validation, before writing. Soft-deleted tails count as surviving; their authoritative state remains. Same-timing metadata/relationship reverts are supported. A single-item snapshot is not a lineage-wide split undo; unsupported undo fails closed rather than duplicating tail work. |
| Purge selection (household + soft-deleted item ids) | `purge_log.id, household_id, actor, item_ids, counts, created_at` | Maintenance-owned definer generates every audit column. No item FK; survives deletion; append-only incl. TRUNCATE. Counts are the pre-delete snapshot, excluding history generated by deletion triggers. |
| Purge item dependents | assignees, viewers, reminders, occurrences, agenda, scheduled reminders, derivation, revisions, task and notification events removed | No runtime DDL/GRANT (R55); notification SELECT/DELETE declared in 0016 and EXPECTED_GRANTS. |
| Attached list dependents | lists, entries, viewers, checks (incl. occurrence-linked checks), list/entry revisions removed and counted | Children before parents; no silent cascades. Maintenance SELECT privileges are declared after 0030 in assigned migration 0060 (R58), with DELETE from 0030; EXPECTED_GRANTS lists both. Positive purge tests use these real migration grants, never a test-only fixture. |
| Derive retry | attempts=0, retry_at=injected now, last_error=null, worker tickle | Item NKU before job write. Typed response reports queued only when an existing job has applied_gen < requested_gen; missing/acknowledged jobs report false. |

| Calendar identity map (`ics_uid_map`) | Count and explicitly delete selected items' map rows before occurrences/items | Its composite item FK is NO ACTION; rows cannot be nulled (`item_id` NOT NULL). Source records and append-only source revisions are independent and retained. No occurrence FK (adapter re-resolves keys). SELECT/DELETE is reaffirmed in 0060 and EXPECTED_GRANTS. |
| Planning proposals (`schedule_suggestions`) | Count and explicitly delete selected items' proposals (all statuses) before occurrences/items | Its composite item FK is NO ACTION; item/occurrence references are NOT NULL. No occurrence FK (regeneration may remove instances). Existing planning_audit survives; deletion triggers append a new actor-attributed DELETE audit/tickle. SELECT/DELETE is reaffirmed in 0060 and EXPECTED_GRANTS. |
| S6b stream (`0025`) | No item/occurrence FK added; no additional purge relation/count | Transactional outbox survives item deletion as already modelled; authoritative reads filter delivery. |

### R56b (extends R56a): purge extension procedure (O2 owner)

`pl_purge_relations` is the single private retention plan: relation names, exact row selections and deletion
order drive locks, catalog inspection, pre-delete counts and explicit deletes. The owner probe computes the
transitive FK closure seeded by every existing deletion relation using `pg_constraint` (cycle-safe UNION), then inspects **every incoming FK
into every relation this plan deletes from**, regardless of child schema or ON DELETE action. This covers
notification_events, planning proposals, list parents and deep list history, not just items/occurrences.
Its handled set matches relation, ordered source columns and ordered referenced columns, not FK names.
A known FK is safe only when its dependent rows are also in the explicit child deletion selection; new FK
shapes on a handled table remain unknown. **Selected rows in an unhandled relation** return mapped 409
`purge-dependency-unhandled`, naming the relation in `detail`, before any domain deletion or purge audit.
All domain/history/audit rows remain unchanged. Empty relations and unrelated rows do not block purge;
CASCADE and SET NULL never silently remove or sever selected unhandled dependents. The probe locks selected
rows in **every deletion relation** (sorted primary keys, items → occurrences → derivation first) before
inspection, preventing concurrent FK insertion into any deletion parent, including notification/history rows.

`pl_purge_unhandled_relation` is a private, owner-owned SECURITY DEFINER catalog probe (row locks only, no data writes) (fixed
search_path; identifier-quoted joins built from FK attributes; only maintenance has EXECUTE). This allows
inspection of migration-owned extension relations **without** broad/default SELECT grants to maintenance or
runtime DDL. The maintenance-owned purge definer still has only explicit grants for counted relations. All
porchlight relations retain the project's migration ownership invariant (`porchlight_owner`).

O2 migration 0055 extends S5's shared plan/probe once: comments, attachments, notification_sources and
notification_event_delivery are now explicitly counted/deleted, and selected notifications item/occurrence
references are counted/severed while recipient history and unrelated coalesced sources survive. Blob metadata,
bytes and collaboration_audit remain for GC/history; `blob_references` counts severed main/thumbnail references.
The FK-shape/second-order closure guard remains fail-closed for every future unknown dependency. No runtime DDL
or broad maintenance grants were introduced; PurgeCounts, EXPECTED_GRANTS and the CLI golden include every count.
Future extensions must use the maintenance-owner migration dance, revoke CREATE again, extend the shared
selection/order plan and verify counted deletion + unknown NO ACTION/CASCADE refusal. A helper's existence
alone never authorizes an uncounted cascade. See [O2 data-out contracts](data-out.md).

### O2 operational state (0056–0057)

| Table/column | Authority and maintenance | Domain derivation / revision / outbox |
| --- | --- | --- |
| `stream_retention` · id, epoch, pruned_through | singleton generated by committed published seven-day prefix pruning; app/agent SELECT only | none; atomic epoch forces held-stream resnapshot, porchlight_stream wake |
| `purge_log` · counts | maintenance per-table counts, nightly empty item_ids; append-only | none; no item enqueue on retention |
| `blob_leases` · expires_at | `pl_renew_blob_lease` locks pinned blobs then renews only a live lease | none; no expired revival; GC exclusion persists until expiry/release |
| `notification_kinds` · `backup_stale` | R53 registry INSERT only, hourly system producer/live super admins after 26h, last-success/recipient dedupe | ordinary notifier and recipient-private outbox, no domain derivation |

`backup_runs` keeps O1's authoritative publication record. Data-only exports use an explicit table allowlist
and member ACL; full encrypted backups include all database control state via pg_dump. Retention clock,
lock/grant boundaries, export inventory and streaming/lease lifetimes are specified in data-out.md.


## S6a notification inputs and generated state

| Table · columns | Authority/writer | Enqueue | Version/audit | Outbox |
| --- | --- | --- | --- | --- |
| `notification_prefs` · member/household key; reminders, assignments, dependents, activity, digest, connection, quiet_start, quiet_end | seeded profile key; self API updates canonical preferences | none for category/quiet changes | immutable keys; pl_version_guard, created_at/updated_at/version; no task event or parent bump | settings.changed, member_id scope, section=notification_prefs |
| `notification_prefs` · default_todo_offsets, default_deadline_offsets, `default_appointment_offsets` | self API (agent SQL read-only) | every live related owner/subject/assignee item with NO explicit item_reminders; sorted item/job locks before prefs update | same version guard, no task event | settings.changed |
| `notifications` · id, household_id, recipient_member_id, kind, category, item_id, occurrence_id, entity_type, entity_id, actor_member_id, actor_label, count, summary, window_start, release_at, released_at | generated by notifier, recipient prefs serialized; immutable key, item ref nullable on physical purge | never | pl_version_guard timestamps/version; no revision/task event or parent bump | notification.created at release; notification.updated on released merge, member_id scope |
| `notifications` · read_at | recipient API monotonic read actions; no-op does not write | never | same version guard; no task event | notification.dismissed on null→timestamp, member_id scope |
| `notification_sources` · notification_id, event_id | generated notifier links; UNIQUE event_id, same atomic transaction as processed_at; FK cleanup only | never | no version/revision/task event; no agent writes | through parent row |
| `notification_events` · `processed_at` | notifier only, immutable payload/actor/key retained; consumed even if suppressed | never | append-only event except one terminal processed_at update | through resulting notification row |

Task-event triggers route immutable per-write assigned/status/completed/reopened events; actor/agent label is preserved.
Derive emits hard-conflict events using only current counterpart projections, lifetime canonical occurrence-pair/member
dedupe. Terminal job attempts crossing 5 emit generic super-admin alerts, generation/member dedupe. Minute scheduler jobs
notification_reminders and notification_dependents route exact due reminder/local_alarm history and effective appointment
weekly/48h/24h slots. Default reminder synthetic IDs remove the authoritative reminder FK but never create item_reminders
inputs. All notification/source/preferences tables are included in full DB export and backup; O2 owns 90-day retention.
`notifications` row item refs SET NULL on item purge; notification_sources links cascade on parent/event purge. Every inbox
and snapshot response recomputes readable sources/count/summary from authoritative pl_item_can_read, never stored count.

S6a retry state: `notification_delivery_retry(member_id,attempts,retry_at)` is private generated consumer control state, no domain derivation/revision/version/outbox, app-only writes, durable bounded backoff; no credentials/error text stored. Full DB export/backup includes it.

S6a event retry state (0023): `notification_event_delivery(event_id,attempts,retry_at,last_error,poisoned_at)` is generated app-only consumer control state (agent SELECT for inspection). No item enqueue, revision, version, audit or outbox. It persists event retry/terminal poison status across restart. R83/0065 allow zero strikes during a six-hour grace from event created_at for transient SQLSTATEs 40P01/40001/55P03/57014 (57014 logs as statement_cancel); later failures consume the ordinary five-strike budget. Transient failures are isolated by event savepoints, with 2–60 s recipient/event backoff; earlier/later deliveries and held release continue. Constraint changes only; EXPECTED_GRANTS stays unchanged. Full DB export/backup includes it, and notification-event retention cascades it away.
