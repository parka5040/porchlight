# Global transaction lock order

**Order:** idempotency claim → calling device (`FOR NO KEY UPDATE`) → unlocked session/member/user authorization reads → user credentials → session writes → items → occurrences → derivation → affected `household_members` rows `FOR NO KEY UPDATE` in id order (R7; the app role cannot use advisory locks). Never `households … FOR UPDATE`.

- Claim the idempotency key first in the actor-attributed transaction. Header authentication precedes the claim; authority is revalidated after claim waits, including replays. Never weaken authentication to replay terminal operations.
- Revalidation takes the calling device row **first with `FOR NO KEY UPDATE`**. Same-device mutations serialize at that first lock; no `FOR SHARE`-to-update upgrade cycle. Credential issuance/revocation use the same device protocol. Sliding session touch locks the device first with `SKIP LOCKED`, then conditionally updates the session (best effort).
- **Device-vs-device rule:** an app transaction holding its calling device must acquire another revocation target with `FOR UPDATE NOWAIT`, never wait for it. SQLSTATE `55P03` aborts/rolls back the transaction and becomes 409 `conflict` with `Retry-After: 1`; neither the claim nor the consumed step-up commits. Self-revoke upgrades only its own already-serialized row. Agent revocation holds no calling device and may wait on its single target. Multi-device deletion (S5) must acquire affected devices in id order before member/user writes; an app caller already holding its own device uses NOWAIT for every other device, even when the caller id is outside that order.
- SSE heartbeat session sliding (S6b) reuses the existing actor-attributed device-first `SKIP LOCKED` touch; it never retains a transaction/connection between frames. ACL/replay reads take no row locks; shutdown drains without acquiring domain locks.
- Session validation is an **unlocked check** while holding the device, not a retained session-row lock. Never hold a session lock across `lock_credentials`: another device's password change may revoke that session while this path waits on credentials. After acquiring credentials, recheck the session before doing credential-dependent work. Session writes/revocations are terminal work after these checks. Logout/self-revoke exemptions are registered terminal metadata, not URI string comparisons; privilege-sensitive operations still revalidate the role.
- Member/user authorization is also an **unlocked read** inside the transaction (R18), for both members and PATs. Do not hold `FOR SHARE` on `household_members` or `users` during revalidation: R7 conflict checks and member self-updates would upgrade it and deadlock. The calling device remains the serialization point; S5 user deletion revokes the member's devices in the same transaction before deleting the member/user, so it serializes with this device lock. A member deleted during body parsing is an authentication failure (401), not a side-query 500.
- User credential writes lock `user_credentials` and share that serialization for login, reauth, enrollment, password change and reset. Multi-row session revocations never acquire a device after a session.
- Domain paths lock items in ascending id order, then occurrences in ascending id order, then their `item_derivation` rows. Relationship triggers lock/bump the item before enqueueing. **Raw assignee/viewer/reminder INSERT/DELETE** uses a BEFORE-row parent `FOR NO KEY UPDATE NOWAIT` guard (0061): DELETE already owns its child tuple, so it must never wait for a parent held by a scope replacing those children. Bulk raw writers prelock their complete parent item set NKU ascending, or roll back/retry the whole transaction on 55P03; the AFTER-statement bump only reacquires those held parents. API/revert relationship replacement writes set differences only, preserving unchanged child identities and assignment history. Occurrence-only writes lock the occurrence then derivation without locking the item. Never lock an item/occurrence while holding its derivation row.
- **S3 conflict/write paths:** after S1 locks, call `items::WriteScope::begin(&mut Transaction, household, ScopeRequest)` exactly once for the complete domain operation. Declare target items, specific edited occurrences, explicit NEW agenda members (all intermediate and final relationship/field sets), closed `reopens` (possible regeneration deletion), split boundaries and planned board bounds/status. Planning reads are unlocked: affected members are the union of core authoritative old/new membership (assignees + subject, owner only as empty-set fallback) and existing target `member_agenda` membership. Pending live peers are selected through either authoritative core membership OR stale agenda rows. Soft-deleted items are excluded except explicit targets and member-zone dependents. Zone plans include every related owner/subject/assignee item (including deleted rows to serialize undelete), whether pending or not. The trigger currently enqueues live items only.
- Acquire target + peer items **ascending id, `FOR NO KEY UPDATE`** (no item keys change/delete); then only their open occurrences plus specific edits/split future rows/board rewritten run, **ascending occurrence id in one `id=any($1)` statement**. If any row can be deleted/reparented, that scan takes `FOR UPDATE`; otherwise NKU. Closed history outside a split/edit is not locked; split/all at seq 0 declares no history boundary. Then acquire their derivation rows **ascending item id, `FOR UPDATE`**, then all affected `household_members` (including soft-deleted ones) **ascending id, `FOR NO KEY UPDATE`** (R7); zone scopes upgrade ONLY declared `member_zones` rows in a final sorted `FOR UPDATE` statement to block FK KEY SHARE checks on new owner/subject/assignee references, never co-members. Inserts and
changed referencing keys require that check; an UPDATE preserving a previously validated FK need not block. Explicit zone primitives are enqueue-only, so they never write/derive new FK references after that final lock. Board-only moves never add parent items as targets: only mover + rewritten projection run NKU, no item/job/member/peer locks. Unwritten bounds and outer keys are read unlocked, not locked. No household-row/advisory lock.
- After member acquisition, re-plan **unlocked**. If membership, pending peers, occurrence IDs or the planned board neighbourhood changed during a wait, rollback the acquisition savepoint (releasing its domain/member locks) and restart. Only three attempts are allowed, then `derive::Error::RetryableScope`; S4 maps it to S1 transient 409/Retry-After. Unrelated column/future-occurrence changes do not invalidate a board plan. No existing item/occurrence/job may be newly locked or scope enlarged after members. New item/occurrence/job rows (including split tails) belong to this transaction. Closed/detached anchors bypass no-op occurrence upserts, because PostgreSQL ON CONFLICT would otherwise lock them even when its WHERE is false.
- Compose only methods on the retained `WriteScope`: update + replace relationships, split + tail update, status + board move, etc. An undeclared closed non-detached reopen is rejected before writing, preventing a late NKU→UPDATE upgrade. Reread versions through `scope.item`/`scope.occurrence` after authoritative writes. `finish(now)` drains pending peers in sorted order, each in its own savepoint; a failed peer rolls back, logs only ID/redacted category, remains pending with retry/last_error untouched, and its stale agenda is excluded from returned conflicts. Terminal peers (attempts >= 5) are not locked/drained and are excluded too. Dirty targets derive once; conflict results are recomputed only after ALL successful derivations. Board-only writes never derive, even if their parent is pending. Split captures moved agenda member/local dates for finish's recompute set and transfers reminder obligations without changing their copied default/explicit delivery identities under held locks (0068); the unlocked reminder producer may commit the old head snapshot, but its dedupe key is identical to the tail's, including after clock rewind; it explicitly derives head before tail, independent of UUID order; reparent emits occurrence.changed per moved row. Caller must rollback/savepoint ALL domain effects on ANY error, including business validation errors, before S1 seals a response. A transaction-local marker ENFORCES one scope: second begin before/after finish returns `ScopeAlreadyUsed`; finish consumes self (no use-after-finish). Rollback of the whole domain savepoint releases locks and marker. Compatibility free functions are single-operation wrappers: never compose them.
- Worker: unlocked due candidates → single item `FOR UPDATE SKIP LOCKED` → only its open occurrences in id order `FOR UPDATE NOWAIT` → its derivation row `FOR UPDATE NOWAIT` → affected authoritative/stale-agenda members (including soft-deleted ones) in id order NKU NOWAIT → savepoint derive/ack (rollback the savepoint on failure) → retry bookkeeping/event. Candidates are rechecked after item lock. A raw closed-occurrence reopen cannot commit between the open-row scan and job acquisition: its synchronous task_events insert takes item FK KEY SHARE, blocked by the worker item UPDATE lock (covered by the worker raw-reopen overlap regression). Acquisition/recheck failures log only ID/redacted category: 55P03 contention at DEBUG, all other failures at WARN; each rolls back only that candidate and continues the batch; a raw child lock never stalls the rest. No job-first election, and no later new existing-domain locks while holding member locks.
- Time reports: separate transaction, member/device state read unlocked first; unchanged reports take NO row lock. Only a needed change takes requesting-device NKU, rechecks live device + member/authority state, then an enqueue-only member-zone WriteScope (related items/jobs, zone member UPDATE; NO occurrences/co-members/peers) → member update/trigger enqueue → commit. Including deleted related item locks serializes undelete. New owner/subject/assignee FK inserts block at the member UPDATE lock; re-plan before the trigger. Reports leave derivation asynchronous; no locks survive into the request's claim transaction. Explicit zone/source/authority primitives retain their injected `now` signatures but use the same enqueue-only scope as reports; `finish` on an enqueue-only scope seals its marker without derivation/conflict reads. Derivation happens in the worker or the next ordinary scope. No zone primitive acquires co-members or drains peers while holding the zone member UPDATE lock. Authority-device validation is unlocked, never a late second-device lock. Device revocation clears its member's authority without changing tzid/enqueueing domain work: already-held device → final member NKU, so the next live device report can claim authority. **S5: domain WriteScope work MUST precede device revocation in the same transaction, or use separate transactions; revocation takes the member lock.**
- Scheduler: register jobs once per runner, read all due names in one unlocked query per tick, and open actor-attributed election transactions only for due jobs; recheck and elect the job row `FOR UPDATE SKIP LOCKED`. Generic callbacks retain atomic effects + bookkeeping. Built-in sweeps use an independent one-slot operational pool (`porchlight-scheduler-items`): each item has its OWN actor-attributed transaction and acquisition/compute/enqueue SAVEPOINT, item first NKU SKIP LOCKED → job NOWAIT only if an enqueue is needed; commit releases ALL its domain locks before the next candidate. The job election holds no domain locks across the sweep. Per-item failures log only the ID, roll back and continue; pending/terminal worker attempts are never reset by sweeps. No domain writer locks scheduler rows, no scheduler waits for an item while holding item_derivation, and no household row lock. Registration of an existing job is an unlocked existence check. A crashed/interrupted sweep retains already-committed items, rolls back only the active item, and is safe to rerun; generic failed callbacks roll back and use capped exponential backoff.
- Agent PAT authorization takes its PAT `FOR NO KEY UPDATE` in the claim transaction; a PAT revoked before transaction authorization cannot act. **Token-revoke API** claims its idempotency key, then reserves caller + target PATs `FOR NO KEY UPDATE` in UUID order **before** locking/revalidating the caller or reading member authority. This bounded queued wait (the ordinary 5 s lock timeout) lets a revoke get ahead of later claims on a continuously busy target **only under bounded concurrency** (the queue ahead must drain within that timeout), without a reciprocal caller→target cycle; it never polls for an idle gap. Sustained saturation can still return 409; no unconditional revocation-win guarantee is made. After reservation it revalidates even on replay. The low-level revoke retains target NKU NOWAIT for other compositions already holding their caller (for example user deletion); a missing ordered reservation must never cause a late cross-PAT wait. A timeout is transient 55P03 → 409 `conflict` with `Retry-After: 1`, and rolls back the whole claim. `porch token revoke` makes at most three attempts with the same idempotency key, honoring numeric 409 Retry-After delays of 0–2 seconds (at most four seconds total sleep); missing/invalid/longer headers or other statuses are not retried. Each connection/request retains its ordinary 15 s transport deadline. Self-revoke reacquires its own row and remains terminal. NKU is compatible with collaboration attribution FK KEY SHARE.
- **Calendar source configuration/removal:** source lock → complete live source item WriteScope → member locks → source update. The source configuration trigger (0064) locks only live items it can update, already declared by the API; deleted source history is never a late item lock after members. Member-zone scopes still include deleted related history. Calendar apply admits its cumulative work budget before acquiring domain locks and declares closed target occurrences before any exception edits.
- **Local admin-token recovery:** no caller PAT/device or idempotency claim. Verify the installation/household, then hold only the selected live super-admin member and user `FOR SHARE` through a terminal new-PAT insert/commit. Never upgrade these locks or acquire credentials, existing PATs, devices or domain rows afterward. Issuance is attributed to `local-recovery` with the new credential's id, not a prior PAT authorization.
- No household row lock is a serialization shortcut. The 5 s lock_timeout applies per lock attempt, not per scope/request; 30 s statement_timeout bounds an individual statement, not an entire multi-statement transaction.
- **COMMIT of a NOTIFYing transaction** (every outbox trigger, `pl_enqueue_derivation`, the projector) first takes PostgreSQL's NOTIFY-queue lock (an `object` lock on `pg_database` objid 0 in shared database 0: one lock for the whole cluster) and holds it through the WAL flush, so these commits serialize across all databases of the cluster. `lock_timeout` bounds that wait too: a 55P03 at COMMIT is the ordinary transient 409. Tests that shorten `lock_timeout` to provoke a domain-lock timeout must restore it before any NOTIFYing transaction they expect to commit, and lock-overlap probes wait for backends blocked behind the holder's pid (`testing::wait_blocked_behind`), never for any `Lock` waiter in the database.

