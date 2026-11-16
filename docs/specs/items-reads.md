# Items read and producer contracts (S4b)

All eight routes live on mTLS/session and Agent/PAT surfaces. Dependents cannot authenticate;
app super admins obey item visibility. Agents see every live item in their household. PAT writes
require write/admin. Color/zone PATCH is self-only; device authority requires an app device.
Requests use the standard version/time-zone headers, mutation idempotency keys and update If-Match.

## Shared ACL

`porchlight.pl_item_can_read(item uuid, member uuid) RETURNS boolean` is the authoritative SQL
rule: live same-household non-dependent member + live item, household OR owner OR assignee OR selected viewer.
Subject alone never grants access. `routes::reads::VISIBLE` is its API predicate (aliases i and
$1 household/$2 member/$3 authenticated agent); never accept the agent boolean from request JSON.
Read decoration delegates capabilities to core access policy. Stream uses the same SQL function.
Search uses a stored generated `search_document` + GIN with `simple`, preserving multilingual
names/vocabulary instead of stemming as English. Results use signed query/caller-bound composite keysets (`search-v2` context). S7a contributes `lists::SEARCH_SOURCE` at the shared UNION location; the combined query orders by
`(source COLLATE "C", id)` and binds the signed cursor to caller/household/agent/query. Items use source `item`,
lists source `list`; each wire row has `entity_type` plus its normal read response. Lists match live list names and
live entry labels/notes via stored generated `simple` documents + GIN (0031); archived entries never match.
Comment sources share the household/member/agent/query binds ($1..$4), before the shared
keyset/limit. Both search decoration and snapshot contributions use their owner's existing RR connection.
S7b contributes live comment bodies with generated simple FTS + partial GIN (0037), using the same VISIBLE filter and source/id keyset. Comment rows have normal Comment metadata plus entity_type:"comment". Snapshot adds comments and attachments metadata in the same RR view; deleted parents/children are excluded.
`GET /v1/items/{id}/comments` likewise checks the item ACL and reads its page in one read-only REPEATABLE READ
transaction (`items::read_snapshot`): a visibility revocation committed between the two statements can never expose
comments written after it (m6fix I1).

Collections return `{items, next_cursor}` (50 default, 200 max). Board order is text status then
`(board_position COLLATE "C", id)` over S3's R30 domain: next non-done/non-skipped/non-expired sequence
per item plus non-expired Done completed at/after now-7d. `porchlight.pl_board_cards(household, now)` owns
this single projection for both reads and S3 move-domain queries; expired rows are never neighbours. Mine filters owner/subject/assignee only after the projection.
Agenda order is chronological effective start/due instant, then occurrence UUID (signed composite keyset).
from/to are required UTC RFC3339, half-open, max 730 days. Date-only rows denote the local day
`[due_date 00:00, next day 00:00)` in the selected member zone (household zone for dependents),
and are included whenever that day intersects the requested window; timed rows include spans
intersecting the window. Another member's agenda is still filtered by the caller's item ACL. Hard conflicts
mask both other item ID (null) and title (Busy), retaining occurrence IDs as required by the shared contract.
Item-shaped rows include capabilities and pending_derivation. Recurrence intervals use exact
`{months,days,microseconds}` via the shared `porchlight.pl_interval_json` function.
Floating wall-clock fields use nullable string schemas with `local-date-time` format, not
RFC3339 instants. Response conformance tests validate every read schema, rejecting extra columns. No query authorizes from a derived table.

