# Deployment hardening acceptance

These policies require a new signed release, not edits to an installed signed distribution. Offline contracts
(`python3 -I deploy/tests/prerollout-deploy-hard.py`) are not evidence that Docker accepted the settings.

## Boundaries and writable paths

Production publishes only IPv4 LAN API/enrollment addresses. Fresh install resolves exactly one local IPv4 in
`ENROLL_CIDRS` and persists `LAN_BIND`. Ambiguous/missing discovery requires `--lan-bind <server-LAN-IP>`;
`0.0.0.0` requires the explicit `--lan-bind 0.0.0.0` flag, not an environment default. IPv6 wildcard publication
is not enabled. Docker-published ports can bypass host INPUT rules: neither NixOS firewall rules nor enrollment's
peer check substitutes for the host bind. Check the host's LAN, tailnet and VPN interfaces independently.

An existing stack without `LAN_BIND` gets an explicit notice: upgrade preserves its old IPv4 wildcard rather
than silently losing connectivity. After checking the intended address, run the verified installed installer:

```sh
bash <stack-root>/release-current/install.sh lan-bind --lan-bind <server-LAN-IP> \
  --root <stack-root> --run-dir <socket-directory> --project porchlight
```

That operation recreates and health-checks the API and restores its previous configuration on handled failure.
The existing release's Compose file must support `LAN_BIND`. A network upgrade's verified new-installer handoff
retains the installed stack settings; configure the bind after the hop if the old wrapper cannot accept new flags.

Production PostgreSQL has **no host port**. Installer, backup and restore use `docker compose exec` or container
TCP. Native development tools and `scripts/dev-backup-role.sh` still use the separately published loopback port
in `compose.dev.yml`; its backend remains non-internal for those host callers. Production backend is internal:
PostgreSQL, migration and backup require no internet; API GitHub/ICS egress and the userspace Tailscale sidecar
use the non-internal ingress network. This is not an outbound allowlist for the API.

API/migrate/backup root filesystems are read-only, all capabilities are dropped and no-new-privileges is set.
API writes use `/data` (blobs, ingestion staging, PKI, secrets), `/run/porchlight` (agent socket), and bounded
128 MiB `/tmp` (anonymous export/library staging). Migration only needs a 16 MiB `/tmp`. Backup uses writable
`/backups`, private disk `/dumpwork`, 512 MiB `/work` tmpfs and 16 MiB `/tmp`; its data/config/credentials mounts
are read-only. These are initial bounds: validate the largest household export as well as attachment/backup
workloads; a temporary-space cap can reject a large export. Dev Compose mirrors the privilege/rootfs policy.

PostgreSQL retains its image's default capabilities pending inspection of the exact pinned image entrypoint;
no-new-privileges is enforced. **Capability reduction is not yet attested.** A fresh-cluster proof must precede
adding `cap_drop: [ALL]` plus `CHOWN,DAC_OVERRIDE,FOWNER,SETGID,SETUID`. Never test init by deleting a real cluster.
Tailscale runs as the installer UID/GID with all capabilities dropped (userspace mode, no TUN/NET_ADMIN); its
private state bind and UID/GID-owned runtime tmpfs remain writable. Rootless containerboot is controller-gated.

API/PostgreSQL/Tailscale steady health probes use 30 seconds, three failures and short timeouts (about 100 seconds
to unhealthy). Start periods stay generous. PostgreSQL gets a 60-second stop grace. Health failure is diagnostic,
not an automatic restart loop. Bootstrap enables `pg_stat_statements` and restricts its functions/views from
PUBLIC; upgrade reruns bootstrap as the superuser before migrate, never asking the migrator to create extensions.

## Credentials and storage alerts

Production Docker environments contain `*_FILE` paths, not credential values. Installer provisions private
0700 `credentials/` with 0600 files before first use; API/migrate/backup only mount their respective credential.
The PostgreSQL root wrapper reads role files before the official entrypoint drops UID; direct role bootstrap
also supports files. Values never enter argv. Existing environment credentials remain supported for old images
and the rollback hop. The private stack `.env` deliberately retains legacy credentials so old-image rollback and
independent encrypted bundles remain self-contained; `credentials/` is regenerated from it on install/upgrade/
restore, not carried as another backup payload. A host/container operator can still read runtime secrets.

