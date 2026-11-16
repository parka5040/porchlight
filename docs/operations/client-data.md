# Client household data (C2b)

`AppServices.data: HouseholdData?` is non-null in production; null is allowed only in shell-only test services.
Own one application-lifetime instance. Do not create another HTTP client: generated `ItemsApi`, `ReadsApi` and
`MembersApi` call `GeneratedCall.trusted(controller)` through C1b's pinned mTLS `ApiTransport`. `:shared` generates
directly from `docs/openapi.json` with C2a's generator conventions (`client/buildSrc/.../OpenApiClientConventions`,
also used by `:contract-check`); its only extra mappings are the reviewed `SHARED_SCHEMA_MAPPINGS` (presence-aware
PATCH documents and the `SearchRead` oneOf). Lists/planning/calendar models may be generated, but C2b builds no
repositories for them (C5/C6).

## Binding interfaces for C3/C4

C5 comment threads render the live newest-50 cache tail (also offline), expanding only on Show older in 50-row increments. Opening never walks the oldest-first server collection. Comment tickles should use `comments.refetchByItemIds`: it reconciles all pages through the captured cache horizon, including edits/deletes beyond page one, or use the normal authoritative `sync.refresh()` snapshot. Attachment batches remain 20 rows and thumbnails remain demand-loaded; see [client-collaboration.md](client-collaboration.md).

| Property | UI flows / operations |
| --- | --- |
| `household` | `members`, `settings`; `refreshMembers`, `refreshSettings`, `patchMember`, `putSettings` |
| `items` | `items`, `observe(id)`; `get`, fully paginated `list`, `create`, `edit`, `delete`, `take`, `setAssignees`, `setViewers`, `setReminders` |
| `occurrences` | `occurrences`, `forItem(id)`; fully paginated windowed `refresh(itemId, from, to)`, `status`, `complete`, `reopen`, `patch`, `move` |
| `board` | `cards`, fully paginated `refresh(scope)`, `neighbours(mover, BoardDrop)` |
| `agenda` | `rows` (all cached members; filter by `agenda.memberId`), fully paginated `refresh(from, to, memberId)` |
| `search` | online-only `search(query, cursor, limit)` → `SearchResults(hits, nextCursor)`: `SearchHit.Item(ItemRead)` or `SearchHit.Other(entityType, json)` (lists, C5); does not alter cache visibility |
| `sync` | `status`, `cursor`; `refresh`, `onForeground`, `refetchByIds`, `evictItem`, `clear` |
| `cache` | immutable `view: StateFlow<CacheView>`, `sync`; typed `itemWrites`/`itemWrite(id)` retain response reminder offsets until an authoritative snapshot |

### Interface notes for C3/C4 (changed in C2b fix rounds 1–2)

- `TrustController.onProblem(problem)` and `onTransportError(error)` are **`suspend`**. An identity-ending
  problem (device revoked/unknown/expired) awaits the durable cache clear before returning.
