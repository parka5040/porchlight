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
- Generated APIs declare required parameters as non-null with no default, including required query parameters such
  as `/v1/agenda`'s `from`/`to` and `/v1/search`'s `q`. Optional parameters default to `null`.
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
snapshot carries reminder identities, scheduled reminders and creation metadata); that snapshot is staged with its board
and agenda like any other, and it queues behind any in-flight refresh, so it is always taken after the write. An
occurrence write stores the response, then refetches the affected entities (parent item, its occurrences in the
snapshot window) and both projections, replacing their rows in place in one transaction; a failed fetch leaves the
previous rows and returns `refreshPending = true`.

A snapshot requested before a write can arrive after it. Two guards keep that older snapshot from undoing the write:

- **Receipts.** Item GET/write and occurrence write responses, member PATCH and settings responses (including a settings GET), and item
  deletions, received after the snapshot was requested, are re-applied inside the snapshot's install transaction.
  - A response replaces a row **the snapshot holds** when its version is higher.
  - A row the snapshot lacks is never re-added. The snapshot carries every undated occurrence of a visible item, so
    such a row was deleted or reparented on the server.
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

Item writes lack creation metadata and reminder IDs. Response conversion preserves the available metadata and the
matching reminder rows provisionally, replaces explicit nulls and stores the real offsets; production writes then
resnapshot to reconcile authoritative identities (even for unchanged offsets). If that refetch fails, rely on the
write's offsets and `refreshPending`, not on provisional reminder IDs or timestamps.

Disk load and SQL transactions execute on `Dispatchers.Default`, not the composition thread; the desktop cache file is
opened (directory, `PRAGMA user_version` check, possible recreate) on first use inside `initialize()`, never while
composing. `initialize()` explicitly loads a persisted cache; production binding does it before collecting trust.
Desktop compares `PRAGMA user_version` with `PorchlightDb.Schema.version`; Android's open-helper callback compares the
same version. Either way a mismatch (upgrade or downgrade) recreates only the disposable cache, never
preferences/credentials. A future SQL schema change must increment SQLDelight's schema version. Close serializes with
cache transactions; post-close mutations cancel.

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
A member collection install does not consume member receipts: an older snapshot still in flight (e.g. C4's item
tickle refreshes members mid-snapshot) re-applies them; only snapshot installs clear receipts.
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
remain visible. A successful item GET/put clears prior denial receipts. A denial received during a snapshot fetch
is reapplied atomically so that older snapshot cannot restore revoked content. Connection/server errors and
step-up-required and 401 are not visibility revocations and retain offline cache. Search hits do not authorize access.
Committed eviction file cleanup completes even if its caller is cancelled; removal failures are suppressed on the
original API problem, never substituted for it. ID refetch batches continue and report the first non-denial failure
only after trying remaining IDs. Windowed occurrence fetches capture a receipt mark before sending; omissions cannot
delete occurrence versions installed by a response or snapshot after that request. Direct unmarked batches
conservatively preserve own-write receipts. Fixed tzid-less todos keep override locals unresolved (no invented UTC
instants), including editor seeds. D03's date overlap lower bound is starts_at → due_at → local day start.
