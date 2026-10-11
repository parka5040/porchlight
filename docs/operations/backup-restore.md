# Backup, independent restore and drill

Production backup runs at **03:00 in the household's IANA zone** (households.tzid), with catch-up when no successful
run exists or the last success is older than 24 hours. `backup_runs` persists started/finished/status, filename,
size/hash/error and `next_due_at`; normal polls are 60 s, with at least one hour backoff after a failure. A shared filesystem lock serializes
nightly and immediate jobs. The dedicated LOGIN role is a member of `pg_read_all_data`, with INSERT/UPDATE of
backup_runs; app and agent can SELECT status but cannot modify it. Never give the API backup/migrator credentials.

An independent `.tar.age` contains: JSON format/version/time/migration/count/hash manifest, `pg_dump -Fc`, PKI,
secrets, public backup recipient, stack `.env`, and all referenced blob **bytes**, not just an index. Dump and counts
share an exported REPEATABLE READ snapshot. This includes the append-only `security_events` table (0070):
its one-year running-server retention does not delete older history from existing encrypted bundles. Catalog-based
count comparisons cover it on restore. Admin household exports also include its JSONL/completion inventory;
member exports exclude it. See [security history](../specs/security-events.md). PKI CA fingerprint and cert/key pairs are checked before publication.
Plaintext staging is **never** in `backups/`: a 0700 `porchlight-backup/work-*` directory below `TMPDIR`
(Compose mounts `/work` as tmpfs with an explicit **512 MiB cap**) holds only PKI, small secrets, `.env`, recipient
and manifest. Attachment bytes are **never copied to staging**: safe regular files from the read-only `/data/blobs`
mount are hashed in bounded chunks, then streamed directly into tar → age. The unbounded custom-format dump is
always staged **on disk**, below `PORCHLIGHT_DUMP_TMPDIR=/dumpwork`, bound from the private 0700
`<stack-root>/backup-work/` (`var/backup-work/` in dev). Install/upgrade/dev helpers provision this bind as the
backup UID; manual Compose users must do likewise. Allow disk space for one complete dump; exclude this private
work directory from cold copies and snapshots. No plaintext tar file is written. Only encrypted `.partial-*` output
goes into `backups/`, then is fsynced and renamed atomically.
SIGTERM/SIGINT stops child process groups, removes both staging trees/partial output and marks an unfinished run
failed. An already committed success is never downgraded by pruning failure or interruption. SIGKILL cannot run
cleanup: at startup, under the same filesystem lock, stale `running` rows become `failed`; both work roots and
encrypted partials are swept (including legacy `.work-*` plaintext from old releases). Tmpfs disappears when the
container is removed/recreated; the disk work root is swept on restart. Private work is not a cold-storage payload.
Heartbeat updates reflect main-loop idle polls and actual snapshot/dump-file growth/hash/tar-read progress, not an
unconditional timer thread. Streaming/idle work stalled for 180 seconds fails the health probe; healthy large dumps
continue to pulse as bytes grow. Pre-dump SQL and small private copies/certificate checks instead publish a fixed
600-second phase deadline: they may legitimately lack byte progress for longer than 180 seconds. Nested work cannot
extend that deadline, and no timer fakes progress; expiry fails health even if the process is still alive. Subprocess
commands also time out at 600 seconds. Compose checks every 60 s with three retries; health does not automatically kill/restart a hung
container: inspect private diagnostics and stop/restart it to clean up/recover. Idle SIGTERM exits zero without FAIL
noise. Retention runs only
after a persisted success: newest per day for 14 days + newest per ISO week for 8 weeks (union; overlapping bundles
count once). Unknown files are never deleted. Upgrade recovery points are pinned in `backups/rollback/`, outside
normal retention, and need explicit operator cleanup after validation/cold copying. No private recovery identity is
present on the server.

