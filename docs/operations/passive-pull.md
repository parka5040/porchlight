# Desktop passive backup pull (O2b)

Laptop only until M9. An enrolled, signed-in **super-admin** desktop automatically pulls the newest independent
age-encrypted bundle into `~/.local/share/porchlight/backups/<hashed-household>/`. This location deliberately stays
under the specified HOME even if XDG_DATA_HOME points elsewhere. No recovery identity is needed to copy it.
The app must be running, its device key available (for a Secret Service device: wallet unlocked) and session Ready; ordinary members/dependents never request backup
status/download or mutate this storage. `/v1/me` reauthorizes each poll; the server also enforces super-admin mTLS.

The first eligible poll catches up. After success the next due is **04:00 desktop local time** (DST resolved by
java.time); a persisted `.next-pull` survives restart. Reachability/authorization checks are every minute; failed
backup attempts back off exponentially from 60 seconds, with 0.8–1.2 jitter and a one-hour cap, reset on success.
Retry state is process-local; encrypted checkpoints survive restart. Manual **Settings → Account → Backups →
Refresh and pull newest** bypasses the due/backoff checks. Installer `--minimized` launches hidden to the tray
(Open/Quit menu), or visible if the platform has no tray. Unknown benign launch flags do not terminate the app. The status panel is
DESIGN-PENDING: last successful server run, next due, controlled error indication, current-session local bundle and
pull errors. Restart before the next nightly run can show “not loaded”; refresh to fetch current server status.
The server backup is separately scheduled at 03:00 household local time, with its own catch-up/backoff policy.

## Integrity and checkpoints

- Household-partitioned 0700 directories; 0600 encrypted partials/metadata; same-household process lock prevents
  concurrent writers. Directories, lock, schedule and ownership index reject symlinks; matching legacy symlinks are
  moved aside without following them. A durable `.owned-files` index records file identity (file key and creation
  time); only recorded, still-matching files are pruned or resumed. Desktop storage must provide stable file keys
  and birth time (tested on Linux ext4); filesystems whose creation time falls back to changing mtime are unsupported.
- Ownership uses two phases: persist intent before CREATE_NEW checkpoint/private temp creation; keep the old
  destination identity plus the pending source identity until atomic rename and directory fsync succeed. Index
  updates use private temp + file fsync + atomic rename + directory fsync. Every authorized, locked poll (even if
  not due) first resolves recorded intents: a checkpoint created under a recorded intent is adopted; a completed
  move (the target carries the recorded identity) is taken over; a move that did not happen keeps the old
  destination and checkpoint; the recorded private temp is deleted by its exact recorded random name (once its
  identity is recorded, only while it still matches, so an operator-replaced temp is kept). Vanished entries are
  dropped. The index is rewritten only when an intent was pending, so idle minute polls write nothing. Legacy
  handling (below) runs only after every intent is resolved. Unrecorded temporary files are never glob-deleted.
  A failed operation leaves its intent for the next poll/restart to reconcile.
- **Failures name the path and one action** (Settings → Account → Backups error):
  - Storage problems while recovering or writing — `Backup storage problem at <path> (disk full | permission
    denied | read-only file system); free space on the disk holding <dir> | make <dir> and its files readable and
    writable by your user | make the disk holding <dir> writable, then refresh`. Nothing is moved, deleted or
    forgotten: `.owned-files` and its pending intents stay exactly as they were, and legacy renaming is skipped, so
    the app's own checkpoint is never renamed aside. Once the storage is fixed the next backoff retry (even when no
    pull is due) finishes the recovery and clears the error; Refresh retries at once. **Do not** move or delete
    `.owned-files` for these errors. Unclassified errors say `check permissions and free space on <dir>, then refresh`.
  - Corrupt index (unparseable, invalid UTF-8, unsupported version, unmanaged names, or not a regular file) —
    `Backup ownership index <path> is corrupt; quit Porchlight, rename it aside within <dir> (do not delete it), then
    refresh`. Nothing else in the directory is touched while it is corrupt. After the rename, verified bundle +
    receipt pairs are re-adopted without downloading and anything unverifiable (e.g. a checkpoint) is preserved
    aside with a notice, as for R82 legacy files below. An index that cannot be *read* (e.g. permission denied) is a
    storage problem, not corruption.
