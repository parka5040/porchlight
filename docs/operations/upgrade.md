# Upgrade and rollback

## Required first hop from 0.1.6

The owner must upgrade **0.1.6 → the next release** from the laptop using a locally verified, signed
`dist/<new-version>/` and the current trusted controller:

```bash
scripts/deploy-server.sh upgrade <new-version>
```

This invokes the **new release installer**. Do not use the 0.1.6 `porchlight-upgrade` wrapper or its
`upgrade --release`: that frozen bootstrap extracts an unsigned archive without expansion limits **before** its
verified handoff. The new extractor cannot retroactively protect that first hop. Also never run an old
installer's `upgrade --source`: 0.1.6 keeps its old, unfenced rollback implementation even with new artifacts.
These shipped scripts cannot be repaired by documentation or by replacing only the API image.

## 0.1.7 → 0.1.8 durable-fence hop

The installed 0.1.7 network wrapper verifies/downloads the new signed distribution, then **execs the verified
new installer before quiescing services or creating its old runtime fence**. The 0.1.8 installer owns the entire
mutation window: fsync durable fence → legacy runtime fence → stop old API/backup → gate agent SQL → old-image
backup/dump → validate/stage configuration and credential files → switch → superuser bootstrap/migrate → new API
health → fsync committed/forward-only phase → restore agent LOGIN → remove both fences. Old-image rollback still sees the runtime fence. A reboot in the new-API health window
loses only the runtime fence; new Compose/API continue to see `/data/maintenance` and refuse writes. Before the
handoff, the old API is serving the old database and no upgrade mutations have begun. Never select an old
installer's `upgrade --source`; use the selected release's independently verified installer for local sources.
An existing missing `LAN_BIND` produces a notice and preserves wildcard exposure until explicitly configured;
see [hardening acceptance](deploy-hardening.md) for that command and controller-only container proofs.

## Subsequent upgrades

Once the hardened release after 0.1.6 is installed, `porchlight-upgrade` is safe to use for subsequent signed
updates. On the server, as the account that owns the stack, run the installed wrapper:

```bash
<data-mount>/porchlight/porchlight-upgrade          # latest signed release
<data-mount>/porchlight/porchlight-upgrade 0.1.7    # explicit version
```

The wrapper remembers the root/run-dir/project settings. The stack `.env` must contain
`PORCHLIGHT_RELEASE_REPO=owner/repo`; install/upgrade defaults it from signed `release.json` only when absent.
An explicit admin value wins; an empty value disables update checks and makes the command request configuration.
For a one-off repository selection, use the trusted installed installer directly with
`upgrade --release <V|latest> --repo <owner>/<repo>` and the same root/run-dir/project options.
Never paste the private `.env` into logs.

The **installed** installer downloads `manifest.json` and its signature over HTTPS from GitHub Releases, with
connection/read timeouts and size caps (1 MiB each; 2 GiB for the dist tar). Redirects are limited to GitHub and
`*.githubusercontent.com`. It verifies minisign with its embedded key **before** reading the version, refuses
lower versions, and exits without changes for the running version (`PASS already at <V>`). Explicit requests
must match the signed version. It fetches `porchlight-<V>-dist.tar` from that version's tag. Before extraction,
every member's name, type and size must match the verified inventory (directories must be inventory ancestors).
Member count and total expanded bytes are bounded by that inventory, including the manifest/signature;
raw extended headers are limited to 16 KiB each and two per member. Only then does it extract,
require the extracted manifest to be byte-identical, verify the entire inventory, and hand off to the
verified new installer. That installer verifies again before mutation. No unverified `lib/` code runs.

Downloads survive the handoff in private `<root>/incoming/install.*/`. Successful upgrades remove that directory;
failures retain it and print its path for inspection. The installer writes `<root>/porchlight-upgrade` atomically
(mode 0750), along with the release public key and upgrade command in the stack `.env`. An exclusive
`<root>/.installer.lock` serializes server install, upgrade, restore, backup and configuration changes before
installed state is read, including the verified-installer handoff and rollback. A competing command exits without
contacting services. Never remove this lock file to bypass a running installer. Keep previous signed releases
and a cold copy of the recovery identity on the laptop.