```bash
bash dist/0.1.0/install.sh backup --root "$PWD/var/install-test" \
  --run-dir "$PWD/var/install-test/run" --project porchlight-install-test
# Supply a specific bundle; default otherwise is newest under var/backups (dev stack).
deploy/scripts/restore-drill.sh var/install-test/backups/porchlight-<timestamp>.tar.age --source dist/0.1.0 --installer dist/RECOVERY_VERSION/install.sh
# --keep leaves private var/restore-drill and the scratch stack for inspection.
```

The drill decrypts under `var/restore-drill`, checks all hashes and archive paths, restores to `porchlight-drill` with
its own bind directories/networks and loopback spare ports (55432/18443/18444), reruns migrations and compares all
table counts + latest migration version + CA fingerprint **before starting the API** (`restore --no-start`).
Only after those comparisons and DB-authoritative blob checks does the drill start the API. `porchlight healthcheck`
requires a migrated DB and a bounded HTTP response from the serving process over its agent socket (four seconds total).
The drill additionally verifies a real enrollment-listener TLS/application response within five seconds: CA validation
and the exact restored server leaf pin authenticate its loopback connection without requiring the original DNS name.
No PAT/device secret or new unauthenticated endpoint is used. Both API writers and the backup daemon
are disabled during snapshot comparisons. It prints only PASS/FAIL lines. Without `--keep`, the scratch stack/decrypted state
are torn down. Remove an existing drill directory explicitly only after inspecting it; a new drill refuses to reuse it.
DRILL_PG_PORT/DRILL_LAN_PORT/DRILL_ENROLL_PORT and DRILL_BACKEND_SUBNET/DRILL_INGRESS_SUBNET override defaults.

For both drills and real recovery, select the **newest verified installer**, not the old bundle's installer.
Use `--installer dist/<recovery-version>/install.sh` for the drill (required); the operator must establish trust in
that tool first, as described in [install](install.md). For real recovery, run that generated installer's
`restore <bundle> --source <matching-release> --identity <laptop-identity>`;
repeat root/run/project settings and use `--force` only after consciously accepting replacement of any existing stack, including one that is stopped. `--no-start` leaves the restored/migrated DB
ready for offline inspection without starting API/backup writers. Displaced `.env` is kept privately as
`.env.before-restore.<utc>`. Shared bootstrap runs before pg_restore so roles and database defaults exist for GRANTs.
Manifest verification occurs before any stop/data replacement. The archive extractor refuses absolute/parent paths,
links, devices and duplicate members; expansion is bounded to 1 TiB/1,000,000 members with bounded tar headers.
Exceeding those restore limits requires an owner-reviewed trusted-helper change, never unbounded extraction.

Runtime UID/GID are **target-local**, not restored from the source `.env`. By default restore persists the invoking
account's UID and primary GID; `PORCHLIGHT_UID`/`PORCHLIGHT_GID` can explicitly select that UID and a group the
account belongs to. Restore refuses an incompatible override or inaccessible/non-owner private target directories
**before** stopping/replacing a stack. Run as the intended target owner and repair directory ownership first; do
not broaden 0700 data/backup/work directories or the 0600 environment/secrets. Cross-host restores therefore do
not retain an unusable source UID. A restored cluster may receive deployment-specific port/network overrides
through the operator's environment: `PG_PORT`, `LAN_PORT`, `ENROLL_PORT`, `LAN_BIND`, `ENROLL_HOST`,
`ENROLL_CIDRS`, `PORCHLIGHT_ENDPOINTS`, `BACKEND_SUBNET`, `INGRESS_SUBNET`. Explicit values are validated before
replacement and persisted before the first target Compose call, including `--no-start`. Choose nonoverlapping
networks and unused ports; the installer cannot detect other hosts' allocations. Secrets remain from the bundle. See [upgrade](upgrade.md) for rollback and
the per-stack installer lock, also held throughout restore.

## Pulled bundle integrity and data-export distinction

