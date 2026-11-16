# Live stream (S6b / M5)

`GET /v1/stream` is mTLS + a device-bound member session, not a PAT route. Super-admin app
sessions follow visibility. A live non-dependent member in the requested household is required
for every delivery. Revocation is terminal, including lagged revocation broadcasts; heartbeat
revalidation catches session/logout/member/user/certificate expiry and database failures.

## Wire catalogue

Every published tickle has SSE `id: <stream_seq>`, `event: <kind>` and JSON
`{seq,kind,entity_type,entity_id}`. An eviction adds `evicted:true`; false is omitted.
No title, actor label, notification summary, source URL, recipient list, or arbitrary outbox payload
is serialized. Producers retain their entity categories; member-scoped routing does not assume
the concrete table name (e.g. settings includes Today, prefs, and availability).

| Kind | Authoritative routing | Test |
| --- | --- | --- |
| item.changed | Current item ACL, entity id identifies item | generated_sse_matrix_is_exercised_against_authoritative_scopes |
| item.access_changed | Current item ACL OR named former reader (`recipients`/removed `member_id`); `evicted:true` only when currently unreadable | former_readers_get_eviction; replay_rechecks_queued_acl_and_lists_evict_former_readers |
| occurrence.changed | Current occurrence's parent item ACL, not cached agenda or historical payload parent | generated_sse_matrix_is_exercised_against_authoritative_scopes |
| derived | Current item ACL | derived_is_a_supported_live_tickle; generated matrix |
| list.changed, list_entry.changed | Legacy: payload item_id/list_id; item-sync-v1: current list/entry parent rows and pl_list_can_read (attached uses current item ACL) | full_scope_catalogue_and_member_dismissal; parent_hints_ignore_forged_payloads_and_recheck_queued_acl |
| list.changed (eviction) | Current list/item ACL OR eviction=true and recipient in recipients; currently unreadable recipients get evicted:true | replay_rechecks_queued_acl_and_lists_evict_former_readers |
| comment.changed, attachment.changed | Legacy: payload item_id; item-sync-v1: current comment/attachment parent with current item ACL | full_scope_catalogue_and_member_dismissal; parent_hints_ignore_forged_payloads_and_recheck_queued_acl |
| notification.created, notification.updated, notification.dismissed | Payload member_id equals session member; never routed by optional item_id alone | full_scope_catalogue_and_member_dismissal; generated matrix |
| settings.changed | Payload member_id equality (Today, notification prefs, availability, etc.) | full_scope_catalogue_and_member_dismissal; generated matrix |
| ics_source.changed, suggestion.changed | Payload member_id equality; producer decides recipients | full_scope_catalogue_and_member_dismissal; generated matrix |
| member.changed | Household broadcast; entity_type member | generated matrix |
| Other/unknown kind | Filtered, cursor still advances | unknown_kinds_not_delivered; replay_advances_past_fully_filtered_batches |

Native items/occurrences are resolved by entity id; historical occurrence parents can change
across a split. Legacy streams resolve other item-scoped entities using the agreed payload item_id;
negotiated streams resolve all parent hints and child authorization from current rows. Household scoping
is applied both to outbox rows and authoritative items/members. Missing/malformed scopes fail
closed without a UUID-cast exception. Attached lists with an invalid item_id never fall back to
an unrelated standalone list_id. Only the recognized native entity kind pairs are accepted.

S7a supplies `pl_list_can_read(uuid,uuid) RETURNS boolean`. Migration 0025 creates a false stub
ONLY if that signature is absent; installing 0025 after 0030 never overwrites the real ACL.
Standalone-list matrix expectations call S7a's core `standalone_list_context` /
`standalone_list_can(View)`; delivery uses its real SQL function and tests create real lists/viewers.
Item/list and former-reader SSE cases retain visibility/relationship dimensions in the ONE generator;
member-private cases independently cross self/other targets. No test substitutes the SQL ACL.
Item eviction removes attached lists, comments, attachments and occurrence caches as well.
Notification inbox content authorization and production/read-to-dismiss writes belong to S6a;
this stream delivers synthetic dismissal tickles to both devices of the same member.

