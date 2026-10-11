# Availability and planning (S8b)

## Binding rulings

- **R47 — empty availability means "load not tracked".** A member with no availability windows gets no load
  conflicts, no load notifications and no EDF suggestions (EDF needs windows); a member with windows on some weekdays
  has capacity 0 on the other weekdays (those DO conflict).
- **R48 — who may accept / whose tasks.** The EDF task set for member X is X's agenda (R2 agenda membership:
  assignees ∪ subject, else owner) — unscheduled tasks/todos with estimates and deadlines — independent of who asks.
  Accept is allowed for (a) a caller with `Edit` on the item, or (b) the suggestion's own member when that member is an
  agenda member of the occurrence at accept time (self-scheduling assigned work); accept writes only the occurrence's
  span override, never item details. On-demand runs: for yourself, or by a super admin for a dependent (spec
  "Dependent items"); a run replaces only the proposed rows it recomputed (never deletes rows for items it did not
  include). Accept preconditions (M8): the member is still an agenda member and `proposed_start` is still in the future
  → else sealed 422 `validation-failed` with a field code (M6: never the transient `conflict` slug for a business
  rejection).

### Fix round 2 rulings (refine R48 / M8)

- **N1:** dependent-candidate notifications fire only for `free_inside_availability` candidates; compare against the
  core enum (no string literals duplicated in the API — use the enum or a shared constant); the gate's last check uses
  the real wire values.
- **N2 — stale proposals:** a run (nightly or on demand) deletes every PROPOSED row of that member whose occurrence is
  no longer eligible (completed/skipped, scheduled, de-estimated, expired, regenerated or deleted) as well as the rows it
  recomputes; accepted/dismissed rows are history and are never deleted. Rows for items the run did not consider
  because of visibility rules stay (R48).