- Checkpoint = encrypted `.part` length (synced every 4 MiB and at stream completion); its filename includes a
  hash of run UUID + the server SHA-256. A crash may lose recent bytes; final full SHA guards integrity. Restart uses
  `Range: bytes=OFFSET-` and quoted digest `If-Range` on the existing root-only mTLS transport.
- Validate response ETag, content length and exact Content-Range before writing. A valid full200 truncates/restarts;
  a changed latest identity discards old partials. No redirects, ordinary system-root HTTP, or ambiguous-body retries.
- Verify final length and **full local SHA-256** before atomic `.tar.age` publication and directory fsync. Mismatch
  keeps the encrypted checkpoint and leaves older backups intact. Incomplete checkpoints resume by range; a full
  invalid checkpoint needs a validated 200 replacement (bad bytes cannot be repaired by appending).
  Session/identity/trust changes fence every chunk and publication.
- Keep the newest **N=8** verified bundles per household, by server finished_at/run UUID; retention runs only after
  verified publication. Constructors accept N=1..100 for tests/embedding; there is no configurable UI setting yet.
  The local policy is distinct from the server 14daily/8weekly union. Do not manually modify `*.json` receipts or
  `.owned-files`. Only recorded, still-owned orphan bundles without a receipt are pruned after a later successful
  publication. Unknown names remain untouched. **R82 legacy migration:** exact managed names
  (`<64 lowercase hex run-id hash>-<64 lowercase hex SHA-256>.tar.age`, matching `.json`, or `.part`) without a
  matching index identity are reconciled on authorized polls. A regular bundle + matching, parseable receipt are
  adopted only after basename/run-id, positive size and full SHA-256 verification; adopted bundles join retention.
  This also repairs a v1 receipt identity recorded before a rename that never happened, when its surviving pair verifies.
  Unverified matches (including legacy checkpoints, orphan ciphertext, replaced files or links) are **atomically
  renamed within the same directory**, never deleted, to `<name>.legacy-<UTC timestamp>-<unique suffix>`.
  Each rename produces one Settings → Account → Backups notice: `Preserved unverified backup file <name> as <full path>`.
  The notice survives that poll's success or failure and non-due polls; it clears on the next actual pull attempt
  (or session reset). An already renamed file is not reported again. Inspect the named path under `~/.local/share/porchlight/backups/<hashed-household>/`
  and move it to cold storage if wanted; renamed-aside files are excluded from retention and remain until you manage them.
  Pull proceeds without resuming or overwriting foreign bytes. Remaining failures name the affected path and an action
  (see the failure classes above, or quit competing writers and refresh); they do not give only a generic retry error.

If copying to cold storage, copy the `.tar.age` **and its matching JSON receipt**, and recompute its `sha256` and
`size` before trusting the copy. The receipt is a transfer-integrity aid, not a signature. Do not copy `.part` files
or mistake them for restorable bundles. Keep the recovery private key separately in the owner's personal backup.

## Restore from a pulled bundle

1. Keep the original encrypted file and its JSON receipt unchanged. Compare `sha256sum FILE` and `stat -c %s FILE`
   with receipt `sha256` and `size`; if no trusted receipt exists, obtain the expected digest through a trusted channel.
   Do not bypass a mismatch. Desktop SHA verification does not replace verification after a later cold copy.
2. **Stage and re-verify a cold copy under this repository's private `var/` before any decryption.** The laptop's
   `age` Docker wrapper mounts only the repository working directory; it cannot see `~/.local/share` directly.
   From the repository root (never change into an untrusted download directory):
   ```bash
   umask 077
   . scripts/env.sh
   # Set bundle and receipt to the matching desktop .tar.age / .json paths; originals remain unchanged.
   # Copies ciphertext + receipt into private var/pulled-restore.*, then checks the COPY's sha256 and size.
   staged=$(python3 -I deploy/scripts/stage-pulled-bundle.py "$bundle" "$receipt") &&
   deploy/scripts/restore-drill.sh "$staged" --source dist/VERSION --installer dist/RECOVERY_VERSION/install.sh --identity var/backup-recovery.key --keep
   # A failed staging must not run a drill; an explicit empty bundle path is rejected.
   # Or the drill can stage itself: restore-drill.sh "$bundle" --receipt "$receipt" --source ... --installer ... --identity ...
   ```
   Set RECOVERY_VERSION to the newest verified installer, independently of the bundle version.
   Fetch the **matching signed release** identified by the decrypted bundle manifest; the independent
   [restore drill](backup-restore.md) verifies it before applying state. Keep the separately held recovery identity
   under private `var/` while running the laptop wrapper, then remove the temporary identity copy when done.
   A missing/mismatched receipt is a hard stop, not a reason to skip verification.