## S7b collaboration and blob locks

Collaboration mutations retain one `WriteScope::begin_at` inside the S1 domain savepoint; parent item locks
serialize current ACL checks and child changes. Scope acquisition/finish precede comment/attachment writes, and
no further items/occurrences/members are locked after it. Failure rolls back all domain effects before sealing.
After domain locks, uploads first lock permanent shard `00` as the volume-wide admission gate, then their complete set of `blob_storage_locks.shard` rows in ascending
hex-shard order, then upsert main + thumbnail blob hashes in ascending hash order BEFORE filesystem writes and
attachment insertion. Under that gate, committed metadata plus missing main/thumbnail bytes must fit the configured household quota. A separate process-local quota mutex protects a physical-byte ledger plus pending-byte reservations: all blob-root files (including fresh orphan/temp files) and pending writes must also fit. Only startup and GC reconcile the entire tree. Per-chunk admission is constant-time; writes, replacements and unlinks update the ledger under the same mutex, including partial writes and failed cleanup. This mutex never spans a SQL await; staging takes it without database locks, canonical admission takes it after shard gates. Blocking writers own quota/free-space reservations through HTTP cancellation; completed files remain charged until actually deleted, including after restart reconciliation. GC shares this ledger with uploads and reconciles without discarding pending reservations; all unlinks (including temp cleanup) retire charges only for removed bytes. The fixed 256 shard rows are never deleted, so file exclusion survives blob-row deletion.
Comment deletion prelocks the UNION of its live attachment hashes before its bulk soft delete; single-attachment
reference triggers sort both hashes. Raw-SQL bulk writers must likewise prelock their whole hash union, not
alternate individual row triggers in arbitrary order. Raw filesystem writers must use these same shard gates.
Comment attachment creation is an author-only comment edit within the inclusive 15-minute window (no super-admin
window bypass), rechecked under the item scope; it bumps the comment version/edited_at but not the item version.
Deleting a comment attachment is also a comment edit: author within the window or super admin moderation; the API
bumps the comment version under the same item scope. Every edit-window decision (comment PATCH, comment attachment
create/delete) reads the injected clock again AFTER its last gate wait — the item scope, and for uploads also the
shard gates — and writes that same instant, so queueing on those gates never extends the window (m6fix M1).
A blob-row lock taken after the decision (the attachment reference-clock trigger on delete, the metadata upsert on
upload) can still wait behind another item's same-hash upload, purge/retention or a lease; that wait is bounded by
the 5 s `lock_timeout` (R75). Rate limits
and the two-temp-per-device/PAT cap precede body streaming; ingest admission happens after streaming, before opening
IdemTx, without holding a pool connection.