- **N3 — proposal lead time:** EDF never proposes a start earlier than the next 15-minute boundary after `now` (in the
  member's zone); accept rejects as not-future only when `proposed_start < now - 5 min`.

## HTTP / CLI

Both session and PAT routers register these operations:

| Method | Path | Policy | Version |
| --- | --- | --- | --- |
| GET/PUT | `/v1/members/{id}/availability` | self, app super admin, or agent | PUT collection If-Match; default version 0 |
| POST | `/v1/schedule/suggest` | self; super admin/agent for dependent only | idempotency only |
| GET | `/v1/suggestions` | private member collection, authoritative item View | signed cursor/limit; optional member_id |
| POST | `/v1/suggestions/{id}/accept` | item Edit OR own agenda work | **occurrence** If-Match |
| POST | `/v1/suggestions/{id}/dismiss` | self/admin + item View | **suggestion** If-Match |
| GET | `/v1/items/{id}/candidates` | item View; unassigned dependent appointment | read-only |

Business rejections use validation-failed (`not-agenda-member`, `not-future`, `not-proposed` field codes), not
`conflict`. Transient lock/scope conflicts are unsealed retryable 409s. Stale accept current is the authorized
occurrence. Suggestions carry `occurrence_version` (null for vanished occurrences). Availability windows use
Monday=0 … Sunday=6 and whole-second `start`/`end` local times; equal endpoints are empty, end<start overnight,
overlaps unioned. SQL uses `starts_local`/`ends_local`. Empty-to-empty PUT still versions/tickles the collection.

Load sums estimates of open, non-expired todo/deadline occurrences on the member-local scheduled start day,
otherwise deadline day, never both. Capacity is unioned elapsed availability minutes (DST-aware). Appointment
busy ranges reduce EDF slots, not load capacity. Warnings carry originating occurrence/member, whole local-day
UTC bounds, null counterpart IDs and "Busy". Writes succeed with conflicts; all item read/write paths use the
same projection. Pending/poisoned task projections are excluded from load recomputation.

One scope drains pending peers. EDF reads authoritative R2 membership in one batch, uses open unscheduled estimated
work with a deadline, searches 90 days, and sorts by core deadline/priority/UUID. The input does not depend on
caller visibility; only the response is View-filtered using the shared authoritative SQL VISIBLE predicate. Replacement covers all considered items, including their stale occurrences, plus deleted/missing items.
Accepted/dismissed history and excluded live-item proposals survive. EDF begins at the strictly next member-local
15-minute boundary, resolved with the shared gap/fold policy (past fold resolutions are skipped).
Listing applies shared VISIBLE before LIMIT with a `(member_id,id)` keyset; invisible rows do not shorten visible pages.

Accept sets only this occurrence's complete local span, retaining its series kind/deadline. Todo/deadline spans
also work for generic occurrence PATCH and raw SQL; C6/S4a must preserve both bounds. A DB CHECK rejects half-spans
for due-only todo/deadline anchors, and core validates the todo/deadline pair. Appointment anchors already carry
both generated bounds and keep their inherited-bound partial-override behaviour. Fixed-zone spans resolve in the item zone;
floating spans use the member zone. DST round-trip, occurrence version/status, capabilities, agenda membership,
proposal no older than five minutes, suggestion status/version and member zone are rechecked; failures roll back the domain savepoint.

Nightly at 03:00 household time uses independent system transactions on the operational pool, outside scheduler
election. It computes the same agenda tasks as on demand and returns aggregated member failures after continuing
all members, so scheduler last_error records them. Dependent ranking uses the earliest open occurrence with
`starts_at >= now`, buffered busy spans, and live user/super-admin schedules; core tier then UUID. A pending parent
or candidate-member peer yields pending_derivation=true, not a false "free" result. Poisoned peers (attempts >=5)
do not pin the response pending; their retained busy ranges remain conservative. Poisoned parents are omitted
with no ranked candidates and pending_derivation=false; operational derive status still exposes the poison error. Only free_inside_availability candidates notify;
the producer receives the real EDF run ID and still follows recipient item visibility.

## Locks, queue and integration contracts

Migrations 0045/0046 create the original six tables and raw-window member NKU NOWAIT serialization.
0047 adds retry/poison fields, member paging index, complete-span CHECK, terminal status guard and immutable
`member_load_signals`; 0048 makes availability bookkeeping per affected member **per statement** (transition tables).
The API inserts all replacement windows in one statement. Audit captures pl_current_actor; tickles use member scope.

Derivation collects old/new member-local dates without touching aggregates. Scope finish recomputes once, only for
members it actually holds, and only changed dates (a pending availability job forces a full refresh). Other peer
members get immutable, fresh-ID signal inserts (insert-if-absent, no shared queue-row upsert/member lock). Signals have no member FK (not even KEY SHARE locks are taken for unheld peers). This
avoids a scope-to-scope queue cycle even while Z's aggregate/queue rows are locked. The member-only worker folds
signals into its job while holding Z, then recomputes/conditionally acknowledges the captured generation. It never
locks items. Error categories, bounded exponential backoff and five-attempt poison state mirror derive jobs. New
authoritative inputs reset retries; deleted members' jobs are acknowledged and projections cleared. SKIP LOCKED
paths explicitly roll back. Scopes and the derive worker lock deleted agenda members too, consistently with
the member-load worker, before clearing their projections. Failures anywhere in a per-member load transaction
are category-logged (no database messages) and do not abort later members. Date-limited SQL uses uncorrelated
hashed membership sets; Rust transition filtering uses an indexed set, not a scan per old conflict.

The load-notification producer calls `pl_emit_notification_event` in ONE statement over all changed
(member,date,conflicting) values, enter **and** leave transitions, only inclusive member-local
`[today,today + 14 days]`, excluding untracked members (R47). Dedupe keys are member/date/state
(one enter and one leave event per member-local date); recomputing unchanged projections is silent.
The estimate path is measured at 28 statements (daily recurrence, three members, >1,500 load rows),
below S3's ≤30 cap, including the real producer statement.

EDF emits `suggestion` once per member/real run with nonempty proposals; the shared function suppresses
business self events. Nightly dependent candidates are emitted in ONE batch per member, only
`free_inside_availability` and readable items, deduped by member/occurrence across runs. Their payload
retains the actual EDF run UUID and item/occurrence references; no new event kind is required.
Both producers skip dependent recipients (and deleted members): dependents cannot read member-private
planning events. Their load/proposals remain available through existing authorized admin reads; no
owner/super-admin notification rerouting is specified by this contract. Existing kinds use the R53 data
registry; producers never redefine the shared emission function.

`/v1/suggestions` and batched visible IDs use S4b's VISIBLE before LIMIT; Rust item capability helpers
remain authoritative for View/Edit checks. The next run also deletes proposed rows whose item left the
member's authoritative task agenda (reassignment/subject/kind change); accepted/dismissed history remains.
Excluded live items still on that agenda are preserved.

Snapshot adds `availability` and `suggestions` arrays inside S4b's existing read-only RR transaction,
with the same published household cursor. App sessions (including super admins) get their own planning
rows; agents get the whole live household. Availability includes version-0 empty defaults; suggestions
also join VISIBLE and include accepted/dismissed history. Neither availability windows nor proposal
spans have independent searchable text, so the search extension deliberately adds no source: item titles
already participate once through S4b FTS/VISIBLE. This avoids duplicates and private-proposal disclosure.
S4b agenda and existing item reads both retain additive load warnings. Agenda fetches warnings only for
its page's occurrence IDs on the same acquired connection and indexes them by occurrence for a linear
merge. R2 item membership is one shared DB SQL fragment for EDF inputs, acceptance, proposal retirement
and candidate-peer pending checks.

Snapshot suggestions serialize the same typed `SuggestionResponse` as REST, including nullable
`occurrence_version`, and never internal columns such as `updated_at`. The additive
`conflicts_evaluated` flag is true only on successful acceptance, where fresh derivation and
ACL masking produce the warnings. Snapshot/list/dismiss/proposal responses set it false:
`conflicts: []` there means unavailable, not conflict-free. Older servers omit the flag (false). The derive worker passes the
injected domain clock to both item and availability-only load batches, including member-local "today".
Missing/deleted/foreign body `member_id` references on suggest use R38's 422 `validation-failed`, field
`member_id`, code `invalid-reference`, for every caller before target-member authorization; valid but unauthorized targets remain 403 and path resources keep 404.
Custom item permission rows cross visibility/relationship with independent self/other suggestion targets
and PAT read/write/admin scopes; other planning rows use the reduced self/other member dimension.

S6 delivers, C6 supplies clients, S8a supplies ICS/feeds/SSRF. Full dumps include all seven tables; O2
exports availability/suggestions, not operational queues/audit, and must purge signals before members.

Controller acceptance: `deploy/scripts/m7-demo-planning.sh [--keep]` starts its own Compose project/environment,
uses UTC availability windows relative to current time and future-dated HTTP fixtures and every `porch schedule` subcommand, checks load/EDF/accept/dismiss/candidates,
traps teardown and prints only PASS/FAIL. Private diagnostics/credentials live under `var/m7-demo-planning`.
No duplicate cargo-test wrapper remains; sandbox CI only syntax-checks this Docker-only gate.
