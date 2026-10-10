# Upgrade and rollback

On the server, as the account that owns the stack, run the installed wrapper:

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
must match the signed version. It fetches `porchlight-<V>-dist.tar` from that version's tag, safely extracts it,
requires the extracted manifest to be byte-identical, verifies the entire inventory, and hands off to the
verified new installer. That installer verifies again before mutation. No unverified `lib/` code runs.

Downloads survive the handoff in private `<root>/incoming/install.*/`. Successful upgrades remove that directory;
failures retain it and print its path for inspection. The installer writes `<root>/porchlight-upgrade` atomically
(mode 0750), along with the release public key and upgrade command in the stack `.env`. Do not run simultaneous
upgrades on the same stack. Keep previous signed releases and a cold copy of the recovery identity on the laptop.

Local and SSH sources still work, with the same version-order rules:

```bash
bash dist/0.1.6/install.sh upgrade --source dist/0.1.7 \
  --root "$PWD/var/install-test" --run-dir "$PWD/var/install-test/run" --project porchlight-install-test
```

## Automatic rollback

Order: verify → stop API/backup writers → encrypted backup using the **old** image/environment → pin that
bundle in `backups/rollback/` outside nightly retention → plain `pg_dump -Fc` as postgres → load images → atomic
release switch → update version/settings → Postgres healthy → one-shot migrate → API healthcheck → scheduler.
Migrate alone sees the migrator credential. Health requires DB/schema and an HTTP response from the actual
serving process over `PORCHLIGHT_AGENT_SOCKET`, not merely a responsive database. SQLx rejects changed applied
migrations. Ordinary top-level encrypted bundles retain the 14-daily/8-weekly policy.

The temporary plain dump is `<root>/rollback/pre-upgrade-<old>.dump` (0600, directory 0700), **outside**
`backups/`: neither the API's read-only backup mount nor nightly pruning sees it. It contains sensitive database
contents; protect the stack disk. The owner's private age identity is not on the server, so encrypted bundles
cannot provide unattended rollback.

If image load, switch, migrate, startup or health fails, the installer stops API/backup, restores the old release
pointer and configuration, and, if migration started, drops/rebootstraps the database and restores the plain dump.
It then health-checks the old API and restarts its scheduler, prints `ROLLED BACK to <old>` and the private log
directory, and exits 1. A failure before migration does not restore the database. Success prints `PASS upgrade <V>`
and removes the plain dump; the pinned encrypted bundle stays until the operator removes it after validation.

Limits: this is database restoration, not down-migration. It does not undo external effects or replace PKI/blob
files. Writes accepted by the new API before a later startup failure can be lost. Power loss, SIGKILL, disk failure,
or a failed restore/start cannot guarantee recovery. After a failed attempt the dump, incoming files and logs remain;
inspect them privately and confirm recovery before removing the plain dump to retry (the installer refuses to
overwrite it). Never blindly start the new API against a partially migrated database.

## Manual encrypted-bundle fallback

`ROLLBACK FAILED` includes the existing encrypted-bundle restore guidance. Use the **old version's** trusted
installer/artifacts and the pinned bundle. Keep the private identity in owner custody; prefer an independently
verified laptop restore drill before relying on any bundle:

```bash
bash dist/0.1.6/install.sh restore "$PWD/var/install-test/backups/rollback/porchlight-<timestamp>.tar.age" \
  --source dist/0.1.6 --identity "$PWD/var/backup-recovery.key" --force --no-funnel \
  --root "$PWD/var/install-test" --run-dir "$PWD/var/install-test/run" --project porchlight-install-test
```

Restore requires the exact bundle version and replaces DB, PKI, secrets, referenced blobs and `.env`, resetting
DB defaults/role passwords through bootstrap. `--force` is required for any existing stack, even stopped.
Displaced `.env` and `data/` are retained privately as `.env.before-restore.<utc>` and `data.before-restore.<utc>`;
remove them only after validating recovery, according to your storage policy. Host age reads the identity only
for decryption; it is never mounted into a container. Restore remains LAN-only until Funnel is explicitly
re-enabled. Do not copy the laptop identity onto the server casually.