GC adopts/locks candidates in ascending `(household_id, sha256)` order, then rechecks current attachment references
and leases. Backup `pl_take_blob_lease(holder, duration, now)` first SHARE-locks blobs in that same order, then
captures all currently live main/thumbnail references in a new statement snapshot. It defaults to 6 hours and
rejects nonpositive/>24-hour durations; callers must renew by taking a new lease before expiry if copying longer.
No upload or lease writer acquires scheduler rows. The generic scheduler transaction elects the runner, but GC
uses independent pools and per-candidate transactions: filesystem shard gate → blob metadata lock/recheck → guarded
blob DELETE → metadata COMMIT → file unlink + shard fsync → release gate. A second one-slot metadata pool avoids
size-one pool starvation while the gate is retained; both pools are retained by the scheduler, not built per sweep.
Shard lock timeout logs category `shard_lock_timeout` and skips that shard while continuing the sweep.
No GC path locks an item or attachment after blob acquisition.
Cancellation before the metadata COMMIT rolls back the gate and metadata transactions. The phase metadata COMMIT →
unlink + shard fsync → gate release runs in its own Tokio task that owns both transactions (the sweep pool's single
connection is borrowed per candidate, not held across the sweep), so dropping the sweep (scheduler shutdown while HTTP
drains) never releases a gate while its blocking unlink is pending (m6fix I2). A crash after DELETE leaves only an
unreachable orphan that can be adopted by a later sweep, never a live missing file. Per-file I/O errors log a path
class and do not abort other candidates. Downloads open only ID-authorized canonical blobs; Linux open handles survive
unlink.