3. The bundle alone contains DB dump, PKI, secrets and referenced blob bytes. The archive verifier inside the installer
   validates safe paths, the manifest and every member hash **before** state replacement. Supply explicit scratch
   root/run/project/loopback spare ports; keep the identity on the laptop, never mount it into the live server.
4. Only after a successful drill use the **newest verified installer**, with matching bundle artifacts, not the `deploy/install.sh` template:
   ```bash
   bash dist/RECOVERY_VERSION/install.sh restore "$staged" --source dist/VERSION --identity var/backup-recovery.key \
     --root "$PWD/var/restore-target" --run-dir "$PWD/var/restore-target/run" --project porchlight-restore-target --no-funnel
   ```
   Replace VERSION (bundle artifacts), RECOVERY_VERSION (trusted tool) and target root/run/project deliberately. The target root must be under the repository on this
   laptop: the `age` wrapper cannot access an external target during decryption. `--force` deliberately replaces
   existing state, even a stopped stack. Inspect migration/count/CA comparisons and health before enabling writers.
   See upgrade/rollback runbooks.

**Household data exports are not independent backup bundles.** An export is tar.gz formatv2 data.json + JSONL tables
+ blobs + a LAST complete.json. Before any import/application, require that final marker, exact inventory/counts,
blob hashes and a valid gzip CRC/trailer (`python3 -I deploy/lib/verify-export.py var/export.tar.gz`). Missing marker,
wrong inventory or missing/bad gzip trailer means incomplete, even if HTTP returned200. No generic export-to-DB
restore/import command is supplied; use an independent encrypted backup for disaster recovery.

## C4 interface and acceptance

`AppServices.backups: DesktopBackups?` is desktop-only. `backups.state: StateFlow<BackupView>` exposes `allowed`,
`status` (generated BackupStatus), `localBundle`, `pulling`, sanitized `error`, `unreachableSince` and
`cantReachHome`. ConnectFailure starts the timestamp; any successful authorized minute check clears it, even
when the nightly pull is not due. Trust lock/identity end/401/403 clears the view. This is an **additional
backup-connectivity signal**, not proof the SSE stream is alive: C4 owns the 5minute stream banner,
aggregating/fencing by its trust epoch. The 30minute unreachable-device alert originates on the server (S6a);
the client only displays that server alert.
`pull()` is suspend and manual; do not block Compose's UI thread. DesktopAppServices owns the minute-poll job and
cancels it on close. No offline mutations or local notification are added by O2b.

### Single tray ownership (C4 merge contract)

C4 owns the tray and notifications. `desktopTrayActions(BackupView, onOpen, onQuit)` in shared JVM code is a
**data-only contribution**, with `open`/`quit` label+callback and nullable, sanitized `backupStatus` text. It creates
no icon, notification or background job. Collect the existing `services.backups.state`; the status never includes
bundle paths or raw errors, and locked/non-admin sessions expose no backup details (only a connectivity signal).
Open reveals the existing window; Quit exits the application and disposes the services.

C4's SNI/dbusmenu tray (ruling R71) is the **sole tray host**: O2b's backup status, Open and Quit actions are
registered into its `trayActions` registry, and the former Compose Tray adapter (`DesktopTray.kt`) is gone. Main
accepts `--tray` and `--minimized` (both start hidden to the tray) and keeps the no-tray visible-window fallback.
A merged desktop must show exactly one icon. No O2b notification
producer is added: C4 continues to own notification delivery and connectivity aggregation.

Controller smoke entry point in the bundled app:
`porchlight --backup-pull-smoke var/PRIVATE/devices/desktop https://localhost:PORT`. It uses porch dev-init PEMs and
the production root-only TLS factory/pull engine, exits nonzero unless a verified bundle publishes, and requires
both the resolved device path and JVM user.home under this repository's var (run from the repository root).
Sanitized FAIL reasons are captured in the gate's private pull.log; raw exception bodies are never printed.
It does not touch the normal Secret Service/PKCS12 wallet.
The complete [M8 gate](m8-acceptance.md) drives it in a scratch HOME.