- `TrustController.sessionEpoch` is the household **identity** epoch, not a per-login counter (see "Cache and
  lifecycle"). Fence your own long jobs with `cache.inSession`; a same-identity relogin keeps the epoch and the cache.
- `search.search(...)` returns `SearchResults(hits, nextCursor)`. Each hit is `SearchHit.Item(ItemRead)` or
  `SearchHit.Other(entityType, json)`; list hits stay raw JSON for C5.
- Generated APIs encode request models and decode response models through `GeneratedCall` on an injectable Default
  dispatcher; transport JSON stringification/parsing uses its own injectable Default dispatcher. Dispatcher switches
  retain request/identity context, return to the caller, and recheck epoch/session after encoding and decoding; an
  old encoded intent cannot acquire replacement credentials. Decoding also rechecks the appropriate PIN gate. The generator template is authoritative; generated sources are never edited.
- Generated APIs declare required parameters as non-null with no default, including required query parameters such
  as `/v1/agenda`'s `from`/`to` and `/v1/search`'s `q`. Optional parameters default to `null`. Form query arrays follow
  OpenAPI explode semantics: exploded arrays repeat keys, nonexploded arrays use a comma-separated value. Each value
  is URL-encoded independently, including `+` in RFC3339 offsets. `GeneratedCall.requestRepeated` accepts multiple
  values per key; existing scalar-map `request` callers keep their API and encoding. Empty arrays omit the key.
- `HouseholdLocked` (an `IllegalStateException`): the **UI** trust gate is not Ready (R64).
  - C4 application-owned work uses `withBackgroundSession(controller)`: only bounded sync reads and notification
    Complete/Read routes may use a valid stored session while `PinRequired`. The scope captures epoch/token so queued
    work cannot acquire replacement credentials. Other trust states still block it. UI scopes never install this context;
    account/device/admin/step-up requests still require the PIN. Cached content remains hidden behind TrustGate.
  - Thrown as itself **before** sending: nothing reached the server, and any retry after unlocking is safe.
  - **`HouseholdLockedAfterReply(idempotencyKey)`** (a subclass): the gate closed while the reply was in flight, so the
    reply was withheld. The outcome is ambiguous: the server may have committed the write. After unlocking, retry
    only with the same body, version and `key = e.idempotencyKey`, never with a new key; the server replays the
    committed result. Reads carry `null` and simply retry. Sync reports either case as `Idle`, not `Failed`.
- Board/agenda rows embed their full `ItemRead`. Projections are fetched after the snapshot, so an item created in
  between can appear on the board or agenda before it appears in `items`; never assume `items.observe(card.id)` is
  non-null. The next sync or C4 event repairs it.
- `cache.putOccurrences(itemId, rows, from, to)` requires the window the rows were fetched with. Undated rows and rows
  outside the window are never deleted.

## Targeted sync rollout (0.1.8)

The single shipping kill switch is `HouseholdData.targetedSyncVerified` in
`client/shared/src/commonMain/kotlin/app/porchlight/shared/data/SyncController.kt`. It is **true**, gated by
`PrerolloutClientSyncDifferentialTest`: 200 deterministic household seeds by default, SQL content-table and published
view equality against fresh full reads after each quiescent mutation, both retained member agendas, mine/household
board coverage, paging/fault/eviction/restart/identity companions. `PORCH_SYNC_CLIENT_SEEDS` increases the campaign;
`PORCH_SYNC_CLIENT_SEED` replays one seed. Failures print the seed and a deletion-shrunk mutation sequence.
Acknowledgement state and privacy tombstone history intentionally differ from a fresh cache; separate prefix and
late-reply assertions verify those operational fences. This is a model/client proof, not a real-device/server cost gate.
Legacy servers keep the snapshot path; an unsupported bundle endpoint grants no replacement coverage, never deletion.
Capability withdrawal is rechecked after replies.

On an `item-sync-v1` server, stream events and scoped local-write follow-ups enter the same
resource scheduler: 250 ms batching, two shared targeted-read slots, at most 512 active resource keys, and one pending
read-start cohort behind each active read. Demands arriving after read start dirty that resource again. Reads pin
identity/storage/session leases; overlapping content and inbox lanes serialize scheduler replacements, and
full refresh is exclusive against targeted installs. All repository reads, including screen-entry board/agenda,
comment/list pages, settings/member reads and worker inbox refreshes, share collection read-start generations.
A reply predating another committed authoritative install cannot write or publish over it; queued scoped
repairs retry with new read-start fences. A crossed privacy/eviction fence instead rejects the old caller
(with the existing safe redacted-list descriptor exception); repair never silently renews that caller's access.
Application-owned targeted scheduler cohorts may re-read up to three times when a newer visibility fence rejects
an attempt, preserving identity/storage/session admission and capturing a fresh fence before each new request.
Item eviction already hides the inbox atomically; dispatch does not advance that inbox fence again.
Provisional mutation receipts retain their separate merge
rules and are not authoritative omission coverage. Full-read ordering repairs preserve their original receipt
horizon. Narrow repairs publish only touched IDs/parent scopes (including any list scopes trimmed during the
transaction), retaining unrelated decoded objects rather than reloading the household. Retained agenda windows (including empty ones) and board scope
commit in `CacheProjectionScope` with the projection rows and survive process restart. Cache schema 14 remains
disposable: older platform caches rebuild without touching trust or delivery metadata. Attachment cleanup obligations
and a snapshot watermark waiting on those obligations are durable cache rows, committed with metadata removal.
Replay and cache initialization retry unfinished purges; neither stream acknowledgements nor a deferred snapshot
watermark advance until original, thumbnail and private viewer aliases are removed. Failure count and next retry time
are durable, with exponential retry delays from one minute to one hour. Unrelated reads retain the storage notice;
heartbeats and successful HTTP requests cannot mark blocked stream progress healthy. The shared notice offers
**Clear local cache**: after confirmation, wipe disposable household rows and the attachment directory, then refresh from the server.
This is same-identity storage recovery: locally authored item and comment drafts and pending requests (including their frozen
step plans and idempotency keys) survive. Existing-resource editors are retained but hidden until a fresh authorised
read restores access; confirmed invisible resources discard their editors with an explanation. Other unsent screen
fields, including list/planning forms, may reset, as the confirmation explains. Every pending write retains its originating
identity epoch and session revision/token; GeneratedCall checks that immutable origin before every send, including uploads,
identical retries and relationship-plan continuations. Delayed screen identity notification cannot transfer an old intent
to replacement credentials. Storage recovery alone does not invalidate that origin. Revoked representations
are never restored. A failed wipe retains its durable obligation and cannot acknowledge. Trust, device settings and delivery metadata
are never discarded. File keys use a random persisted cache namespace, stable across same-owner process restarts and
rotated on identity changes; the first schema-14 open purges legacy process-epoch files before owner admission. Overlapping durable agenda
coverage can exceed 730 days; reconciliation splits it into wire-valid windows before either bundle or full reads.
Evictions bypass fetch queues
and invalidate older replies, including ancestor-keyed checks for an evicted split occurrence.
A failed continuation installs nothing and grants no acknowledgement. A successful bundle never advances the global
stream cursor: only the fully committed delivered prefix may do that. Numeric sequence gaps are legal.

Item bundles replace only their declared item, occurrence/reminder windows, board cards, requested agenda key
intervals, suggestions, and explicitly covered collaboration. Fetch every comment/agenda continuation before
reconciliation. Comment/inbox `ids` reads remove absent requested IDs only; attachments are metadata-only and removed
private files are purged. A covered occurrence omission removes only its occurrence-cache row: retained agenda
windows, off-window reminders, inbox entries, suggestions and occurrence list checks have independent coverage.
A separate occurrence-row generation still rejects older row replies without declaring the occurrence globally
evicted. Only explicit occurrence deletion or item visibility loss cascades across those collections. Projection invalidations
refetch decorated agendas and inbox representations even when entity versions did not change.

Item visibility loss masks foreign conflict decorations (including load warnings carrying private counterpart IDs)
in every cached item, projection, write receipt and suggestion in the eviction transaction. Identifiers become null
and the title becomes Busy; member, owning occurrence, time interval and warning kind remain, matching server masking.
A failed peer read is not evidence that an authorized warning disappeared. It removes inherited
list content and invalidates the entire inbox because coalesced summaries need not expose their source item IDs.
Incoming bundle, snapshot and projection decorations check the referenced item's/occurrence's current visibility
and request-start generation, not just the owning row's fence. Late replies cannot reinstall revoked references,
even if a newer authorized read has since restored that item. Canonical split lineage IDs remain structural metadata,
not permission to restore an ancestor's list content; aggregate Busy load warnings without private references remain. See [the wire dependency closure](../specs/stream.md#negotiation-and-dependency-closure-0078).

Occurrence omission uses the bundle's household `zone` plus validated `local_from`/`local_to` projections, while
comparing the original UTC bounds (essential at a fold). Effective detached anchors, core's earlier-fold/first-valid-
minute gap rule, R10 spans, date-only days and half-open membership follow
[the exact contract](../specs/items-reads.md#exact-occurrence-coverage-membership-additive-metadata). Agenda omission
instead uses each scope's effective member zone, PostgreSQL midnight resolution (later fold/pre-gap offset), SQL's
three-day date floor, and its covered page key interval. Cached member/device zones are not substitutes. Older
servers omitting metadata use the atomic `ItemBundleNeedsSnapshot` fallback when membership is uncertain; malformed
metadata/unresolvable anchors fail closed through a separate invalid-coverage fallback. Neither infers deletion.

First sync, identity/cache loss, terminal resnapshot, unknown future kinds, broad role changes, identityless committed
writes, unsupported/oversized bundles and unclassifiable coverage retain the authoritative full-refresh fallback.
With targeting enabled, full snapshot starts coalesce to at most one per 30 seconds per identity/session/storage generation, honoring Retry-After;
work stays unacknowledged while waiting. Background catch-up is **not implemented**: `ready` currently precedes replay
and is not a caught-up barrier. The worker retains its legacy safety-net read; see
[client-live.md](client-live.md#targeted-background-catch-up-rollout-blocker).

Use generated models (`app.porchlight.shared.api.models`) rather than defining screen DTOs with a divergent wire shape.
`ItemEdit` preserves omission vs explicit JSON null. `EditScope.All` sends `all`; `ThisAndFuture` sends
`this_and_future` and requires `occurrenceId`. Per-occurrence overrides go through `occurrences.patch`.
C3a's `occurrences.move` additionally accepts a `visible` projection filter and retained `BoardMoveReplay` (bodies per key, `activeKey`) for filtered/relocked UI moves; see [client-screens-c3a.md](client-screens-c3a.md). `HouseholdData.bindTrust` accepts an optional Main-delivered identity-epoch callback for memory-only screen state. Existing callers/defaults are unchanged.

`BoardDrop(status, index)`: `index` counts the visible cards of that column **without the moving card** (0 = top; past
the end appends), so dropping a card back where it was keeps its index.

Every mutation returns `WriteResult.Success(value, refreshPending)` or `VersionConflict(current, problem)`.
Item/occurrence writes return a nullable value: a documented 204 is committed success without a representation, not an invalid 200 body. Visibility-removing 204 evicts the item and schedules the normal snapshot follow-up; a this-and-future split's 204 refers to the new tail and never evicts the head. A create 204 also remains success: no ID/model is invented, and the snapshot reconciles the new item. Regenerated-away occurrence 204 evicts that occurrence/projections/reminders and refreshes its parent. Follow-up failure still reports saved/refreshPending, never rejection. Read `value?.conflicts` for item/occurrence conflicts; present `current` verbatim for compare UI. Never silently replace
versions or automatically retry version conflicts. Success remains success when follow-up reads fail: `refreshPending`
means schedule a read retry, not replay the mutation. Cancellation propagates; callers must not interpret it as proof
that the server did not commit. Callers may supply the original key for an intentional byte-identical replay.
There is no offline queue or optimistic authoritative mutation. A connectivity policy can be injected into
`WriteCoordinator`; production network failures surface through the transport without being replayed.

Unsealed 409 `idempotency-in-progress` (server Retry-After, 1-second fallback) and `conflict` with numeric Retry-After retry the same frozen body/version/key, bounded to three attempts with exponential backoff floors and a 5-second wait cap per attempt (at most two waits). Exhaustion is not rejection: ScreenWrites retains the key/closure and offers identical retry; a new submit cannot replace that pending intent. Sealed validation/version conflicts end the intent; correcting them requires a new key.
Only a decoded `application/problem+json` 4xx is a sealed server decision. Any 5xx (the server rolls back and releases
the key; a proxy may answer after a commit), a non-problem error page (`http-error`) and an undecodable reply, including
an undecodable 2xx (`invalid-response`), are ambiguous (`ApiProblem.isAmbiguousOutcome`): ScreenWrites keeps the frozen
key/closure and offers only the identical retry or an explicit abandon, so a resubmitted create cannot duplicate.
No-op status 200 is ordinary success. A move sends composed `status` plus `board_move`: `after_id` is the visible
predecessor above, `before_id` the visible successor below. One 422 `invalid-reference`/`stale-neighbour` retry
refetches the board and chooses a **new** key. The visible projection, never all cached occurrences, supplies bounds.

Write follow-ups never wipe a projection (R59). An item write stores the response, then resnapshots (R59a; only the
snapshot carries reminder identities, scheduled reminders and creation metadata). Until it arrives, response conversion
preserves available creation metadata and matching reminder rows provisionally, replaces explicit nulls and stores real
offsets. The snapshot is staged with board and agenda like any other, and queues behind any in-flight refresh, so it is
always taken after the write. Overlapping demands share application-owned work in demand cohorts: each demand joins
the next read that starts after it, with at most one pending cohort behind the active read. Each cohort completes or
fails with its own satisfying read, not when all newer work drains. Cancelling a waiter does not cancel shared work;
`HouseholdData.close()` cancels its refresh scope. Flights are compatible only within the same identity epoch, storage
generation and session revision. Scheduled reads pin their original session without granting UI callers background
PIN access; replacement-session waiters never inherit an obsolete flight's cancellation. Every newly started read
renews its visibility fence while preserving its original identity/storage admission. Fence capture and publication of
the cohort's started/visibility state share the admission lock, released before network work. A post-eviction
restored-access demand cannot join a read in the gap between fence capture and activation, even if that older read's
cursor covers its event. Older snapshot omissions cannot fence a queued creation repair, while evictions after the
new read starts still win. Board, agenda and retained
scopes fetch independently with a shared three-request ceiling; pagination within each collection stays sequential
and all results still promote in one transaction. A stream echo observes an in-flight cohort and requests another
only if its sequence was not covered by that snapshot or its later request-start visibility fence requires a fresh
authorized read. A staged refresh read that fails with HTTP 429/503 and Retry-After retries once after that delay,
capped at five seconds and cancellable; no triggering mutation is replayed. Without Retry-After, or on a second
failure, the normal failure/refreshPending behavior applies.
Failed refresh leaves provisional identities/timestamps; use the acknowledged offsets and
`refreshPending`, not those provisional fields. An
occurrence write stores the response, then refetches the affected entities (parent item, its occurrences in the
snapshot window) and both projections, replacing their rows in place in one transaction; a failed fetch leaves the
previous rows and returns `refreshPending = true`.

A snapshot requested before a write can arrive after it. Two guards keep that older snapshot from undoing the write:

- **Receipts.** Item GET/write and occurrence write responses, member PATCH and settings responses (including a settings GET), and item
  deletions, received after the snapshot was requested, are re-applied inside the snapshot's install transaction.
  - A response replaces a row **the snapshot holds** when its version is higher.
  - A row the snapshot lacks is never re-added. It includes every undated occurrence of a visible item as of its read;
    absence of a previously existing undated row can reflect deletion/reparenting. Newly created rows and dated rows
    outside the window can also be absent; absence is not a general deletion proof. Repair refetch handles them.
  - A deletion evicts the snapshot's older copy of the item, with its occurrences, projections and reminders.
  - This covers undated occurrences too; the C4 window extension also returns them. The older row is never published.
- **Repair refetch.** Item refetches that ran meanwhile (write follow-ups, C4 tickles) are repeated once after the
  install: the parent item, its dated occurrences in the window plus undated rows (`undated=true`), and both projections. If that repair fails, the
  installed snapshot stays and `status` is `Failed`; the next refresh or tickle repairs it.

What remains uncovered:
- C4 closes the undated tickle gap: bounded `/v1/items/{id}/occurrences?from&to&undated=true` includes undated rows under the same authoritative parent ACL. `refetchByIds` uses that option and reconciles both dated-window and undated coverage. Default repository refreshes still preserve undated rows when they do not request them.
- Rows missing from an older snapshot are not resurrected by receipts; queued repair/resnapshot restores new
  creations, or `refreshPending` reports failure. A delete's receipt keeps the item evicted.

Keep `pending_derivation` markers from member/item responses until an authoritative refetch. C4's `derived` tickle uses
`sync.refetchByIds`; member IDs resnapshot all affected time projections. Unknown occurrence IDs resnapshot because the
server has no occurrence GET. Item 404 evicts its dependent rows. C3/C4 own foreground/lifecycle hooks calling
`sync.onForeground()`. C4 binds an application-owned live reader while trust is Ready, with durable post-refetch acknowledgements; see client-live.md. `Porchlight-TZ` is C1b's device zone; server
canonicalization remains authoritative.

## C5 collaboration extension

`HouseholdData.lists`, `.comments` and `.attachments` are now implemented. See [client-collaboration.md](client-collaboration.md) for typed flows, refetch/eviction hooks, occurrence scopes, replay keys, private binary transport and route/platform contracts.

## Cache and lifecycle

SQLDelight `PorchlightDb` (`app.porchlight.shared.db`) stores indexed identity/parent/expiry plus complete canonical
JSON in `CacheMember`, `CacheItem`, `CacheOccurrence`, `CacheAgenda`, `CacheBoard`, `CacheSettings`, `CacheReminder`,
`CacheWrite`; `Staging` and singleton `SyncState` hold snapshot generation and cursor/time. JSON retains versions,
capabilities, pending, relationships and reminders without lossy columns. Snapshot staging, all-table promotion,
visibility eviction, cleanup and cursor commit share one transaction; observers receive one immutable view only
after commit. Failure at staging or promotion rolls back the database and cursor. The snapshot has no agenda/board
rows, so `sync.refresh` first fetches every page of the board (current scope) and of the own agenda for the snapshot
window (now − 180 days to now + 548 days), then switches entities, cursor and both projections in ONE transaction
(R59); if any fetch fails nothing switches and `status` is `Failed`. Board/agenda rows of items no longer visible are
evicted by snapshot difference. Projection replacement is in place and scoped: the board is one scope, replaced whole;
an agenda refetch replaces only that member's rows its window provably covers, so other members' rows and rows
outside the window stay. A windowed occurrence refetch (`/v1/items/{id}/occurrences?from&to`, which never returns
undated occurrences) replaces the returned rows and deletes only cached rows of that item whose start provably lies in
the window (a one-day margin for wall-clock/date-only starts); undated rows and rows outside the window stay. Projection
filters/windows are not persisted; request them on entry, not inferred from surviving cache rows.

Cleanup removes occurrence/agenda/reminder rows older than 180 days; spanning rows use their end; undated rows stay.
Date-only occurrence and agenda rows expire at the end of the member-local day.

Disk load and SQL transactions execute on a serialized `Dispatchers.IO.limitedParallelism(1)` dispatcher under the
cache mutex, not the composition thread; the desktop cache file is
opened (directory, `PRAGMA user_version` check, possible recreate) on first use inside `initialize()`, never while
composing. `initialize()` explicitly loads a persisted cache; production binding does it before collecting trust.
Desktop compares `PRAGMA user_version` with `PorchlightDb.Schema.version`; Android's open-helper callback compares the
same version. Either way a mismatch (upgrade or downgrade) recreates only the disposable cache, never
preferences/credentials. A future SQL schema change must increment SQLDelight's schema version. Close serializes with
cache transactions; post-close mutations cancel.

Production cache factories recover physical SQLite corruption (CORRUPT/NOTADB) or invalid persisted JSON with at most
one destructive recovery per cache instance: close handles, discard only `read-cache.db` and its WAL/SHM/journal,
publish an unavailable view (`sync=null`), recreate, then force binding to resnapshot/reconnect even on a healthy stream. Settings, credentials, alarm codes and
delivery inventories are never reset. I/O, full-disk, busy and permission errors do not authorize deletion. Binding
and desktop alarm startup keep retrying initialization (1-second backoff, capped at 30 seconds) with a redacted storage
notice instead of losing their collectors; Android workers return retry. A failed recreation does not loop deleting.

Android disables backup and explicitly excludes every credential/device-protected domain from both cloud backup and
device transfer, including legacy full-backup rules. Settings DataStore, encrypted trust/pending enrollment, alarm
high-water and delivery metadata migrate into `noBackupFilesDir` before use. Private metadata is forced and atomically
renamed and its containing directory forced before legacy preferences are cleared or a commit is acknowledged; settings relocation likewise keeps its legacy file until the replacement is durable. An interrupted cleanup retries the directory force before legacy removal. Android trust metadata opens only inside suspend key-store operations, so migration/decode failures reach the controller recovery screen rather than crashing construction. Damaged
private metadata is never treated as disposable cache. A replacement device must re-enroll. JVM checks inspect the
merged debug manifest and packaged XML exclusions; an OEM device-transfer run remains a controller check.

`bindTrust` belongs to the application scope and is bound before `TrustGate` restores (composition order). Ready waits
for C2a endpoint/renewal refresh then snapshots. `sessionEpoch` is the household **identity** epoch: it advances only
when the identity owning the cache ends or changes — logout, entering Enrollment/ReEnroll (re-enrollment, self-revoke,
device revoked/unknown/expired) or a different device/member — and a durable cache clear is attempted (awaited by
the trust operation; the binding's epoch-change clear is a best-effort retry, not a guarantee). Restore (every
cold start), close, PIN relock, wallet/upgrade/local-trust retry gates, session expiry and a same-identity relogin do
not advance it: the persisted cache survives, hidden behind the gate, and offline launches read it. Restoring into
Enrollment/ReEnroll also clears (a process that died before an earlier clear). Generated responses/errors check
their captured epoch AND session token, so a late reply or 401 from an expired session cannot reach the new session.
Repositories carry one inherited coroutine lease across the whole request/pagination/refetch, checked before mutation
and before commit. A shared commit mutex serializes epoch advances (suspending, no `runBlocking`) with SQLite
commit/publication (never with network calls), closing the check-versus-commit race. Thus a response from a previous
identity cannot repopulate data after logout and rapid relogin. A PIN relock mid-request drops the reply as
`HouseholdLockedAfterReply` (retry writes with its key), and sync status returns to `Idle`, not a connection failure.
C4 must use `sync` operations (or `cache.inSession` around an entire stream job), not promote a response captured
outside its session lease.

Every identity end (logout, Enrollment/ReEnroll, another device/member) also awaits a durable cache clear before the
trust operation returns. So once `logout()` returns, the logged-out member's rows are deleted and committed, even if
the process dies at once, provided the clear succeeded. If it fails (for example a SQLite I/O error), logout still
completes; the binding's epoch-change clear can retry while the process lives, but is not a guaranteed backstop.
An already closed cache rejects both clears, leaving the logged-out rows on disk until a later successful clear
with an open cache (identity change or restore into Enrollment/ReEnroll). The deletion is at the application level:
SQLite may keep freed pages in the file (no `secure_delete`).

## Gates and controller smoke

Run the exact acceptance command in README and `CARGO_NET_OFFLINE=true scripts/ci.sh --sandbox`. CI checks:
authoritative OpenAPI precision (including every route's path parameters), real-handler recording equality, JVM
fixture consumption, common JVM/Android-host protocols, persistence, trust lifecycle, contract-check and
controller-only filtering. Only the snapshot cursor's validated non-negative integer value is volatile in fixture
equality; no fixed cursor is assumed. No server migration or SQL query change is needed.

Controller after merge: source `scripts/env.sh`, run README acceptance; `scripts/ci.sh --sandbox`, then normal
`scripts/ci.sh` and `scripts/ci.sh --full`. Use existing `client-trust.md` probes on API35/API36 and desktop:
enroll/login → unlock snapshot → write/reminder → pending/derived refresh → logout/re-enroll → rapid relogin →
foreground/relock. Confirm offline reads remain gated correctly and writes show an error, never queue. Real
mTLS/Secret Service/UI/emulator checks require the controller environment, not the sandbox.

## Review hardening: cache dates, version order and visibility

Occurrence cleanup uses effective post-override anchors (fixed locals resolved in the item's zone,
floating locals in the member zone), not the old generated recurrence date. Date-only values live through
their local day. Snapshot/Board/detail/Today and This occurrence editor seeds likewise use effective anchors;
generated identities and This and future split anchors remain unchanged. DST folds pick the earlier offset,
gaps the first valid minute, and collapsed spans preserve positive nominal duration (R10).

An agenda refetch replaces every cached row included by that member's requested window, including date-only
local-day overlap; other members and out-of-window rows remain. Per-resource item/occurrence/member/settings
individual responses never downgrade a stored version; keyed SQL lookups keep batch guards linear. Snapshots and
fully paginated member collections authoritatively replace rows, including LOWER versions after server restore.
Only higher-version receipts received after the request are re-applied, atomically, for resources still included.
A member collection keeps receipts needed by an older in-flight snapshot (e.g. C4's item tickle refreshes
members mid-snapshot), except pre-request receipts for rows authoritatively removed or replaced by LOWER versions.
Those restored-history receipts are invalidated so a subsequent write cannot restamp an old version. Genuine
post-request write guards remain; snapshot installs and identity `clear()` clear receipts.
Cursor regression discards old-epoch receipts: `sync.refresh` captures the receipt mark and the cursor installed at
that moment (`cache.snapshotMark()`) BEFORE requesting the snapshot, and only a snapshot whose cursor is below that
request-time cursor counts as a restore/rollback. The live cursor is never the basis: C4's stream acks advance it while
a snapshot loads (narrow occurrence/notification tickles are not serialized with refresh), and the snapshot's cursor
(the server's max stream seq when read) is never below a cursor acked before the request, so an ordinary resnapshot
keeps re-applying own-write, delete and visibility-denial receipts. Installing a snapshot sets the cursor to the
snapshot's, even if an ack had moved it higher meanwhile; a later reconnect resumes from it and re-delivers those
tickles (idempotent authoritative reads).
LiveSync's terminal stream `resnapshot` frame (and the cursor-less start) calls `sync.refresh(authoritative = true)`:
replace the cache without replaying receipts. Other tickle-driven snapshots (`item.changed`, `derived`, member/collab
events) stay ordinary refreshes. Missing resources evict.

Item GET not-found/forbidden evicts parent, occurrences, projections and collaboration children; detail/editor
locks when the parent disappears, including an already open editor; write outcomes and identical-request retry
remain visible. `cache.inSession { fence -> … }` captures the durable visibility generation before network work.
Every cache install API requires that explicit, unforgeable `ReadCache.Fence`; passing a different cache's or expired
request's token fails at the common transaction gate. Snapshot installation remains a single fenced commit.
Archived/omitted list entries advance per-entry fences across their stored scopes; nested delayed list/snapshot
replies cannot reinstall them. `RetainedCacheValue` gates list search results, open list-entry editors and planning
candidate cards against the current cache view on every render. Item editor generations and comment bodies use the
same gate; a redacted comment also closes its editor even if its placeholder remains cached. Planning unplaced
results keep their request-start visibility generation, and board drags retain only an occurrence ID resolved from
the current projection at drop. Write feedback retains outcome/identical-retry metadata after eviction, but clears
current/comparison values, saved representations and derived warning text. Identity clear invalidates retained
copies; same-identity storage recovery quarantines resource-dependent drafts until fresh authorisation. A successful item save retires only its submitted edit generation; newer typing remains visibly unsaved,
rebased on the committed version (also when the reply arrives off-screen or finishes a creation).
Every item,
list/entry/scope, comment/attachment, notification/suggestion and board/agenda commit checks its own and parent eviction generations atomically;
a late pre-eviction reply throws `StaleVisibilityReply` instead of restoring private content or clearing tombstones.
A successful read begun after eviction may re-establish access; its generation remains recorded so still-older reads
stay fenced, even after that restoration. Deleted representations, snapshot child omissions and covered-window removals also advance fences; capacity/expiry trimming does not. History is capped at 256 rows, with a monotonic in-process rejection floor protecting requests older than pruned history (a conservative stale reply can require a fresh request). Only current evicted IDs are published; the counter is seeded once on open, not scanned on each request. List redaction uses this same fence regardless of cleanup capabilities. Search never restores access: list hits are fenced at receipt and the screen discards retained results when the visibility revision changes, including when a redacted descriptor later leaves the cache. Snapshot installation filters post-request evictions too. A denial received during a snapshot fetch
is reapplied atomically so that older snapshot cannot restore revoked content. Connection/server errors and
step-up-required and 401 are not visibility revocations and retain offline cache. Search hits do not authorize access.
Visibility loss masks a peer conflict's identifying title and IDs as **Busy**, retaining its authorized warning kind,
member and time interval in SQL and Calendar; definitive counterpart deletion can remove the warning entirely.
Committed eviction file cleanup completes even if its caller is cancelled; removal failures are suppressed on the
original API problem, never substituted for it. ID refetch batches continue and report the first non-denial failure
only after trying remaining IDs. Windowed occurrence fetches capture a receipt mark before sending; omissions cannot
delete occurrence versions installed by a response or snapshot after that request. Direct unmarked batches
conservatively preserve own-write receipts. Fixed tzid-less todos keep override locals unresolved (no invented UTC
instants), including editor seeds. D03's date overlap lower bound is starts_at → due_at → local day start.

Occurrence eviction deletes Board by its occurrence primary key and Agenda/Reminder through indexed occurrence
columns; only normal immutable-view publication decodes unrelated projections. Cursor-only acknowledgements use the
same owner, epoch, storage-generation and commit gates (including external-reset detection), but read no payload
rows and update only `cache.sync`, not the presentation `CacheView`. `CacheView.sync` describes its last payload
publication; consumers needing the live replay cursor use `cache.sync`. Snapshot marks likewise use that live cursor.
Snapshot omission checks build ID sets once. Schema version 11 adds notification/planning parent, visibility-generation
and occurrence/agenda/reminder expiry indexes and recreates the
disposable cache on both platforms. Identity-binding file cleanup errors report `attachment-cleanup-failed` and
block new-session reads. Admission checks may retry at one-second intervals, but actual private-file purges honour
the durable one-minute-to-one-hour cleanup backoff; **Clear local cache** explicitly retries a verified wipe.
Explicit trust cleanup still propagates failure so the durable logout marker remains available for restart recovery.