**Deployment invariant: one household per data volume.** The hash-only byte paths and shard gates do not support
multiple household databases sharing a volume. Orphan adoption deliberately skips ambiguous multi-household volumes.

## S8a calendar import/feed composition

- After S1 claim/device/PAT authorization, acquire the single source row `FOR NO KEY UPDATE` before planning any
  domain writes. All calendar operations use source → one retained item WriteScope → occurrences → jobs → members.
  No item writer later acquires a source row; setting the feed FK takes only the compatible source key-share lock.
- Preview and commit share the same caller ACL/validation/conflict masking adapter inside one domain savepoint.
  Authorized planning item versions must still match under the retained scope locks; otherwise roll back and return
  the unsealed transient 409. A cancelled item can be restored only with its current edit capability.
- The scope declares every existing closed/managed exception that can be changed/reopened before member acquisition.
  Calendar materialization admits genuinely new rows only; out-of-horizon exception anchors never lock an undeclared
  existing occurrence. All item/occurrence methods use the retained scope, not a second free-function scope.
- Scheduler `refresh_ics_sources` uses the independent operational pool; due-source reads are unlocked, then each source
  takes its own system-attributed transaction/source NKU `SKIP LOCKED` and rechecks due/enabled state. The outbound
  fetch holds only that source lock (20 s total cap), then domain writes use a rollbackable savepoint. Each source commits
  before the next; failures preserve domain rows and record only a static status/error. There are no domain locks
  across network I/O, and request callbacks never fetch or acquire a second connection.