Snapshot is one read-only REPEATABLE READ transaction, including the household's published stream scan
watermark (highest stream_seq MVCC-visible in that transaction; zero if none). This is not an ACL-filtered
watermark: stream itself scans/advances over filtered tickles. Items include relationship IDs/reminder
configuration; occurrences and scheduled reminders cover now-180d to now+18 calendar months, including
undated occurrences and spans overlapping the window even when their start precedes it. Membership uses
**effective** occurrence anchors (including all `override_*` values), never only the generated recurrence date.
Fixed local-time overrides resolve in the item's zone; floating anchors resolve in the household zone.
Date-only todo boundaries (including fixed date-only overrides) use the household zone for cache membership.
Both time-resolution paths use core's
DST rules (earlier fold offset; gap to next valid wall time; R10 preserves collapsed span duration), shared
with derivation, rather than PostgreSQL's different `AT TIME ZONE` disambiguation. SQL first selects a conservative
superset using separate generated and effective-override endpoint boxes widened by three days (unbounded override shifts
are included directly; the slack bounds UTC offsets, gap resolution and R10 correction). Rust then refines exact
half-open membership; far history without an intersecting override is never transferred. Unparseable raw-SQL
time inputs fail open per SQL-bounded row with ID/category-only warnings, not a household-wide 500. Scheduled
reminders also require their occurrence in this cache, and fire within the half-open window.
Members expose only profiles, not credentials;
`tz_authority_device_id` is visible only on the caller profile. Profiles include `pending_derivation`
for live related items (owner/subject/assignee). Scheduled reminders are private to
caller (agents see all). Late publication of an already-included write is harmless; a concurrent write absent
from snapshot publishes after the cursor. Snapshot is intentionally unpaginated and supplies the complete cache.
The `lists` array contains normal ListResponse templates, including viewers/entries/Got it/capabilities, from
`lists::snapshot_contribution` on that same transaction. Per-occurrence templates have no selected occurrence;
occurrence-specific checks are fetched through list detail. Soft-deleted lists/deleted-parent lists never enter
snapshot or search, including for agents. Owners/admins discover orphan cleanup separately via
`/v1/lists?include_orphaned=true`. The retained parent ACL (ignoring only its tombstone) still governs content.
Without that read permission, cleanup returns id, version, item_id, mode, owner_id, `redacted: true`, `[delete]`
capabilities and empty entries/Got it/viewers, with no name. Clients render "Private list" and Delete, not cached
content or a parent link. Authorized parent readers retain view/delete. This opt-in does not change stream/item
ACL authority, and stale-delete `current` descriptors follow the same redaction.

## Item sync bundle

`GET /v1/items/{id}/sync` is the authoritative targeted replacement protocol advertised by
`GET /v1/me` as `capabilities: ["item-sync-v1"]` (additive string array). Older clients ignore
it; clients on servers without that capability keep full-snapshot sync. An unsupported endpoint
is **not** evidence of deletion.

The bundle uses one read-only REPEATABLE READ transaction and the snapshot document writer.
Every item/child query is item-keyed before decoding. Normal read admission applies, plus four
active responses per device (PAT identity on the agent surface); excess requests get the existing
429 `rate-limited` problem and `Retry-After`. Permits survive unread/gzipped bodies and release on
EOF/disconnect. No snapshot device/member/global admission is acquired. JSON negotiates gzip
through the normal final response layer; no binary attachments are fetched.

Top-level fields:

- `items`: exactly one **ItemRead**, including the snapshot's relationships, reminders,
  capabilities and pending flag (not the narrower item-detail shape).
- `window_from`, `window_to`: server clock minus 180 days / plus 18 calendar months;
  legacy aliases of `coverage.occurrences.from`/`to`, with zone/local metadata in that
  coverage object (not a separate occurrence window).
- `occurrences`, `scheduled_reminders`: the identical snapshot rows for this item. Membership
  uses household-zone/DST refinement and the conservative detached generated/effective hull;
  undated occurrences are included. Reminders are caller-private (agents see all) and require
  both an included occurrence and a fire time in the half-open server window.
- `board`: all household-board cards for this item under the caller's ACL, **including cards
  outside the occurrence window**. A retained mine view filters these by its normal relationships.
