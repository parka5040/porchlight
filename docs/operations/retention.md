# Retention and maintenance

The scheduler uses the household IANA zone, persisted catch-up state and an injected clock in tests. Domain items,
occurrences, revisions and events are never age-purged. Explicit super-admin `porch purge` retains its guarded FK
closure, counts/audit and refusal on an unhandled dependent; unknown relations must never silently cascade.

- **03:45 household-local nightly**: notifications/events strictly older than90days, expired sessions/enrollment
  codes/password resets/PATs/step-ups (`expires_at <= now`). Explicit child counts and per-table purge_log audit
  commit atomically. Expired PAT attribution links on comments/attachments erase to NULL, content remains.
- **Hourly/startup**: idempotency response keys expire after24h (not duplicated by the nightly job).
- **Outbox**: published seven-day prefix only, before the first recent published sequence; keep the maximum if all
  are old. Unpublished rows never prune. Clients held across a prune receive terminal retention_pruned resnapshot,
  so reconnect/resnapshot rather than retrying an obsolete cursor. See stream.md.
- **Blob GC**: unreferenced30days; live referenced or leased main/thumbnail hashes never GC. A backup lease commits
  before snapshot/dump, renews on observed progress and releases after publication/failure. Crash expiry6h.
- **Server bundle files**: newest per day14days + newest per ISO week8weeks (union, not22 distinct bundles);
  prune only after persisted success, never unknown files or pinned backups/rollback/. Pinned rollback files need
  deliberate operator cleanup after validation/cold copying.
- **Desktop**: newestN=8 verified encrypted bundles per household, no prune on failed download/checksum; separate
  policy from server retention. Details in passive-pull.md.

Inspect `porch backups` (admin PAT) or the desktop super-admin Account backup panel. Server failures return only a
controlled diagnostics hint. Last successful backup strictly older than26h emits deduped backup_stale notifications
for live super admins; no duplicate episode until a new success. Before first success, age is measured from household
creation and next_due may be null. Do not change retention audit/history directly with app credentials. Inspect
private worker/backup logs under var and scheduler retry state; neither rerunning a failed pull nor resnapshotting
requires a domain mutation.

Exact tables, lock ordering, grant boundaries and clock predicates: [data-out](../specs/data-out.md),
[lock-order](../specs/lock-order.md), [backup/restore](backup-restore.md). Never run a manual purge without a recent
verified independent backup and a successful isolated restore drill.