## S8b planning locks

Planning mutations call one begin_at/finish scope before touching availability/suggestions; the scope owns sorted
item/occurrence/job/member locks and drains pending peers. Suggest/availability declare the member; accept declares
the item, occurrence and member. Suggestion/availability rows are terminal work, never followed by new domain locks.
Availability locks existing windows by ID before its profile (matching raw child→profile triggers), rechecks the
collection version after profile acquisition, then replaces windows. Raw availability row writers take the
member NKU **NOWAIT** before profile/queue bookkeeping (0046). They already own a child row, so they must abort
with 55P03 rather than wait for an API/worker's retained member lock; retry the whole raw transaction.
This prevents child→profile/queue versus scope member/queue→child cycles and serializes new window inserts.
Raw availability triggers never lock items.
Member-only load jobs select candidates unlocked → member NKU SKIP LOCKED → queue FOR UPDATE NOWAIT → projection and
conditional ack. They never take item/occurrence/derive-job locks. Peer derivation only collects old/new local dates;
finish recomputes ONCE for held members and inserts fresh-ID immutable member_load_signals for unheld peers (no
member lock/shared queue-row upsert). Only the member-only worker folds/deletes signals, so two scopes draining
{X,Z}/{Y,Z} cannot deadlock on Z's queue/projection. Pending availability jobs force a full refresh; ordinary item
changes touch changed dates only. Queue generation acknowledgements retain the captured generation check. Scheduler planning uses independent per-member transactions
on the existing one-slot operational pool; scheduler election holds no domain locks across members. Accept rechecks
zone/capabilities/status/version after acquisition; floating eligibility is recomputed in the locked recipient zone,
not taken from the aggregate board disposition. Errors roll back the domain savepoint before sealing.

## Lists (S7a)

