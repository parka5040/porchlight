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
| list.changed, list_entry.changed | Attached: payload item_id with current item ACL; standalone: payload list_id through pl_list_can_read | full_scope_catalogue_and_member_dismissal; generated matrix |
| list.changed (eviction) | Current list/item ACL OR eviction=true and recipient in recipients; currently unreadable recipients get evicted:true | replay_rechecks_queued_acl_and_lists_evict_former_readers |
| comment.changed, attachment.changed | Payload item_id with current item ACL | full_scope_catalogue_and_member_dismissal; generated matrix |
| notification.created, notification.updated, notification.dismissed | Payload member_id equals session member; never routed by optional item_id alone | full_scope_catalogue_and_member_dismissal; generated matrix |
| settings.changed | Payload member_id equality (Today, notification prefs, availability, etc.) | full_scope_catalogue_and_member_dismissal; generated matrix |
| ics_source.changed, suggestion.changed | Payload member_id equality; producer decides recipients | full_scope_catalogue_and_member_dismissal; generated matrix |
| member.changed | Household broadcast; entity_type member | generated matrix |
| Other/unknown kind | Filtered, cursor still advances | unknown_kinds_not_delivered; replay_advances_past_fully_filtered_batches |

Native items/occurrences are resolved by entity id; historical occurrence parents can change
across a split. Other item-scoped entities use the agreed payload item_id. Household scoping
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

## Controls and replay

- `ready` data `{cursor}`: opening scan cursor, no SSE id.
- `resnapshot` data `{reason:"cursor_expired"|"cursor_unknown"}`: terminal, no SSE id. Fetch
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