A daily persisted scheduler job checks the server leaf. Below 60 days it retains the household CA, private key
and SANs, fsyncs an atomic leaf replacement, and the shared rustls resolver serves that leaf on subsequent LAN,
Funnel and enrollment handshakes without restart. Resumption stays disabled. Existing TLS connections need no
renegotiation. Certificate notifications use 90/60/30/7-day bands, deduped by expiry/band/recipient; successful
renewal normally prevents reaching the 30/7-day bands. Renewal failures retain the alert and retry daily. These
notification kinds use server-generated, actionable certificate-band/expiry/renewal-failure and disk-space/inode/
backup-headroom titles and bodies in both clients' existing inbox presentation. Only live household administrators
receive them. They remain separate even within one activity window; quiet hours still delay delivery but do not
replace operational wording with a generic digest. No credentials or concrete filesystem paths enter summaries.

Hourly disk checks inspect data and backup filesystem available blocks/inodes: below 15% or below the measured
backup working-space high-water mark sends at most one notification per storage class/admin/UTC day. Backup
persists `.working-space-need` (0600): concurrent dump plus encrypted-output bytes, not database-size guessing.
Before the first successful measurement only percentage thresholds apply. This is headroom telemetry, not a
quota or a guarantee against other containers consuming the host filesystem.

## Scoped housekeeping

```sh
bash <stack-root>/release-current/install.sh prune --root <stack-root> --run-dir <socket-directory> --project porchlight
# Review the dry-run, then repeat with --apply.
```

The per-stack installer lock covers both modes. Only semver `porchlight-api` / `porchlight-backup` image tags and
semver directories directly under this stack's `releases/` are candidates. Keep the current release, previous
known-good release, all versions referenced by retained rollback bundles, and versions referenced by containers
(including stopped containers/image-ID aliases). No Docker system prune, force deletion, volumes, unrelated
repositories, incoming distributions or backup ciphertext are touched. New pinned rollback bundles get a private
`.tar.age.version` sidecar. Older bundles lack this mapping: prune refuses rather than guessing from timestamps.
The operator must independently identify and record each matching version, or retain everything. Release
manifests must match their directory version; symlink/recovery ambiguity fails closed.

## Controller proof (laptop only)

From the main checkout, with no production commands:

```bash
umask 022
. scripts/env.sh
scripts/dev-stack.sh up
( . scripts/dev-db.sh; dc --profile backups build backup; dc --profile backups up -d --wait backup )
( . scripts/dev-db.sh; dc exec -T postgres sh /docker-entrypoint-initdb.d/10-porchlight.sh )
( . scripts/dev-db.sh; dc --profile backups run --rm --no-deps -T backup now )
# Inspect only non-secret hardening fields; never print Env or expanded Compose config.
for service in api migrate backup postgres; do
  id=$(docker ps -aq --filter label=com.docker.compose.project=porchlight-dev --filter label=com.docker.compose.service="$service")
  test -n "$id"
  docker inspect --format '{{.Name}} rootfs-ro={{.HostConfig.ReadonlyRootfs}} dropped={{json .HostConfig.CapDrop}} security={{json .HostConfig.SecurityOpt}}' "$id"
done
( . scripts/dev-db.sh; dc exec -T api /usr/local/bin/porchlight healthcheck; dc exec -T backup bash /opt/porchlight/backup/backup.sh health )
( . scripts/dev-db.sh; dc exec -T postgres psql -X -U postgres -d porchlight -At -c "SHOW shared_preload_libraries; SHOW effective_cache_size; SHOW autovacuum_work_mem; SELECT extname FROM pg_extension WHERE extname='pg_stat_statements';" )
bash deploy/scripts/m1-demo.sh
bash deploy/scripts/m2-demo.sh
bash deploy/scripts/m3-demo.sh
bash deploy/scripts/m7-demo-ics.sh
bash deploy/scripts/m7-demo-planning.sh
# Existing full gate set also covers M1/M2/M3/M7, packaged clients and managed devices:
bash scripts/ci.sh --full
```

Use `--keep` on the isolated M2/M3/M7 gates when inspecting their service hardening (select their project label,
not `porchlight-dev`); then use their documented teardown. Require init, migration, real mutations/SSE, attachment
staging, encrypted backup and independent restore to pass under the hardened mounts. Test fresh PostgreSQL
initialization in an isolated project, not merely an already initialized cluster. Separately rehearse a signed
production-Compose install→upgrade→rollback→restore on laptop spare binds/ports (M8 acceptance) and userspace
sidecar activation with an owner-provided vetted image/key. Sidecar/Funnel cannot be proven by the HAProxy dev
stand-in. Record new-connection certificate fingerprints before/after injected-time renewal, steady probe
failure timing, `/tmp` pressure, cgroup peaks and graceful database shutdown. Offline tests assert policy and
exercise mocked installer/fsync/reboot plus real Rust TLS/SQL, but cannot attest Docker, host firewall or egress.