Lists do not modify item/occurrence/derivation inputs, so they do not open a competing WriteScope. Within the S1
IdemTx domain savepoint, list HTTP mutations take the attached item NKU (if any) → parent list NKU → target entries
NKU (ascending id for clear). Creation checks edit under the item lock; all other writes recheck authoritative ACL
under the parent locks. No member, derivation or household lock is taken. All effects roll back before sealing a
business error, and stale resources are attached only after rollback. One-connection HTTP tests cover every callback.
List moves serialize on the parent; ordinary fractional moves update one entry. An exhausted/tied gap spreads
only the minimal run between unchanged external keys (core ordering, S3's spread algorithm), expanding only if
those bounds are exhausted too. The run is prelocked in ascending id with NOWAIT, preventing a cycle against
raw child-first writers or a move's already-locked target; busy raw rows cause retryable 55P03, never a waiting
inversion. Unrelated entries keep their keys, versions, revisions and tickles. Two-connection HTTP insert tests
exercise the same neighbours in both request orders ×20 with distinct keys and no deadlock.
Wire after_id means predecessor, before_id successor, including hidden-free standalone order.

Raw entry/check triggers never write/lock the parent list (their own versions/revisions/tickles are sufficient).
Check INSERT/DELETE takes its entry NKU NOWAIT in the BEFORE-row guard (folded into draft migration 0030). A raw DELETE already
holds a check row, so waiting on the entry would invert the HTTP entry → check order. Bulk check writers prelock
entries ascending or retry 55P03; a rejected write leaves checks, versions and revisions unchanged.
Raw viewer INSERT/DELETE may already hold a child row lock; its BEFORE-row parent NKU uses NOWAIT, preventing an
inverted parent wait against an HTTP writer. Raw bulk relationship/move writers should prelock their parent lists in
ascending id and entries in ascending id or retry SQLSTATE 55P03. Statement viewer triggers then bump already-locked
parents. No new item/occurrence lock is acquired after a member lock. List checks intentionally do not lock occurrences:
the insert guard verifies parent/split-descendant membership via S3's split_from_item_id chain, and historical
checks may outlive regeneration. Collection filters are authoritative, not derivation-based; SQL and Rust read ACL
agreement is asserted by the full endpoint matrix, with Rust-hidden rows skipped in collection/seam loops.

## Transient retry policy

SQLSTATE **55P03** (NOWAIT or bounded lock-timeout expiry), **40001** (serialization failure), and **40P01** (deadlock) map to **409 `conflict` + `Retry-After: 1`**. They are logged at **warn** with only the SQLSTATE and a static classification; no SQL text, constraint, credentials, request data or database error message is logged. This is a runtime recovery policy, not permission to violate lock order: every deadlock needs investigation.

Mutations roll back **all** effects, the claim, consumed step-up and pending post-commit publications; **nothing is stored** and the client may retry the **same idempotency key and body** after Retry-After. Never turn a transient `ApiError` into a callback success/response; propagate it as `Err` so no sealing SQL runs on an aborted transaction. Ordinary business conflicts, even with Retry-After, are stored and replayed. GET handlers and header authentication/session/PAT-touch SQL may also return this unstored 409; the shared OpenAPI problem set documents it on reads as well as writes.

Callbacks must do **all SQL through the provided `IdemTx`**, never `state.pool` or a separate transaction. State provides non-SQL settings/limits/hub/scheduler services. Work outside IdemTx would survive rollback and be repeated on retry.

N1 regression tests send renewal + logout, login + renewal, password change + renewal, two same-device mutations, and revoke + renewal in both submission orders, twenty repetitions each. Password change + renewal must both succeed; a newly issued certificate must be in the revoked fingerprint set if revocation follows it. App cross-revoke and two members taking both R7 member locks also run in both orders twenty times. Internal database details are never exposed to clients. N1 pair and R7 member-lock tests continue rejecting 409 as well as 500, so a deadlock cannot be masked by the runtime retry policy. Cross-revoke may accept a NOWAIT conflict, but its captured SQLSTATE warnings must contain only 55P03 (never 40P01 or 40001), one per 409.

## Board contract (R30, S4a/S4b)

`WriteScope::move_on_board(occurrence, version, before?, after?)` is predeclared with `BoardMove` including destination
status. Use only `board_moves` for a same-column board-only operation; declare `occurrences` too ONLY for status/override
edits. Domain = household next open per series + Done completed in last 7 days, regardless of caller visibility.
Order is `(board_position COLLATE "C", id)`. With before: insert immediately after it, using its NEXT domain neighbour;
with only after: immediately before it; neither: end. Any ordered pair is accepted, not just adjacent visible cards.
Done membership uses one frozen clock for planning, rechecking and moving. `WriteScope::begin_at(tx, household,
request, now)` is the additive public injected-clock constructor used by HTTP writes and the free wrappers;
pass the same instant to mutations and `finish`. `WriteScope::begin` retains its unchanged convenience signature
and captures server `Utc::now()`. `finish(now)` does not retroactively change a board plan's clock.
Wire `after_id` is the predecessor (DB before); wire `before_id` is the successor (DB after). HTTP body bounds must
reference visible cards in the destination column: missing, hidden or wrong-column ids uniformly return 422 with
`invalid-reference`; visible same-column cards no longer projected return `stale-neighbour`. These reference checks
do not narrow the household ordering/locking domain: hidden cards still participate in gap selection and respread.
After validates ordering when before is present. Hidden future/expired Done rows never move. Only the minimal tied
projection run is respread when it fits; if exhausted, expand outward as in `ordering::insertion_run` until it fits. All expanded run IDs enter the original pre-member acquisition plan; remaining outer bounds are read, not rewritten/locked. A changed composed neighbourhood requiring
new locks yields typed `RetryableScope`, not a programmer `Validation{field:"scope"}` error.
Only the explicit mover is marked `board_moved` and force-updated; automatic respread bystanders update only if their
key changes and then emit `occurrence.changed`. An unchanged-key bystander keeps its version and R31 untouched state.
The board-mover transaction marker is restored after respread, so later raw board writes remain explicit moves.
S4 must roll back all domain
effects before sealing a transient response. Conflicts remain raw: mask BOTH other_item_id and other_title.

## Administration and audited purge (S5)

User edit locks live super-admin members plus the target **FOR NO KEY UPDATE**, in ascending id order,
then rechecks If-Match and the last-admin invariant. NKU serializes the invariant without conflicting with
owner FK KEY SHARE checks during item creation (R7). No household serialization lock is used.

Deletion starts with the already-claimed caller PAT. It takes the target's **cross-PAT NKU NOWAIT** locks
in id order before devices, so mutually deleting agents never wait in a PAT cycle. Then devices UPDATE in id
order → credential row → device sessions → live admin/target members NKU in id order. It rechecks versions and
PAT/device membership sets before revoking devices, all PATs and enrollment tokens, and soft-deleting user/member.
Revocation clears timezone authority without enqueuing item work. Post-commit notifications close connections;
no item WriteScope is opened while device/member locks are held. Domain business errors roll back the savepoint.

Purge first locks selected rows in every planned deletion relation (R56), starting with items UPDATE in id
order → occurrences UPDATE in id order → derivation rows UPDATE in id order. Only then does it inspect the
FK catalog and probe dependent rows; a dependency rejection precedes audit and deletion. List child deletion triggers may lock list/entry parents; they follow
the selected item locks. Counts and `purge_log` insertion precede child deletion. Projections and list children
are removed before their parents. Notification events are deleted first (0063: before projections and
occurrences, so no occurrence-only event survives); history/jobs are deleted last, before items. Trigger-
generated history is also removed. The whole audit + deletion is atomic. Physical deletion needs UPDATE locks,
not NKU. O2's counted extension locks notification_events and notifications before source/delivery deletion,
comments before attachments, then referenced blob rows in household/hash order before resetting their GC clock.
The same planned-parent inbound-FK guard protects every deletion parent against unhandled FK insertion.
0071 rechecks that parent immediately before DELETE, including rows produced by earlier child triggers; it
acquires no new relation locks at that stage. Trigger obligations and unsupported deferred/parent-delete
reference creation are specified in derivation-matrix.md.

O2 nightly retention: old notification_events UPDATE sorted id → old
notifications UPDATE sorted id → expired sessions UPDATE sorted id, per household UUID order; then explicit
source/delivery/step-up children precede parents. Only after all event/auth cleanup: `stream_assign_mutex` UPDATE → outbox prefix prune → retention epoch update. No path takes notification/event locks after this mutex. Grants on notification_events(id)/sessions(id) permit the
row locks without granting unrestricted maintenance updates. Counts plus purge_log and stream_retention epoch
advance commit together. Ordinary worker deadlock/serialization failures roll back and retry through scheduler.
No short lock_timeout is left on a connection that later commits a NOTIFY.

Backup/export leases commit in an independent transaction before their read-only RR snapshot. Acquisition
locks blobs SHARE in household/hash order; `pl_renew_blob_lease` locks its pinned blobs in that same order then
updates the lease only if still unexpired. GC takes blob UPDATE, then checks live leases; expired leases never
revive. Export opens regular blob descriptors before returning the bounded gzip stream, releasing on completion
or disconnect; backups renew on progress and again after encrypted hashing before publication. Crash leases
expire after six hours. The complete O2 inventory/lifetime contract is in [data-out.md](data-out.md).

Derive retry locks the item NKU before updating its derivation row; it never takes a job-first election lock.
Revert uses the ordinary WriteScope item → occurrence → job → member plan before restoring the snapshot.


## S6a notifications

- Prefs replacement claims/revalidates through S1, then (only when default offsets change) ONE enqueue-only
  WriteScope with member_zones prelocks all related items/jobs in global order, then the member row. Finish seals
  that scope without deriving. Lock `notification_prefs` last, recheck version, then UPDATE. Category/quiet-only
  replacement takes the member FOR KEY SHARE before prefs; no-op writes nothing. Raw SQL agents have SELECT-only prefs; owner/maintenance
  migrations changing offsets must follow item/job-before-prefs order. The enqueue trigger takes no new item locks.
- Notifier: unlocked ordered recipient candidates → member FOR KEY SHARE SKIP LOCKED → recipient
  notification_prefs FOR UPDATE SKIP LOCKED → unlocked chronological batch discovery → referenced items
  FOR KEY SHARE SKIP LOCKED, UUID order → ALL surviving batch notification_events FOR UPDATE, UUID order →
  ALL candidate existing notification rows FOR UPDATE, UUID order → source INSERT and processed_at UPDATE,
  all atomically. Candidate rows cover batch digest quiet-end identities (including released/read rows) and
  unread non-digest windows overlapping the batch's first/last event times. New notification rows belong to
  this transaction; prefs prevent another consumer introducing a new target. Processing order remains
  created_at/id, distinct from lock order. Member and item reservations NEVER wait while holding prefs:
  a busy/missing item rolls back and defers the entire recipient. Item reservations precede events so a later
  notification INSERT's item FK cannot wait on a purge item holder while retaining its event locks.
  Item NO KEY UPDATE mutations remain compatible. Prefs serialize consumers and digest identity lookup
  includes released/read rows. Event and candidate notification locks are outside per-event savepoints;
  acquiring them incrementally would invert purge's event → notification or notification-id order after
  an earlier successful merge (N5/I2). Read-all locks its visible released unread rows in UUID order in a
  materialized CTE before its multi-row UPDATE; it takes no event locks.
  Physical purge locks item UPDATE → event UPDATE sorted id → notification UPDATE sorted id; nightly
  retention likewise locks events before notifications. The notifier takes no occurrence/job/household locks.
  Source ACLs are unlocked authoritative reads.
  Release takes member KEY SHARE before the same prefs serialization, rechecks Digest (deleting suppressed
  held digests/source links, not released history), then locks all unreleased notifications in UUID order
  before DELETE/UPDATE; it takes no event locks and
  waits for currently due unprocessed backlog. Each recipient has its own
  transaction, with one SAVEPOINT per event. Event failures roll back link/notification/outbox/processed_at together,
  then write notification_event_delivery metadata under the same prefs serialization (including its event FK KEY SHARE).
  After five failures the event is marked poisoned/skipped, allowing backlog and digest release to proceed.
  Recipient infrastructure/release failures roll back the recipient transaction and back off durably in
  notification_delivery_retry (2–60 seconds; exponent capped at 6); successful consumption or actual release clears it.
  Backoff writes take member KEY SHARE before retry-state rows. Logs contain UUIDs and safe error classes only.
  One failed recipient does not bypass healthy delivery or the separate release pass. Multiple workers can run safely.
- Reminder/dependent scheduler callbacks use the existing scheduler_jobs election and only unlocked current domain
  reads plus the shared definer event emitter. No late domain/item locks, no prefs-row locks, no separate callback
  transactions. Read/read-all locks only recipient notification rows, never prefs then items. Transient SQL states
  propagate through the existing unstored 409 mapping; callbacks use only IdemTx.

## Security history and issuance budget (0070)

Security-event INSERTs occur inside their caller's transaction, after existing auth/device/member locks.
Actor/target IDs deliberately have no identity FKs: event insertion adds no member/device/token lock edge.
Retention DELETEs only security-event rows under the existing maintenance plan and records purge counts.
Certificate renewal checks indexed issuance history while holding the live-device lock (same lock as revocation
and issuance). It never locks a certificate row then waits for a device. Same-key replay precedes the budget check.
First authenticated use of a leaf (0071) also locks the device, rechecks certificate/device revocation and expiry,
then sets `first_seen_at` once. Already-seen leaves need no new auth write. Unused-successor revocation runs under
the renewal's same device lock, after CSR validation and before replacement insertion. Only leaves issued with
`usage_tracked=true` can be classified as unused: first use either wins and fences fresh old-leaf issuance, or
replacement wins and authentication refuses that unused revoked leaf. Legacy unknown successors are never revoked
by replacement; their first presentation takes the same lock, marks them used, and fences older leaves thereafter.