- `agenda`: `{member_id, from, to, items: ItemRead[]}` per requested window. Repeat
  `agenda=<member UUID>:<RFC3339 from>:<RFC3339 to>` up to eight times; identical windows
  are deduplicated. Each half-open window must be positive and at most 730 days. URL-encode
  offset `+` signs. Membership, ACL, ordering, masked hard conflicts and load warnings share
  `/v1/agenda`'s SQL/decoration. At most **200 agenda rows total** are returned across windows.
  `coverage.agenda` contains `{member_id,from,to,zone,local_from,local_to,cursor,next_cursor,complete}`
  per window; the same zone/local bounds appear on each top-level `agenda` window.
  Repeat `agenda_cursor=<next_cursor>` for incomplete scopes on the same sync endpoint.
  Unrequested scopes are untouched, not empty replacements.
- `suggestions`: snapshot-shaped suggestions referencing this item, caller-private (all for agents).
- `comments`, `attachments`, `lists`: populated only with `include=collaboration`, otherwise empty
  **without replacement coverage**. Lists include attached templates and readable inherited
  per-occurrence templates along split ancestry, never unrelated standalone lists. Attachment
  entries are metadata only. Comments use ascending UUID keyset order, capped at **200** per
  response. Send `comments_cursor=<comments_next_cursor>` to this same endpoint to continue;
  nonempty comment pages' cursors also work with the ordinary comments collection.
- Every document is bounded to **8 MiB uncompressed**, enforced by the document writer.
  Agenda/comment rows are staged one at a time; a row crossing the byte budget starts the
  continuation instead (so a page may have fewer than 200 rows). A bounded 32 KiB footer
  allowance reserves coverage/cursors and delimiters within that budget. Canonical nonpaged
  item/child sections (including board, suggestions, attachment/list metadata) must also fit.
  If those sections alone exceed the budget, the endpoint fails closed with **422
  `item-sync-too-large`** (`application/problem+json`), not a truncated bundle or a 500.
  This grants no replacement coverage: retain unacknowledged work and use the ordinary
  paginated collection reads to reconcile; retrying the same oversized bundle will not help.
  Only agenda and comments have sync continuations; there are no canonical-section cursors.
- `coverage`: `{item_id, continued, item_sections, occurrences:{from,to,zone,local_from,local_to}, undated_occurrences,
  reminder_member_id, scheduled_reminders:{from,to}, board_scope:"household",
  agenda:[{member_id,from,to,zone,local_from,local_to,cursor,next_cursor,complete}], suggestions, collaboration,
  comments, comments_cursor, comments_complete}`. `reminder_member_id` is null for agents.
  Initial responses have `continued:false,item_sections:true,suggestions:true`; `collaboration`
  covers attachments/lists only when requested. `comments` explicitly covers comment pages.
  Continuation requests contain only `agenda_cursor` and/or `comments_cursor`, never initial
  `agenda`/`include` parameters, and return only those continued sections plus metadata. They
  set `continued:true,item_sections:false,suggestions:false,collaboration:false`; the legacy
  window/board metadata does not grant replacement coverage when `item_sections` is false.
  Unrequested sections are omitted on continuations and grant **no** replacement coverage.

### Exact occurrence coverage membership (additive metadata)

`coverage.occurrences.zone` is the household IANA zone read in the same RR view as
snapshot membership, **not** the device/member zone or the fixed item's zone.
`local_from` and `local_to` are the exact `from`/`to` projections into that zone,
ISO-8601 wall-clock strings without offset (fractional seconds retained). These three
properties are optional in OpenAPI for compatibility with older servers; this server
always emits them for occurrence coverage, including continuation metadata. Their absence
on an older server requires snapshot fallback when local membership cannot be determined.
Reminder coverage remains `{from,to}`: reminder fire times are instants only.

**Membership compares resolved instants, not naive local bounds.** The local bounds
are projections, not separately rounded dates or independent membership thresholds.
Retain UTC `from`/`to` to distinguish both sides of a DST fold: converting a bound to
local and back with the earlier-fold rule can change the bound. In particular, a window
beginning at the second 01:30 does not cover a floating point resolving to the first
01:30. Do not implement `local_from <= local_anchor < local_to`.