Local and SSH sources still work, with the same version-order rules, but always start the independently trusted
**new release installer**, never the old installed copy. Verify its signed distribution first as described in
[install](install.md). For an isolated laptop stack (production's 0.1.6 first hop uses the controller above):

```bash
bash dist/0.1.7/install.sh upgrade --source dist/0.1.7 \
  --root "$PWD/var/install-test" --run-dir "$PWD/var/install-test/run" --project porchlight-install-test
```

The hardened installer refuses `upgrade --source` if its own code differs from the verified source installer;
run the selected release's verified installer instead. This check does not exist in the frozen 0.1.6 bootstrap.

## Release status and device relay

During one server process lifetime, release discovery retains the highest authenticated version seen. A signed
older **or equal** manifest reports `last_error: "release-regressed"` without lowering `latest` or clearing
`update_available`; a strictly newer verified release clears that error. This high-water mark is **not persisted
across restarts**. Manifests have no signed expiry, so this does not prevent a freeze before a newer version has
been observed or across restarts. Independently confirm expected security updates; do not treat a signature as
proof of freshness. The installed upgrader's equal-running-version no-op remains unchanged.

The read-only, operator-owned release relay validates ranges before hashing, caches verified checksums against
path/length/mtime/inode (also device and ctime), and re-verifies changes. It admits at most two hashing jobs globally
and two download bodies per device; excess returns 429 `rate-limited` with `Retry-After`. Disconnect releases body
quotas, but an already-running hash retains its admission until it finishes. Do not modify mounted artifacts in
place; stage and switch verified releases with the installer.

## Automatic rollback

Order: verify → create maintenance fence → stop API/backup writers → commit agent NOLOGIN and terminate its existing
PostgreSQL sessions → encrypted backup using the **old** image/environment → pin that
bundle in `backups/rollback/` outside nightly retention → plain `pg_dump -Fc` as postgres → load images → atomic
release switch → update version/settings → Postgres healthy → one-shot migrate → API healthcheck → scheduler.
Migrate alone sees the migrator credential. Health requires DB/schema and an HTTP response from the actual
serving process over `PORCHLIGHT_AGENT_SOCKET`, not merely a responsive database. SQLx rejects changed applied
migrations. Ordinary top-level encrypted bundles retain the 14-daily/8-weekly policy.

The temporary plain dump is `<root>/rollback/pre-upgrade-<old>.dump` (0600, directory 0700), **outside**
`backups/`: neither the API's read-only backup mount nor nightly pruning sees it. It contains sensitive database
contents; protect the stack disk. The owner's private age identity is not on the server, so encrypted bundles
cannot provide unattended rollback.

Before the new API passes health, an image load, switch, migration, startup or health failure stops API/backup,
restores the old release pointer/configuration and, if migration started, drops/rebootstraps the database and
restores the plain dump. The installer then health-checks the old API, restarts its scheduler, prints
`ROLLED BACK to <old>` with the private log directory, and exits 1. A failure before migration does not restore DB.

The new API remains write-fenced throughout Compose health polling and the explicit healthcheck. All mutations
on LAN, Funnel, enrollment and the agent socket return `503 maintenance`, `Retry-After: 1`, before authentication
or idempotency claims. Reads and healthchecks continue, except household export (which consumes a single-use
step-up and takes durable blob leases): that GET also returns `503 maintenance` before either write. Both clients,
including Account actions, show “Server is updating — try again shortly”; Account Retry preserves the action and request keys.

Read-side effects audited across the registered GET handlers and their shared extractors:
- Device timezone headers are deferred while fenced, including authority claims, member version changes and
  resulting derivation/outbox work. The read still succeeds; the next unfenced report retries the hint.
- Session sliding in `SessionAuth` and SSE heartbeats, and PAT last-used timestamps, may continue. They only
  record activity/extend existing sessions, not credentials, revocations, permissions or household data. Losing
  that bookkeeping on rollback may shorten a session or undercount usage; reauthentication/another read repairs it.
- Household export is deferred as above, rather than rolling back consumed step-ups or live blob pins.
- Other GETs (items/occurrences, board/agenda/snapshot/search, members/settings, lists/comments/attachments,
  feeds/planning, notifications/preferences, devices/endpoints/me/connection, backups, releases and admin reads)
  only read authoritative data or projections. Notification receipts/read marks and feed refreshes are mutations,
  not GET side effects. SSE replay/connection cursors are process-local; no durable read receipt is written.
  Background projection/notification work is not an externally acknowledged household edit and can be recomputed.

The durable fence is `<root>/data/maintenance`, fsynced with its parent before any new API starts; production
Compose sets `PORCHLIGHT_MAINTENANCE_FILE=/data/maintenance`. Installer also writes `<run-dir>/maintenance`
for old APIs during upgrade/rollback. Direct server configuration without that environment setting retains the
legacy default beside the agent socket. Reboot can remove `/run`, never the durable fence. Both fences are
removed only after successful completion/rollback (or handled forward recovery after rollback is disarmed).
The API caches filesystem status for 250 ms; allow that delay after manual fence changes. A manual fence
does not drain in-flight writes; the installer stops the old API before taking its dump. Independently, the
installer sets `porchlight_agent NOLOGIN` and waits for termination of existing agent backends before either
snapshot, preventing raw SQL commits from being acknowledged and then discarded by dump rollback. The role
stays gated throughout migration and rollback (database bootstrap preserves existing role login attributes).
After commit or successful rollback, cleanup restores `LOGIN` only after a durable committed decision. Failed
rollback retains `NOLOGIN` and the HTTP fence; keep external SQL clients stopped until recovery is complete.
Superuser/migrator credentials must remain operator-only: the agent gate does not fence those privileged writers.

**Point of no return:** after health succeeds, the installer atomically replaces the durable fence with its
`committed` phase and fsyncs the file and directory **before** restoring agent LOGIN or removing either fence.
Recovery consults that phase even if a signal interrupted the shell assignment. The new release/database are
then forward-only. The marker remains through agent restoration and is finally unlinked with directory fsync;
a crash around unlink may leave either a committed marker or no marker, never a rollback-authorizing pending
marker. The plain dump is then removed. Backup/sidecar startup failures or handled signals thereafter keep the new release/API and
its database, exit nonzero, and print `FORWARD RECOVERY` with private logs and a shell-quoted recovery command.
Fix the auxiliary service error and run that command (`up --no-deps` for backup/sidecar); do not restore the
pre-upgrade snapshot over new writes. A same-version upgrade is not an auxiliary-service repair. Success prints
`PASS upgrade <V>`; the pinned encrypted bundle stays until the operator removes it after validation.

Limits: rollback is database restoration, not down-migration; it does not undo external effects or replace
PKI/blob files. Power loss, SIGKILL, disk failure or failed restore/start cannot guarantee recovery. A failed
rollback retains its dump, incoming files and logs. Inspect them privately and confirm recovery before removing
the plain dump to retry (the installer refuses to overwrite it). Never blindly start the new API against a
partially migrated database. Post-exposure recovery must preserve the new database.

### Stale maintenance fence

SIGKILL/power loss can leave `<root>/data/maintenance` and/or `<run-dir>/maintenance`. The durable file survives
reboot. Both `pending` and `committed` keep the API write-fenced on startup; unknown/unreadable files fail closed.
The installer reads and reports the phase, then refuses further service changes. It never performs an unattended
restore or clears a stale marker. Hold the per-stack installer lock during manual recovery, stop external SQL
clients and inspect only privately: the active signed release, configuration, logs and recovery dump.

The exact 0600 UTF-8 phase file format is one of these single lines (including the final newline):

```text
porchlight-upgrade-v1 pending
porchlight-upgrade-v1 committed
```

- **pending:** agent LOGIN has not been restored and HTTP remains fenced. Keep the agent NOLOGIN and stop
  API/backup. Determine from private logs whether migration began. If it did, restore the complete pre-upgrade
  dump using the old signed release/configuration and bootstrap; otherwise restore the old release/configuration
  without replacing the database. Alternatively complete the new migration deliberately while still fenced.
  Health-check the selected release/database, durably replace the marker with `committed`, then resume as below.
- **committed (forward-only): NEVER restore the pre-upgrade dump.** SQL writes may already have been acknowledged,
  even while HTTP remains fenced. Preserve the selected release and database; repair/start them and health-check
  without database replacement. This also applies to a committed successful rollback: the selected release is then
  the old release, and its resumed database must not be reset again. Restore agent LOGIN using a private operator
  session (`ALTER ROLE porchlight_agent LOGIN`), remove the runtime fence, unlink the committed durable marker and
  fsync its parent directory. Resume auxiliary services. Retire the plain dump only after verifying recovery.
- **absent:** new writes may have been acknowledged; recovery is forward-only even if the dump or runtime marker
  remains. Never infer permission to restore from a leftover dump. Resolve any runtime marker only after health.
- **unknown/unreadable or legacy `upgrade pending`:** keep both HTTP and SQL writers disabled. The old format did
  not durably record exposure; recover forward unless independent evidence proves that no writes were exposed.

For a manual pending-to-committed transition use the newest verified installer's trusted operations helper
(`durable-fence <root> commit`); its atomic replacement fsyncs the file and parent. Its `remove` action fsyncs the
parent after unlink. Do not use an ordinary text editor or an unsynced `rm` as the durability protocol. Never
print credentials or connection strings; do not retry another installer until the stale recovery state is resolved.

## Ordering-key maintenance (migration 0067)

Pause external agent SQL jobs for the upgrade to avoid failed requests; the installer enforces their quiescence
with NOLOGIN and session termination throughout the backup/rollback window. Migration 0067 briefly blocks writes to `item_occurrences` and `list_entries` with
`SHARE ROW EXCLUSIVE` table locks acquired **before** reading ordering keys. A local 5-second lock timeout fails
fast with `Canonical ordering migration could not lock ordering tables within 5 seconds; pause agent SQL writers
and retry upgrade` (SQLSTATE 55P03), surfaced in the installer's migration failure log. Release the writer and
follow the rollback/retry procedure above; do not override an active writer or skip the migration.

Repair is one household-scale transaction, not independently committed batches. Only collections with
noncanonical keys are respread (normally none on a 0.1.6 database), retaining C-collated position/id order,
including archived rows and ties. Already-canonical collections retain keys, versions and audit history. All
repair writes and NOT VALID constraint additions commit together; any error before that commit rolls them all
back. Constraint validation follows in a separate transaction, releasing the short constraint-add exclusive
locks before scans. A retry after that first commit skips completed repair and safely repeats validation.

## Manual encrypted-bundle fallback

`ROLLBACK FAILED` includes encrypted-bundle restore guidance. Use the **newest verified installer**, even for an
old bundle, with `--source` selecting the **bundle's matching signed artifacts**. Tool version and restored server
version are separate: the new tool supplies target-local UID/GID, locking and validation; old images/migrations
match the old dump. Keep the private identity in owner custody; prefer an independently verified laptop restore
drill before relying on any bundle. For a 0.1.6 bundle with a verified 0.1.7 recovery tool:

```bash
bash dist/0.1.7/install.sh restore "$PWD/var/install-test/backups/rollback/porchlight-<timestamp>.tar.age" \
  --source dist/0.1.6 --identity "$PWD/var/backup-recovery.key" --force --no-funnel \
  --root "$PWD/var/install-test" --run-dir "$PWD/var/install-test/run" --project porchlight-install-test
```

Restore requires the exact bundle version and replaces DB, PKI, secrets, referenced blobs and `.env`, resetting
DB defaults/role passwords through bootstrap. `--force` is required for any existing stack, even stopped.
Displaced `.env` and `data/` are retained privately as `.env.before-restore.<utc>` and `data.before-restore.<utc>`;
remove them only after validating recovery, according to your storage policy. Host age reads the identity only
for decryption; it is never mounted into a container. Restore remains LAN-only until Funnel is explicitly
re-enabled. Do not copy the laptop identity onto the server casually.