A desktop pull is already verified before atomic publication, but a later cold copy must be verified again.
Compare the encrypted bundle's full SHA-256 and size against its matching desktop JSON receipt (or another trusted
expected digest) **before decrypting/restoring**. Keep the identity separate; a receipt is not a signature.
Do not pass `~/.local/share` paths directly to `age`/the installer on this laptop: the Docker wrapper mounts only
the repository working directory. From that root, stage ciphertext + receipt into a private `var/` directory and
re-verify the copied size/SHA before decryption:
```bash
umask 077
. scripts/env.sh
staged=$(python3 -I deploy/scripts/stage-pulled-bundle.py "$bundle" "$receipt") &&
deploy/scripts/restore-drill.sh "$staged" --source dist/VERSION --installer dist/RECOVERY_VERSION/install.sh --identity var/backup-recovery.key --keep &&
# After a successful drill, restore the SAME verified var copy with explicit target root/run/project options:
bash dist/RECOVERY_VERSION/install.sh restore "$staged" --source dist/VERSION --identity var/backup-recovery.key \
  --root "$PWD/var/restore-target" --run-dir "$PWD/var/restore-target/run" --project porchlight-restore-target --no-funnel
```
Use the newest generated verified release installer above, never the `deploy/install.sh` template (which refuses
direct execution). VERSION selects the bundle artifacts; RECOVERY_VERSION selects the newest trusted tool.
Replace both and target root/run/project deliberately. The target root must be under the repository
on this laptop because the `age` Docker wrapper cannot access an external target during decryption.
The drill also accepts `--receipt PATH` to stage an external pulled bundle itself, and rejects external bundle
paths without a receipt. Keep originals unchanged and the recovery identity separate. Inspect/remove private
`var/pulled-restore.*` staging deliberately after the drill/restore. See [passive pull](passive-pull.md).

Household `.tar.gz` exports are data-only, not independent restore bundles. Before any consumer applies one,
require formatv2 `data.json`, tables/blobs inventory matching the LAST `complete.json`, **two zero tar end blocks
and a valid gzip CRC/trailer**. Missing/corrupt tar termination, missing completion marker or missing/bad gzip
trailer means incomplete even with HTTP200. Run
`python3 -I deploy/lib/verify-export.py var/household.tar.gz`; this verifies without extracting/applying. There is
no generic export-to-database restore tool. Use the encrypted independent backup for disaster recovery.

## Existing dev stack

```bash
scripts/dev-backup-role.sh       # appends the dev secret; updates ONLY the backup login on an existing cluster
scripts/dev-stack.sh up          # builds and migrates 0050
mkdir -m 700 -p var/backups
# Source dev settings for Compose through the existing helper (do not print them).
( . scripts/dev-db.sh; dc --profile backups build backup; dc --profile backups run --rm --no-deps backup now )
deploy/scripts/restore-drill.sh --source dist/0.1.0 --installer dist/RECOVERY_VERSION/install.sh
```

The dev service is opt-in under `backups`; production starts the daemon automatically. The final projection is an
explicit allowlist: live `attachments.blob_sha256` and `thumbnail_sha256`, joined to `blobs` under a committed GC
lease acquired before the dump snapshot. Unleased or missing referenced metadata fails closed. Files use only
`aa/bb/<sha256>` paths. Both the prehash and the exact tar-streamed bytes must match the snapshot's authoritative
SHA-256 **and size** before publication; a corrupt file fails the run without pruning older bundles. The independent
drill enumerates live main/thumbnail references from the restored DB and rehashes their safe regular files, checking
existence and size independently of the bundle manifest. New reference tables must extend this allowlist deliberately.

Cold storage: copy encrypted bundles, e.g. `rsync -a --no-links --exclude='.*' --ignore-existing var/install-test/backups/ /mounted/cold/porchlight/`.
Every bundle already includes its referenced bytes, so no separate blob-index restore dependency exists. Keep recovery
identity and release keys in a separate personal backup, never alongside server storage. Verify periodic restores from
the cold copy. Loss of the identity is permanent loss of access to encrypted bundles.