For each cached occurrence of this item, apply the following independent client rule:

1. Start with its generated anchor fields. Only when `is_detached` is true, replace
   each `local_start`, `local_end`, `local_due`, `due_date` with its non-null
   `override_local_start`, `override_local_end`, `override_local_due`, `override_due_date`.
   Null overrides retain generated fields. For a detached **fixed** item with `tzid`,
   re-resolve the effective local start/end pair in the **item's** zone and replace
   `starts_at`/`ends_at`; similarly a non-null effective `local_due` replaces `due_at`.
   Otherwise generated fixed instants are authoritative, including a known second-fold
   instant; never re-resolve those instants from their generated wall time.
2. Resolve local times with core's DST rule: choose the **earlier instant** in a fold;
   in a gap, advance to the first valid wall-clock minute after the gap (not "add the
   gap duration"). Resolve a positive local start/end pair together: if the resolved
   end is at/before the resolved start, set end to resolved start plus the original
   positive wall-clock duration (R10). Preserve the nominal local fields.
3. In the household `coverage.occurrences.zone`, compute `S` as the first non-null of
   `starts_at`, `due_at`, resolved `local_start`, resolved `local_due`, resolved midnight
   of `due_date`. Compute `E` as the first non-null of `ends_at`, the resolved end of
   a valid local start/end pair (including R10), resolved `local_end`, resolved midnight
   of the day after `due_date`. Thus floating spans/points resolve in the household
   zone; date-only rows cover the household-local day, regardless of the item's
   `time_mode`/`all_day` flag. All-day spans follow their stored start/end anchors by
   the same rule, and may span multiple local days. Each midnight is resolved separately,
   so DST days need not be 24 hours. This priority also specifies mixed legacy anchor fields.
4. If `S` is absent the row is undated (covered only with `undated_occurrences:true`).
   Otherwise, if `E` exists, membership is `S < to && E > from`; if `E` is absent,
   membership is `from <= S && S < to`. Spans touching only an endpoint are excluded;
   points at `from` are included and points at `to` excluded. All comparisons are UTC.

With `item_sections:true`, an absent occurrence satisfying this rule can be removed;
rows outside coverage cannot. The SQL generated/detached hull is only a conservative
candidate optimization, **not** a union of generated/effective membership. Classify the
effective anchor only. Invalid raw-SQL timing rows retain the snapshot's SQL-bounded
fail-open behavior; a client unable to decode/resolve an anchor must retain it and fall
back, never infer deletion from an unclassifiable row.

### Agenda window coverage

Each `coverage.agenda[]` and `agenda[]` window also reports `zone`, `local_from`,
`local_to`, on initial and continuation pages. Here `zone` is the requested member's
**effective** zone (`tzid` for ordinary members, household zone for dependents), which
can differ from occurrence coverage. The local bounds are exact projections of that
window's UTC bounds. A nonexistent/other-household member has an empty agenda and no
zone/local metadata. No zone metadata is placed in the authenticated cursor; a fresh
RR view supplies it on each continuation, just like the projection rows.

Agenda membership uses the cached **agenda projection**, not occurrence anchors:
include when `(starts_at < to && ends_at > from)` or
`(from <= due_at && due_at < to)`, or for `due_date` when
`due_date >= UTC_date(from)-3 days`, `A < to`, and next-day local midnight `> from`;
`A` is the first non-null of `starts_at`, `due_at`, and local `due_date` midnight.
Date midnights here use PostgreSQL `AT TIME ZONE` in the reported zone, matching
`/v1/agenda`: ambiguous midnight chooses the **later** instant, nonexistent midnight
uses the pre-transition offset (not core's gap-to-first-valid rule). Floating agenda
spans/points are already member-resolved instants from derivation; do not resolve
occurrence wall fields again. Never use the household occurrence rule for agenda
removal. Also honor the covered key interval `(A,occurrence_id)` and `complete`/cursor
rules below: zone metadata does not turn a partial page into full-window coverage.

Clients **must keep requesting until every scope is complete**. Each page is an authoritative
replacement of **exactly the rows/key interval it covers**, like `/v1/agenda` pages: after the
input cursor through the last returned key, or through the end when `complete` is true. An
empty incomplete page covers no rows. Never erase an entire window/thread on a partial page.
Apply omission removal only for covered intervals; aggregate full scopes only after exhaustion.
No coverage authorizes household-wide deletion. Each continuation reads a fresh RR view;
concurrent events keep the scope dirty for another reconciliation. There is no global stream cursor.

Invisible, soft-deleted and physically purged items return the **same 404 not-found problem as
item GET**. A capable client must evict that item and its child/file/projection scopes on this
response, rather than retaining old private rows. Other failures retain unacknowledged work.

`porch items sync <ID> [--agenda <member>:<from>:<to>] [--include collaboration] [--json]`
follows every continuation and merges all pages into one JSON output, with final complete
coverage and null continuation cursors. Only this command accepts up to 9 MiB per HTTP response
(the server budget plus margin); other CLI JSON commands retain the 4 MiB safety limit.
It uses the same read protocol. No new tables, write grants, outbox producers or derivation inputs
are introduced; existing export and backup/restore coverage is unchanged. Client installation
and event dispatch are separate rollout slices; capability alone does not enable differential
sync until the client equivalence gate passes.

### Exact requested-id refetches

`GET /v1/notifications?ids=<UUID>,...` and `GET /v1/items/{id}/comments?ids=<UUID>,...`
accept **1–100** validated UUIDs. These are complete, unpaginated scoped refetches (`next_cursor:null`),
not a page of the collection: only requested, existing, visible rows are returned. A requested
id absent from `items` means **removed or invisible for this caller** and must be evicted from
that scope. Read notifications are included, but unreleased or another recipient's notifications
are not. Notification sources still recheck the recipient's authoritative item ACL. Comment
parent ACL is checked in the same RR view; an invisible parent returns 404 for whole-item eviction.
`ids` is exclusive with `cursor`, `limit`, and (for notifications) `unread`; invalid UUIDs, empty
sets, more than 100 entries and conflicting options return 400 invalid-request. Existing collection
pagination is unchanged when `ids` is absent.

## S8a read extensions (0040–0044)

`/v1/snapshot` includes `ics_sources`: live feed summaries only for app super admins and admin PAT agents, fetched inside the same read-only REPEATABLE READ transaction as items and the household stream scan cursor. The device-cache projection contains only `id`, `name`, URL `host`, `enabled`, categorical `last_status`, `last_fetch_at` and `version`. It omits the full URL (often a bearer secret), ETag/Last-Modified, import key and remote error/warning details. Full configuration stays behind the super-admin `/v1/feeds` route (admin PAT counterpart `/v1/admin/feeds`); devices fetch it on demand rather than storing it in the snapshot. Regular members receive an empty array, even when they own a feed appointment. Import scope rows are omitted. Source URLs/validators never appear in search.
Imported appointments already contribute to the shared item `search_document`; no separate source FTS corpus or cursor dimension is necessary. Search, snapshot, board and agenda use shared VISIBLE, and item-list SQL uses pl_item_can_read, with Rust capabilities checked against the same authoritative relationships. Feed `ics_source_id` is returned on item read models and restricts app capabilities to view/comment on every read surface; authenticated agents retain normal capabilities. One-off imports keep a null marker.

## Today settings schema

PUT /v1/me/settings body (not an envelope):

```json
{"today_sections":["overdue","today","this_week","needs_owner"],"scope":"mine","this_week_days":7,"pinned_list_id":null}
```

Unknown keys/enums/wrong JSON types: 400 invalid-request. Semantic schema failures (including out-of-range integer magnitudes): 422 validation-failed.
- today_sections: ordered array of unique values overdue/today/this_week/needs_owner/pinned_list, zero to five.
- scope: mine or household.
- this_week_days: integer 1..14 (default document uses 7).
- pinned_list_id: UUID or null; required non-null if pinned_list section is enabled. S7 owns list ACL/existence.

GET/PUT return `{member_id, settings, version}` + quoted version ETag. Every new member has a version-1
default document seeded by a definer trigger; existing documents are preserved in storage.
Invalid stored documents (including `{}`) normalize to defaults on every read, including snapshot. PUT requires If-Match and
updates only the caller row. These settings cover Today only; device start-screen/resume state stays local
(master-plan Decisions/spec Settings table), and notification settings belong to S6's notification_prefs.
Raw agents retain existing JSON-object-only storage validation; typed API documents are validated at the API.

PATCH /v1/members/{id} accepts any of color (`#RRGGBB`, uppercase), `tz_source:"manual"` + tzid, or
`make_zone_authority:true` (calling device). Zone and authority are mutually exclusive. Color may accompany
either. Empty/unsupported source/mismatched zone fields: 422. Invalid IANA zone: 400 invalid-time-zone.
R35b separates permissive read/ICS parsing from storage canonicalization. UTC-equivalent aliases
(`UTC`, `Etc/UTC`, `GMT`, `Etc/GMT`, `Z`, `Zulu`, `Universal`, `UCT`, zero-offset GMT variants)
store `UTC`. The static bundled tzdata link table canonicalizes all 257 IANA links, including
`US/Eastern` → `America/New_York`, `Asia/Calcutta` → `Asia/Kolkata`, `CET` → `Europe/Brussels`,
and `EST5EDT` → `America/New_York`; canonical regions retain their names. The sole documented
storage exception is `Factory` (unspecified local zone); parsing/ICS remain permissive.
Every chrono-tz variant either canonicalizes to a PostgreSQL catalog name accepted by pl_valid_tzid
or belongs to this exception list; tests cover every target and offset samples every six hours in
2025–2037. Core item validation, member zone/authority reports, and household initialization store
canonical names. S8 ICS persistence uses permissive parse_tz for instant resolution and storage_zone plus item validation for stored item/override zones.
0015 normalizes all existing household/member/item zones before tightening pl_valid_tzid and fails
fast naming any remaining non-conforming row. Raw SQL writes must already use canonical names.
Porchlight-TZ is optional: accepted hints canonicalize; invalid/duplicate hints are ignored without
rejecting the request or reporting a zone. Explicit invalid PATCH zones return invalid-time-zone.
The supplied version is checked before mutations and by S3's locked primitive; all domain effects run in a
savepoint under IdemTx and rollback before storing a business failure. Color follows zone/authority work,
so no item lock occurs after member acquisition. Explicit zone/authority scopes commit enqueue-only;
a generic IdemTx after-commit hook drains their related jobs with the worker's per-job implementation
in fresh transactions. Acquisition/derivation failures remain pending for the worker; response stays 200.
Member-profile `pending_derivation` is caller-only (false on other profiles); target and caller parameters
remain independent. Conflict masking hides both counterpart IDs.
The hook refreshes `pending_derivation` from committed state and never runs for replays.
The wrapper seals a conservative response with the domain commit, then best-effort reseals refreshed
flags. An interruption or concurrent replay during this brief hook interval can retain the conservative
pending flag, never repeat the mutation; snapshot/agenda and worker events remain authoritative. A composed PATCH can bump the member version twice.

## Notification producer ABI

```sql
porchlight.pl_emit_notification_event(
 p_household uuid, p_recipient uuid, p_kind text,
 p_entity_type text, p_entity_id uuid,
 p_payload jsonb DEFAULT '{}', p_item uuid DEFAULT NULL,
 p_occurrence uuid DEFAULT NULL, p_dedupe_key text DEFAULT NULL
) RETURNS uuid
```

Result NULL = suppressed self event or duplicate household/dedupe_key. NULL dedupe keys do not deduplicate.
Captures member and application_name via S1's pl_current_actor: API agents carry owner member + agent label;
raw SQL agents cannot spoof member and must set validated application_name=agent:<name>. The function is
security-definer with pinned search_path; app and agent may EXECUTE, but neither can INSERT directly.
Both can SELECT; only app may UPDATE(processed_at), reserved operationally for S6's notifier. Other fields
are immutable even to privileged direct UPDATE; processed_at may only go NULL → value.
DELETE is rejected except for `porchlight_maintenance` (S5 owns maintenance privilege grants).
Purge order: delete notification_events for the item first, then generated/relationship children,
audit history, and finally the item (respecting lineage order). The item FK is retained.
There is intentionally no occurrence FK: events survive regeneration; the producer checks occurrence
household/item consistency at insertion. Household composite FKs protect member/item references. Payload must be a JSON object; producers decide recipients and payload contents.
Kinds and actor-self suppression are data in the owner-written `porchlight.notification_kinds(kind text PRIMARY KEY, self_suppressed boolean NOT NULL, description text NOT NULL)` registry (R53). `notification_events.kind` has a foreign key to that registry; unknown kinds fail with 23503. App/agent roles may SELECT the registry, never INSERT/UPDATE/DELETE. The producer reads its flag rather than hard-coding a kind list. Canonical registry rows:

| Kind | Self suppressed | Introduced | Meaning |
| --- | --- | --- | --- |
| assigned | yes | 0015 | Item assignment changed |
| status_changed | yes | 0015 | Occurrence status changed |
| completed | yes | 0015 | Occurrence completed |
| comment | yes | 0015 | Comment added |
| mention | yes | 0015 | Member mentioned |
| conflict | no | 0015 | Hard scheduling conflict |
| load_conflict | no | 0015 | Availability load conflict |
| reminder | no | 0015 | Scheduled reminder |
| dependent_unassigned | yes | 0015 | Dependent item needs an assignee |
| derivation_error | no | 0015 | Item derivation failed |
| suggestion | yes | 0015 | Scheduling suggestion |
| calendar_changed | yes | 0044 | Changed imported/feed appointment (details below) |
| feed_error | no | 0044 | Failed/partial feed fetch (details below) |
| backup_stale | no | 0057 | No successful household backup for more than 26 hours |
| server_certificate_expiring | no | 0077 | Daily server-leaf expiry band (90/60/30/7 days); live super admins only |
| disk_headroom_low | no | 0077 | Hourly byte/inode/backup-working-space headroom alert; live super admins only |

To extend: in the workstream's reserved migration, `INSERT INTO porchlight.notification_kinds(kind,self_suppressed,description) VALUES (…) ON CONFLICT (kind) DO NOTHING`, and add a row here. Never redefine `pl_emit_notification_event` or a kind CHECK outside 0015. The notifier handles registered kinds generically with safe default templates for kinds it does not yet know.
No notifications table or delivery/routing is implemented here (S6).

`settings.changed` uses entity_type member_settings, entity_id member UUID and payload `{member_id}`;
only that member receives it. `derived` retains authoritative item ACL at delivery. The closed allow-list
lives in server/db/src/stream.rs for S6 extension. New tables/columns are included automatically in full
pg_dump/export/backup; no selective export was added.

## S7b collaboration producers and stream scopes

HTTP comment creates emit `comment` to owner/assignees and `mention` to newly mentioned live readers;
edits emit only newly introduced mentions. The actor is excluded for both session and PAT writes.
Both kinds are emitted in one statement within the domain savepoint, with entity_type `comment`,
entity_id comment UUID, item_id parent UUID, payload `{item_id,comment_id}` and dedupe key
`comment:<comment_id>:<version>:<recipient_id>:<kind>`. A dual recipient receives both kinds.
No new notification kinds were added. Under R53 the notification_kinds data registry provides suppression;
no producer redefines pl_emit_notification_event or a kind CHECK.
`comment.changed` / `attachment.changed` outbox payloads contain exactly `{item_id}` (no body/name/hash).
The closed stream allow-list routes those kind/entity pairs through pl_item_can_read on that item scope,
including soft-delete tickles; a hidden/deleted parent is never delivered. S6b retains that authoritative check.

## S8a notification kind extension (0044)

- `calendar_changed`: changed imported/feed appointment inputs or exception edits only, one event per live readable owner/assignee. Emitted in one SQL statement per import batch, never for normalized no-ops or deleted/hidden/dependent recipients. Business self suppression is enforced inside the shared ABI (including agent owner attribution); no dedupe key, because separate changed imports are separate business events. Entity/item is the item UUID, payload `{item_id}`. Preview savepoint rollback includes these events. Source scheduler is a system actor.
- `feed_error`: operational failed/partial feed status, live super-admin recipients only. Source bookkeeping trigger emits a single batch through the shared ABI, covering scheduler, removed-owner and raw SQL status changes. Not self-suppressed. Entity is `ics_source`, payload `{source_id,status}` (no URL, name, remote UID, title or error detail). Dedupe key `feed_error:<source UUID>:<recipient UUID>:<UTC fetch date>` implements one alert per source/recipient/day, whether failed or partial; another source/day still notifies. No item/occurrence FK. Notifications delivery/processed_at remain S6-owned.

## S8b planning extensions

Snapshot includes `availability: AvailabilityResponse[]` and `suggestions: SuggestionResponse[]` from the
same read-only RR transaction as items/cursor. Sessions (also super admins) get their own rows; agents
get all live household availability (empty version-0 defaults included) and all household suggestions.
Suggestions join the shared VISIBLE predicate even for history. No planning table grants item visibility.
Availability and proposal spans have no independent text document: the FTS extension adds no planning
source; their item title is already searchable once with VISIBLE. Agenda includes additive load warnings.
Producers retain existing `load_conflict`/`suggestion` kinds and the ABI above: load is a single batched
statement over near-term enter/leave transitions, member/date/state dedupe, no empty-window notifications;
EDF is member/run deduped, dependent candidates member/occurrence deduped with real run payload.
Planning producers skip dependent/deleted recipients (dependents cannot read these member-private
events); dependent projections remain readable by authorized admins, with no notification rerouting.
Snapshot suggestions use the REST typed response, not table JSON; internal `updated_at` is excluded.
Agenda warning queries are bounded to page occurrence IDs and merged by occurrence in linear time.

### List producer and stream extension (S7a)

List/check writes emit `list.changed` / `list_entry.changed` tickles, not notification_events. Under R53,
notification kinds are data in the `notification_kinds` registry, not a closed CHECK/ABI. S7a is not a business
notification producer and registers no list-check kind or pseudo status_changed event.
See the pass plan's Stream scopes: every list payload includes `list_id`; attached payloads additionally include
`item_id` as the authoritative ACL scope (item scope takes precedence). Standalone scope uses the real
`pl_list_can_read(uuid,uuid)` (0030), never a permissive stub. Eviction payloads preserve previous recipients.

## C4 narrow occurrence refetch

The existing `GET /v1/items/{id}/occurrences` accepts optional query `undated=true`: includes undated rows alongside the `from`/`to` window. Default/false preserves the existing window semantics. Same parent-item authoritative View ACL and capability projection as every collection read; no super-admin visibility bypass, no new permissions. Authenticated pagination cursors bind the option as well as item/window/caller/zone. The existing occurrences permission-matrix rows apply unchanged; `client_live::windowed_occurrences_can_include_undated_with_item_acl` additionally checks true/false/default and private-other-owner 404. No SQL grants/migration or derivation inputs changed.