Control frames are listed under [Controls and replay](#controls-and-replay): `ready`,
capability-gated `caught_up`, terminal `resnapshot` and `drain`; none has an SSE id.

## Targeted read replacements

Servers advertising `item-sync-v1` on `/v1/me` support the
[Item sync bundle](items-reads.md#item-sync-bundle) and bounded requested-id comment/inbox refetches.
The bundle is an authoritative **item-scoped** replacement, not a new stream cursor: never promote
its response to a global acknowledgement. A 404 on a supported item bundle evicts the item and
its children; absent requested comment/notification IDs evict only those IDs. Collaboration
comments with a continuation are not complete omission coverage until every page is reconciled.
Legacy clients keep snapshot refresh; clients only enable targeting after their differential gate.

### Negotiation and dependency closure (0078)

A device opts in on **each** `GET /v1/stream` with
`Porchlight-Capabilities: item-sync-v1`. The header accepts comma-separated tokens and repeated
header lines; unknown tokens do not opt in. `/v1/me` advertises server support, not device opt-in.
Without this header, existing 0.1.7 frames retain their kinds, fields and routing; the extra
invalidation/reset rows below are scanned but never delivered. Availability rows, previously
filtered, also remain filtered for legacy streams. Numeric cursor gaps remain legal.

Negotiated child frames have optional `item_id` on `occurrence.changed`, `comment.changed`,
`attachment.changed`, `list.changed` and `list_entry.changed`, plus `list_id` on
`list_entry.changed`. Both scan and final queued-delivery recheck resolve these IDs from current
household-scoped rows, under the recipient's current ACL, never by copying outbox payloads.
An occurrence hint names its CURRENT parent after split/reparenting; clients also invalidate
any cached former parent. Missing/unreadable parents never become hints. Evictions do not add
unreadable parent IDs. No new content, labels or private counterpart IDs enter the wire.

| Negotiated kind | Scope / client action |
| --- | --- |
| `item.invalidated` / `item` | Current item ACL; `entity_id` names a bundle to replace, including retained agendas and collaboration. This is a read-projection invalidation, not a version bump or derivation enqueue. |
| `notification.invalidated` / `notification` | Recipient `member_id` equality only; `entity_id` names an inbox ID to refetch with `ids`. Absence removes that ID, including after physical DELETE. No source item IDs are sent. |
| `settings.changed` / `availability` | Recipient `member_id` equality; refresh member availability. Historical `member.changed` / `availability` rows translate to this same frame only for negotiated streams. |
| internal `sync.reset` / `household` | Not serialized as a tickle. Delivery sends terminal `resnapshot` with `{reason:"projection_changed"}`, no SSE id. Replace the full snapshot and resume its cursor; never acknowledge the reset in advance. |

Read-projection producers run in the SAME transaction as their source write. Statement-level
`member_agenda` transition tables invalidate changed items and peers overlapping OLD and NEW
busy spans. Item UPDATE/DELETE transition tables invalidate conflict peers even for title-only,
ACL-only and relationship parent bumps. `member_daily_load` OLD/NEW date transitions invalidate
items on the affected member-local dates, including member-only worker completion and disappearing
warnings. These are conservative invalidations: an unchanged bundle may be named too. Suggestion
writes also invalidate their item; attached list/entry changes invalidate split descendants whose
bundles inherit the template. No generated-state trigger enqueues domain derivation.

Item updates invalidate released notifications referencing the item directly or through a
coalesced source (including count/title masking on ACL change). Item changes also invalidate
split descendants, including non-overlapping tails whose collaboration inherits ancestor lists
(0079). In 0080 this traversal runs only for ACL, relationship parent bumps, lineage, delete and
restore transitions, not title/notes/time edits. The recursive query has no blocking full-closure
sort: it yields at most 201 rows, with depth bounded at 200; either boundary resets conservatively.
All producers skip candidate discovery once that household has a protected transaction reset.
Only actual member display-name changes invalidate released notifications with a source actor in
the changed member set, including single-source and coalesced summaries (0080 OLD/NEW transition
tables and indexed actor lookup). Colour, zone and authority edits do not scan actor history. Source-link
DELETE and released notification DELETE produce recipient-private invalidations, covering
purge/retention and suppression; ordinary notification update/read tickles are unchanged.

An existing derivation job's nonpending-to-pending transition emits `item.invalidated` in the
enqueue transaction (0079); initial item/job creation is already covered by `item.changed`.
A delayed worker's completion is a separate `derived` phase.
Bundle replacements and `derived` also refresh related caller-profile pending flags.

`pl_sync_emit` deduplicates extra entity invalidations transaction-locally (including savepoint
rollback); at most **200** extra entity rows and one coarse reset per household are emitted in a
transaction. Candidate queries take at most 201 distinct IDs. On overflow, remaining extras are
suppressed by a reset marker. Domain events are not capped or suppressed.
0079 replaces the caller-writable `porchlight.sync_emitted` setting with owner-only
`sync_emissions`, keyed by backend, full transaction ID and household. Neither app nor agent can
read/write the state or execute its private helper; session/transaction presets are ignored.
The transaction start timestamp also fences restored bookkeeping. Rows are retired by the next
transaction on that backend, with bounded (200-row), nonwaiting one-day lazy retention for
abandoned backends. State and outbox writes roll back together, including savepoints. This is
private operational bookkeeping, not a household export resource; full backups include it safely.
The helper takes only private state/outbox locks, never item/occurrence/job/member locks, so
load-worker and purge lock order is unchanged. No runtime grants or backup exclusions are added.

Household writer inventory: raw/admin household name, zone, deletion and any future household
column UPDATE are covered by the household trigger; initial installation INSERT precedes active
streams. Member role/deletion updates (API and raw SQL) issue the same coarse reset because
capabilities, effective zones and feed visibility can change broadly. Ordinary member display,
color and timezone/authority tickles remain unchanged; zone-dependent item derivation and load
completion supply item invalidations. No separate household-settings HTTP writer exists.

Proof: `prerollout_sync_server_b::coverage::seeded_http_agent_bundle_inbox_settings_coverage`
compares complete HTTP item bundles (collaboration and both retained member agendas), inboxes and
settings before/after each HTTP/PAT/real agent-login SQL mutation, checking real TLS streams for
two readers of different visibility. Source commit is fetched/applied before delayed workers
run; worker completion is checked independently with a new cursor. Raw app member-zone and raw
agent availability writers bypass request-local post-commit drains, and the zone fixture asserts
pending state before releasing the worker. Barrier frames cannot satisfy
any assertion. Every changed item, notification ID, settings resource and member profile must
have its own coverage; invisible hints are rejected (former-reader eviction IDs are intentional).
The proof includes actor renames, ancestor ACL gain/loss/viewers/delete/restore, retention,
member deletion and household-zone changes. Isolated producer-suppression tests in
`coverage/proof/mutations.rs` require an unsuppressed positive control and a structured failure
with the expected source/worker phase, resource kind and affected ID; see the
producer table in [derivation-matrix.md](derivation-matrix.md#targeted-stream-dependency-closure-0078).
Fixed DST/horizon clocks are identical for both observations in each step. Set
`PORCH_SYNC_SEED=<u64>` to replay generated ACL/metadata sequences; default 403398381, always
printed on failure. Parent spoofing/queued ACL, bounded/savepoint-safe amplification, recipient
inbox deletion and historical availability have separate DB tests. Legacy negotiation and
terminal reset/EOF have real TLS tests.

0080 cost regression (`prerollout_sync_server_b_fix2_cost`): with 800 split tails and 2,000
released single/coalesced notifications (3,000 sources), bundled PostgreSQL `auto_explain`
counts nested SQL statements, not just client round trips. Restoring the 0079 item/actor
producers and index configuration inside rolled-back transactions gives:

| Routine raw write | 0079 statements / outbox rows | 0080 statements / outbox rows |
| --- | --- | --- |
| Title edit | 1,420 / 202 | 16 / 1 |
| Member colour edit | 1,414 / 202 | 6 / 1 |

The remaining row is the unchanged domain tickle. The bounded-work DB fixture explains the
shipping descendant-query body after `ANALYZE`: both 400- and 800-tail lineages visit 201 recursive
rows and scan 201 item rows; ten repeated parent bumps in one transaction invoke discovery once.

## Controls and replay

- `ready` data `{cursor}`: opening scan cursor, no SSE id; precedes replay, not a replay-complete marker.
- `caught_up` data `{cursor}`: **only with `Porchlight-Capabilities: item-sync-v1`**, once per
  connection, no SSE id. The cursor is the fixed **connect watermark**: the global published
  maximum observed by the opening window query, not the requested cursor or a later live maximum.
  Every outbox entry at or below it has been delivered or skipped under the existing member/ACL
  rules before this frame. An empty backlog sends `ready` then `caught_up` immediately.
- `resnapshot` data `{reason:"cursor_expired"|"cursor_unknown"|"retention_pruned"|"projection_changed"}`:
  terminal, no SSE id (`projection_changed` only after capability opt-in). Fetch
  `/v1/snapshot`, replace the cache, resume from its cursor.
- `drain` data `{reason:"shutdown"}`: terminal, no SSE id. Reconnect with backoff using the last
  tickle id (or the snapshot cursor if no tickle arrived).
- Heartbeat comment `hb`: 120 s in production; no cursor or event id.

Last-Event-ID takes precedence over `?after=`. Both accept a nonnegative i64; absent cursor
starts at the global maximum. Publication order is assigned by the existing serialized projector,
not producer UUID/commit order. Gaps are legal. Each scan is bounded to 500 outbox rows, including
filtered rows. LISTEN/NOTIFY only wakes a scan; reconnect and heartbeats repair lost hints.
All still-queued sequences are rechecked together (`seq = ANY(...)`) in ONE ACL statement
immediately before flushing. The complete result is encoded as one bounded HTTP body chunk
(up to 500 ID-only frames); no authorized tickles are cached across body polls. Backpressure therefore
resumes at a new scan/recheck rather than delivering the next frame on an obsolete queued decision.
Bytes already handed to the transport cannot be recalled. Revocation/drain take priority over each
flush. Access events can turn into eviction (or ordinary refresh) at delivery.

**Replay window: seven days of outbox.created_at**, with the exact cutoff retained. A cursor at
or below the largest published row strictly older than that cutoff expires. A cursor preceding
an already-pruned prefix (below first retained seq minus one), or above the global maximum,
requires resnapshot. This is deliberately conservative for publication gaps and delayed projection
of old rows: a fresh snapshot is safe; replaying across an uncertain retained prefix is not.
Window bounds are global, matching the existing repeatable-read snapshot cursor. The current
maximum is always resumable, even if its row is old: no newer changes can be missed, and a fresh
snapshot on a quiet household must not enter a resnapshot loop. O2's nightly `pl_retention` prunes only
published rows strictly older than seven days below the first recent published sequence, retaining the
quiet maximum and every unpublished row. Deletion and singleton `stream_retention` epoch/pruned_through
advance commit atomically under `stream_assign_mutex`.
Every live scan and batched queued delivery reads `pruned_through` in the SAME statement snapshot as its rows.
Only pruning beyond the last fully flushed or wholly ACL-filtered scan cursor sends terminal `resnapshot`
with `{reason:"retention_pruned"}`. A queued batch is not yet flushed: slow/unpolled readers cannot lose
its prefix silently. Pruning already-consumed history leaves held streams alive. Idle wake/heartbeat scans
perform the same check, without an extra per-frame generation query. Stream read/delivery SQL failures retry
at 100/200/400 ms, retaining queued IDs for fresh ACL revalidation; exhaustion closes the body without falsely
claiming pruning. Auth revalidation remains fail-closed. Reconnect performs normal window validation.
`window_at` still aggregates outbox bounds per open; this is not a claim of an indexed initial-window query.
See [O2 contracts](data-out.md).

### Bounded background catch-up recipe

1. Open `/v1/stream` with the last acknowledged tickle ID (or snapshot cursor) and
   `Porchlight-Capabilities: item-sync-v1` on this connection.
2. `ready` only confirms the opening cursor. Keep processing **every** tickle, including
   frames newer than the connect watermark, until `caught_up` arrives. Publications after the
   connect watermark may arrive before or after `caught_up`; it is not an upper bound on frame IDs.
3. Finish applying/refetching and acknowledging everything before `caught_up`, then the client
   may close. Persist the last successfully processed tickle ID for reconnect, not the control's
   cursor (which must not regress an acknowledgement of newer frames). Controls have no SSE id
   and do not themselves advance the replay cursor.
4. On terminal `resnapshot`, replace the full snapshot and resume its cursor instead. Invalid/too-old
   opening cursors retain the existing terminal resnapshot-only response: no `ready` or `caught_up`.
   A reset, pruning, revocation or shutdown during replay retains its existing terminal behavior;
   no catch-up completion is claimed for an unfinished backlog.

There are no heartbeats between `ready` and `caught_up` unless replay lasts longer than the
heartbeat interval. Longer replay may interleave heartbeat comments, but `caught_up` still follows
all pre-watermark visible events. Neither a heartbeat nor `ready` proves catch-up completion.
Legacy streams do not receive `caught_up` and retain their existing bytes and behavior.

## Session sliding and shutdown

Heartbeats reuse auth::touch_session after current device/certificate/member/user/session validation.
A due touch (hourly) has actor attribution and the platform's existing session lifetime; expired or
revoked sessions are never revived. Touch locks the device first with SKIP LOCKED, then conditionally
updates the session, exactly like an authenticated request; contention is best effort and retried
on the next heartbeat. No transaction/connection is retained while awaiting stream events.

Shutdown is a persistent hub watch flag, independent of notification/revocation broadcast capacity.
The server stops accepting, sends terminal drain frames, calls Hyper graceful_shutdown to disable
keepalive and flush active responses, and waits up to five seconds before cancelling remaining
connections/handshakes. Control frames never advance the cursor. Tests verify drain + clean EOF,
held-stream session sliding, expiry/logout/revocation, and listener/request-pool isolation.

Controller TLS gate: `deploy/scripts/m5-demo-stream.sh`; configuration and destructive demo-only
fixture warning are in the script. It requires two distinct enrolled dev members and a privileged
superuser connection (outbox INSERT/DELETE) to the SAME database as the TLS API. Never run it against production.
