# Server install

## Owner: host prerequisites

The owner applies the host module on **the server**; agents do not change host configuration or run sudo.
The module ships in the source tree, not in `dist/`:

1. Copy the module from the laptop's source root into the server's NixOS configuration:
   ```sh
   scp deploy/nixos-porchlight.nix <deploy-host>:<nixos-config>/porchlight.nix
   ```
2. Add `./porchlight.nix` to `imports` there and set the two machine values it requires. They stay in your NixOS
   configuration, never in this repo; `<server-user>` is the SSH login that runs the stack and `<data-mount>` equals
   `PORCHLIGHT_DEPLOY_DATA_MOUNT`:
   ```nix
   porchlight.host = { user = "<server-user>"; dataMount = "<data-mount>"; };
   ```
3. Apply the configuration (requires the owner's sudo password):
   ```sh
   sudo nixos-rebuild switch --flake <nixos-config>#<nixos-host>
   ```

Check the host prerequisites, logged in as `<server-user>`:

```sh
getent group porchlight
id
ls -ld /run/porchlight <data-mount>/porchlight
command -v minisign age
systemctl show docker -p RequiresMountsFor
```

Both directories must be `0750 <server-user>:porchlight`; `/run/porchlight` is recreated every boot.
`<server-user>` must belong to `porchlight`, both tools must be on PATH, and Docker's `RequiresMountsFor` must include
`<data-mount>` so it cannot start the stack against an empty mountpoint when the `nofail` mount is unavailable.
Incoming releases: `<data-mount>/porchlight-dist` must be a writable directory owned by the SSH user, or absent with a writable parent; install creates it with `mkdir -p -m 0750`.
`<server-user>` must log out and back in before the new group applies to interactive shells (or deployment uses `sg`).
The module opens LAN API/enrollment TCP ports `8443` and `8444`; it does not open private ports `9443` or `5432`
and does not alter host Tailscale configuration. Enrollment also checks the peer CIDR in the API.

The laptop resolves `porchlight.lan` via this entry in its `/etc/hosts` (no LAN DNS record is required).
Phones use [enroll-by-IP](devices.md#phones-without-lan-dns), with Funnel enabled first:

```text
<server-lan-ip> porchlight.lan
```

## Deploy

The LAN controller interface is:

```text
scripts/deploy-server.sh preflight
scripts/deploy-server.sh install <version> [--admin-name <display name>] [--admin-password-file <laptop 0600 file>] [--extra-san-dns <fqdn>]...
scripts/deploy-server.sh upgrade <version>
scripts/deploy-server.sh status
scripts/deploy-server.sh enroll-host <host>
scripts/deploy-server.sh enroll-uri --device <name> [--user <username>] [--out var/m9/enroll-uri.json] [--token-file <absolute server 0600 path>]
scripts/deploy-server.sh backup
```

Versions must be `MAJOR.MINOR.PATCH` (minor/patch 0–99, no leading zeros). There is no `--help` mode: invalid
usage prints this interface and exits 2. `--out` may be relative or absolute but must resolve under this checkout's
`var/`; it defaults to `var/m9/enroll-uri.json`. See [Funnel](#funnel-after-tailscale-pre-registration) for the post-LAN commands.

Run from the **laptop checkout**, as the owner, after host prerequisites and the laptop's `/etc/hosts`
entry are applied. Confirm SSH host-key trust interactively first. The server's machine values come from the gitignored `.env`
(see `.env.example`), never from the repo: `PORCHLIGHT_DEPLOY_HOST` (SSH host or `~/.ssh/config` alias, written
`<deploy-host>` below), `PORCHLIGHT_DEPLOY_DATA_MOUNT` (`<data-mount>`) and `PORCHLIGHT_DEPLOY_LAN` (the server's
LAN address with prefix, e.g. `192.168.1.10/24`, which sets `<lan-cidr>` and `<server-lan-ip>` below). There are no
defaults: a missing value exits 2 before any SSH, and an exported variable overrides `.env` for one command. The
SSH login is `<server-user>`, the account that owns the stack; preflight checks ownership against it.
The owner must be able to create/write `<data-mount>/porchlight-dist` on the data mount. The controller never
runs sudo, configures NixOS/LAN DNS, or generates recovery/signing keys.

Use a complete, trusted signed `dist/<version>/` from [release.md](release.md), including its **generated**
`install.sh`, `manifest.json`, `manifest.json.minisig`, and musl `porch`. Confirm the installer's embedded signing
public-key fingerprint out of band. `deploy/install.sh` is only a template, not a trust root. The controller
requires those three release files, copies with `rsync -a --protect-args --delete`, and compares remote/local
SHA-256 of installer and manifest **before execution**. The pinned installer then verifies the signature and
all release payloads. Transport hashes do not make an untrusted local installer safe.

```bash
umask 077
. scripts/env.sh
v=$(tr -d '\r\n' < VERSION)
scripts/deploy-server.sh preflight
PORCHLIGHT_DEPLOY_DRY_RUN=1 scripts/deploy-server.sh install "$v" --admin-name 'Owner'
scripts/deploy-server.sh install "$v" --admin-name 'Owner'
```

`preflight` is read-only, emits one `PASS|FAIL <check>` per check, and exits 1 if any fail. It checks NixOS;
`porchlight` group membership for the SSH login; `/run/porchlight` and `<data-mount>/porchlight` as
`0750 <server-user>:porchlight`; the data mount; minisign/age/rsync/Python ≥3.12; Docker/Compose and Docker's
`RequiresMountsFor=<data-mount>`; ports 8443/8444; and absence of a conflicting `porchlight` Compose project
root. Existing ports are accepted only when their bindings belong to this project.

Install refuses an existing `<data-mount>/porchlight/.env`: never delete production state to retry blindly.
Inspect a partial install privately; see [Failed first init recovery](#failed-first-init-recovery) below. This is a **fresh production init**:
no laptop test database, device profile or PKI is copied. A new household CA is generated on the server.
The only recovery material sent is the **public** age recipient derived locally with
`age-keygen -y var/backup-recovery.key`. Back up that private identity separately first; it never leaves the
laptop/personal backup (see [keys.md](keys.md)).

### Failed first init recovery

A rejected admin password can leave `.env`, `release-current`, signed `releases/`, loaded images, a migrated
`pgdata/` bind mount and the `porchlight` Compose project (postgres/migrate). `data/`, `backups/` and `backup-work/`
may also exist. Shell redirection creates `init.json` **before** init runs, so a password rejection normally leaves
an empty file, not necessarily an absent one; `init.log` holds the diagnostic. The controller refuses another
install because `.env` exists; upgrade cannot perform fresh init.

This procedure is **only for a confirmed password-policy rejection on the first install, before any successful
init or enrollment**. Inspect `init.log` privately on the server; never paste it or `.env`/`init.json` into reports.
If `data/pki`, `data/.init-staging`, a nonempty `init.json`, or any initialized database rows exist, **stop**:
use an owner-reviewed recovery/restore procedure, not a fresh init. Missing PKI alone does not prove no DB commit.
Do not use this procedure on an initialized production household or for an unexplained failure.

In an owner-controlled **server bash session** as `<server-user>`, check and stop only this project, then move failed state
aside (no deletion). **Stop at the first failure**; do not continue or retry install with partially moved state.
This is a manual, owner-reviewed recovery, never an automatic deployment step. Keep the same ROOT/RUN_DIR
as the installer for Compose's bind mounts. The child bash below exits on failure without closing the SSH session:

```bash
bash <<'RECOVERY'
set -euo pipefail
umask 077
ROOT=<data-mount>/porchlight
export PORCHLIGHT_ROOT="$ROOT" PORCHLIGHT_RUN_DIR=/run/porchlight
release=$(readlink -f -- "$ROOT/release-current")
dc=(docker compose -p porchlight --env-file "$ROOT/.env" -f "$release/compose.yml")
[[ -f "$ROOT/.env" && -f "$release/compose.yml" ]]
[[ ! -e "$ROOT/data/pki" && ! -L "$ROOT/data/pki" ]]
[[ ! -e "$ROOT/data/.init-staging" && ! -L "$ROOT/data/.init-staging" ]]
[[ ! -s "$ROOT/init.json" && ! -L "$ROOT/init.json" ]]
[[ -d "$ROOT/pgdata" && ! -L "$ROOT/pgdata" ]]
# Only counts are returned; both must be zero. If postgres is unavailable, stop and investigate.
counts=$("${dc[@]}" exec -T postgres psql -U postgres -d porchlight -At -c \
  'SELECT (SELECT count(*) FROM porchlight.install_state), (SELECT count(*) FROM porchlight.households);')
[[ $counts == '0|0' ]]
"${dc[@]}" down --remove-orphans
[[ -z $(docker ps -a -q --filter label=com.docker.compose.project=porchlight) ]]
saved="$ROOT/failed-first-init-$(date -u +%Y%m%dT%H%M%S)"
mkdir -m 700 -- "$saved"
# Docker's bind mount is root/postgres-owned: the stack user cannot reparent pgdata.
# Move it FIRST as container root, using the postgres image pinned in deploy/compose.yml.
# No .env or other failed state is moved until this command succeeds.
docker run --rm --pull=never --user 0 --entrypoint /bin/sh \
  --mount "type=bind,src=$ROOT,dst=/state" \
  postgres:18.6@sha256:5a5a84b19854a9ffaa54082c166ff4ec27473a361e496e5ea167f298f2da9722 \
  -c 'test -d /state/pgdata && test ! -L /state/pgdata && test ! -e "/state/$1/pgdata" && mv -- /state/pgdata "/state/$1/pgdata"' \
  sh "${saved##*/}"
[[ ! -e "$ROOT/pgdata" && -d "$saved/pgdata" ]]
# Remaining moves run as the stack user. Keep .env LAST so any failure still blocks install.
for entry in data init.json init.log backups backup-work .env; do
  if [[ -e "$ROOT/$entry" || -L "$ROOT/$entry" ]]; then
    mv -- "$ROOT/$entry" "$saved/$entry"
  fi
done
RECOVERY
```

Do not add `down -v`: this stack's `pgdata` is a **bind mount**, not a named volume, so even `down -v` would not
reset it. The one-shot root container renames `pgdata/` without changing ownership, before the stack user moves any
other state; `.env` stays in place until the last move, preventing a half-finished recovery from admitting install.
All renamed state stays on the data mount, retains its permissions and may contain secrets; keep the 0700 archive in
protected custody. If any check, container command, or rename fails, stop at that first failure and ask the owner
to review/complete recovery (preserving ownership); never substitute deletion. Keep the signed releases/`release-current` and loaded images;
they are safe to reuse. Leave `/run/porchlight` intact as the host-managed socket directory.

Before retrying from the laptop, prepare a vetted non-breached password of **12–1024 Unicode characters** in an
owner-owned 0600 file under `var/m9/` and use `--admin-password-file` (see below). The embedded breach list is
compiled from the release's `server/core/data/breached-passwords.txt`; check privately/offline, never via an
external password service.
Rerun the same signed version's `scripts/deploy-server.sh install` command after preflight. Confirm fresh init
and health before enrollment; do not delete the archived attempt automatically.

The controller uses these fixed production settings:

| Setting | Value |
| --- | --- |
| Root / incoming releases | `<data-mount>/porchlight` / `<data-mount>/porchlight-dist/<version>` |
| Socket directory / Compose project | `/run/porchlight` / `porchlight` |
| Enrollment peer CIDR / host | `<lan-cidr>` (from `PORCHLIGHT_DEPLOY_LAN`) / `porchlight.lan:8444` |
| Endpoint | `https://porchlight.lan:8443` |
| API GID | resolved from server group `porchlight`, not the stack user's primary group |
| Certificate SANs | DNS `porchlight.lan`, `localhost`; IP `127.0.0.1`, `<server-lan-ip>` |

Install always passes `--no-funnel`. Repeated `--extra-san-dns <fqdn>` adds SANs without replacing the required
ones; it does **not** activate Funnel or publish a new endpoint. Use the post-LAN `funnel` command below. Postgres stays
loopback-only on 5432; ingress 9443 is not published.

Without `--admin-name`, the controller prompts for a display name. The login username is `admin`.
Interactive install requires a local terminal on stdin and stdout; type the password only after the prompt appears.
The installer asks for the password with echo disabled over `ssh -t`. For automation, supply an existing,
nonempty, owner-owned regular **0600** laptop file instead:

```bash
scripts/deploy-server.sh install "$v" --admin-name 'Owner' \
  --admin-password-file "$PWD/var/m9/admin.pass"
```

Prepare that file privately (not a password literal in shell history). This path uses BatchMode SSH **without**
a TTY: the bytes travel only on stdin into a server 0600 temporary file below the stack's `tmp/`, removed by a
trap on success/failure. No password or PAT is put in argv. Remove the laptop password file after use.
Do not enable shell tracing or log `.env`, `init.json`, keys, passwords or enrollment URIs. Install ends with
`status`; initialization secrets stay in the server's 0600 `init.json`.

All commands honor `PORCHLIGHT_DEPLOY_DRY_RUN=1`: print planned remote/probe commands with secret values
redacted, without transport, key derivation, prompting, enrollment-file creation or server mutation. Install/
upgrade dry runs still require the local release files; an optional password file must still satisfy permissions.
Invalid usage/missing local inputs exit 2; transport/remote checks must succeed before dependent steps run.

## Enroll the first device

Record the fresh CA fingerprint **without printing the bootstrap token**:

```bash
ssh -o BatchMode=yes <deploy-host> \
  'python3 -I -c '\''import json; print(json.load(open("<data-mount>/porchlight/init.json"))["ca_fingerprint"])'\'''
scripts/deploy-server.sh enroll-uri --device 'Main laptop'
```

`enroll-uri --device <name> [--user <username>] [--out var/m9/enroll-uri.json]
[--token-file <absolute server 0600 path>]` defaults to user `admin`. Without `--token-file`, it reads
`bootstrap_token` from server `init.json`; with it, use a privately provisioned server PAT file. The token goes
through a remote 0600 temporary file to the active release's `porch --socket /run/porchlight/agent.sock
--token-file <tmp> --json enroll ...`, never through argv or terminal output. Server temporary files are removed
on success/failure. Save the bootstrap PAT in protected custody before removing `init.json`; do not remove it
until you have a working replacement admin PAT for future enrollments.

The laptop output is atomically saved under this checkout's `var/`: file **0600**, containing directory **0700**.
A failed request does not replace an existing output. Only `PASS enrollment URI saved to <path>` is printed.
Do not paste its JSON/URI into a terminal, logs, screenshots, bug reports or this runbook.

### Enroll this laptop

1. Quit the dev desktop app and disable any dev autostart while moving state. Rename (do **not delete**) the
   dev-era data directory before installing/enrolling the real production profile. Honor a custom XDG_DATA_HOME:

   ```bash
   data_home=${XDG_DATA_HOME:-$HOME/.local/share}
   if [[ -e "$data_home/porchlight" ]]; then
     mv -- "$data_home/porchlight" "$data_home/porchlight.dev-before-m9.$(date -u +%Y%m%dT%H%M%S)"
   fi
   bash "dist/$v/install.sh" desktop --source "dist/$v"
   "$data_home/porchlight/current/app/bin/porchlight"
   ```

2. Use the **same release** as the server. In another laptop terminal, copy directly to the Wayland clipboard:

   ```bash
   wl-copy < <(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["uri"], end="")' var/m9/enroll-uri.json)
   ```

   Paste in the app, verify the displayed CA fingerprint against the public fingerprint recorded above, then
   supply the admin password in the app. Treat clipboard history as sensitive; clear the clipboard/history after
   use (`wl-copy --clear`), and remove the consumed `var/m9/enroll-uri.json`. Mint a new URI for each device.
3. Choose the desktop key store: **Secret Service** if a provider runs in this user's D-Bus session (unlock it
   through its own prompt), otherwise the **protected file** (0600 key secret in a 0700 directory; encrypted disk
   recommended). The file choice is also available if a provider is present but unusable. No KDE/GNOME dependency
   is required; the recorded backend never silently changes. Re-enroll to switch backends.
4. Prove a LAN enrollment succeeds and the API enrollment log records the laptop's real `<lan-cidr>` peer,
   not a Docker gateway. Do not widen `ENROLL_CIDRS` to compensate. Record evidence and the moved M5 desktop
   attestations in [server-acceptance.md](server-acceptance.md). M5 demo/soak scripts remain **dev-only**.

## Funnel (after Tailscale pre-registration)

First complete the owner's prerequisites in [docs/operations/funnel.md](funnel.md): HTTPS/MagicDNS,
`tag:porchlight`/tagOwners and Funnel nodeAttr, neutral machine name/randomized tailnet name, and a single-use
pre-approved tagged auth key. Merge/release the installer+runbook part A before running these live commands.
The **active signed release** on the server must contain `compose.funnel.yml` and the `funnel`/`server-cert`
installer modes; a newer standalone installer does not upgrade `release-current`. Keep the working LAN stack
and household CA; never enable public enrollment.

Choose a vetted Tailscale version and obtain its immutable repository digest on **the server**:

```bash
# Owner/controller server session; substitute the vetted version.
docker pull tailscale/tailscale:<version>
docker image inspect --format '{{index .RepoDigests 0}}' tailscale/tailscale:<version>
```

Verify the returned `tailscale/tailscale@sha256:<64 lowercase hex>` reference, then set the public `image`
variable to that reference on the laptop. Pre-pull it on the server before activation so download time is not
part of the installer's login budget. Prepare `var/tailscale-authkey` privately (no literal in shell history):
nonempty regular file, no symlink, owned by your user, **0600**, under `var/` with private containing directories.
Do not enable tracing or paste the key, stack `.env`, Compose config or tailscaled state into logs.

From the laptop checkout:

```bash
scripts/deploy-server.sh funnel --ts-hostname quiet-node --tailscale-image "$image"
# Only after PASS funnel <fqdn>:
rm -f var/tailscale-authkey
scripts/deploy-server.sh funnel-check
```

Exact interfaces:

```text
scripts/deploy-server.sh funnel --ts-hostname <name> --tailscale-image tailscale/tailscale@sha256:<64 lowercase hex> [--ts-authkey-file <laptop path> | --no-authkey]
scripts/deploy-server.sh funnel-check [--fqdn <name>]
```

Hostname is 1–63 lowercase letters, digits or hyphens. FQDNs must match
`^[a-z0-9-]+(\.[a-z0-9-]+)*\.ts\.net$` and valid DNS labels (including installer PASS output and saved state).
The key path defaults to this checkout's `var/tailscale-authkey`; relative paths are checkout-relative. Local
validation happens before SSH. The key travels only on BatchMode SSH **stdin**, explicitly `ssh -T`, never
through argv or a TTY. One remote session writes a private mktemp file, atomically replaces
`<data-mount>/porchlight/tmp/m9-ts-authkey` (0600 in 0700), then invokes the installer. A remote EXIT trap removes
the transport and temporary files on success **and failure**, including `--no-authkey` runs; a subsequent run
replaces any leftover from an untrappable interruption. The trusted installer's safe stdout/stderr reaches the
operator, including LAN-only and staged-key retry guidance. Never paste private sidecar logs or login URLs.
The laptop source is never deleted automatically: delete the actual custom path too after successful activation.

Only exit success plus a valid `PASS funnel <fqdn>` publishes `var/m9/funnel-fqdn` atomically (0600).
The wrapper's `--no-authkey` means **no new laptop key**: it omits BOTH installer key flags, allowing part A to
reuse its private staged `$ROOT/tailscale/authkey` first, or existing node state otherwise. It is not a passthrough
of the installer's stricter `--no-authkey` (node-state-only). Both installer retry hints map to the wrapper below:

```bash
# After failure AFTER staging, or for an idempotent rerun with saved node state:
scripts/deploy-server.sh funnel --ts-hostname quiet-node --tailscale-image "$image" --no-authkey
# After failure BEFORE staging (upgrade/prerequisites first), or with an expired/revoked staged key:
# Keep/replenish the private laptop source; use a fresh owner-approved key if necessary.
scripts/deploy-server.sh funnel --ts-hostname quiet-node --tailscale-image "$image" --ts-authkey-file var/tailscale-authkey
```

A failed activation can leave `FUNNEL_ENABLED=0`, unchanged endpoints/SANs, and a staged installer key for an
explicit retry; no transport copy is needed for recovery. If abandoning that retry, remove the staged key privately
on the server: `ssh -T -o BatchMode=yes <deploy-host> 'rm -f -- <data-mount>/porchlight/tailscale/authkey'`.
Restore also leaves the stack **LAN-only**: `funnel-check` fails until explicit reactivation as above with surviving
state, or with a new key when no staged key/node identity survived. A same-root restore can retain identity; never
copy identity to two running nodes. `PORCHLIGHT_DEPLOY_HOST` and `PORCHLIGHT_DEPLOY_DRY_RUN=1` work for both commands;
dry-run does not read key bytes, invoke transports/probes, save the FQDN, or create certificate/log files.

`funnel-check` defaults to the saved FQDN. It exits 1 on any failed assertion, with these precise check names:

- `compose-valid`, `api-health`, `tailscale-running`: quiet Compose validation, internal health, Running with matching FQDN.
- `authkey-cleared`: empty runtime `TS_AUTHKEY`, no `/config/authkey` (including symlinks).
- `raw-tcp-proxy-v2`: live TCP 443 → `api:9443`, PROXY v2 and AllowFunnel enabled.
- `leftover-authkey`: neither `$ROOT/tmp/m9-ts-authkey` nor `$ROOT/tailscale/authkey` exists (including dangling symlinks).
- `private-state`: private modes, empty persisted auth reference, Funnel enabled and exact LAN+Funnel endpoints.
- `production-sans`: required LAN/localhost/IP/FQDN SANs. **Not strictly read-only**: creates a one-off
  `dc run --rm --no-deps -T api server-cert --show --json` container and runs its installation verification.
- `remote-status`: aggregate remote assertions; `FAIL status-transport` instead only for SSH exit 255.
- `household-ca`: fetches only public `data/pki/ca.crt` over authenticated SSH into `var/m9-funnel/ca.crt` (0600 in 0700).
- `probe-path`: resolve before probing; any `100.64.0.0/10` or `fd7a:115c:a1e0::/48` answer refuses the probe.
  Run the laptop **off-tailnet** (Tailscale down) to exercise public Funnel, not tailnet Serve. DNS failure also fails.
- `public-tls`: the verified TLS/mandatory-mTLS proof below; skipped when `probe-path` fails.

It never prints `.env` or full Compose configuration. Private `var/m9-funnel/tls-check.log` contains only fixed,
non-server-controlled diagnostics. Static sidecar IP, userspace mode and absence of a host 9443 listener require
separate live evidence in the acceptance checklist; `funnel-check` does not assert them.

M9-R5 public probe is isolated `python3 -I`, using stdlib `ssl.create_default_context(cafile=<household CA>)`,
TLS 1.3 minimum, SNI and SAN hostname verification (CN fallback disabled), with no client certificate or HTTP
request. `PASS public-tls` requires the client handshake to complete with the verified household-CA/FQDN identity,
then the **first recv** to raise `ssl.SSLError` with reason `TLSV13_ALERT_CERTIFICATE_REQUIRED` (or
`CERTIFICATE_REQUIRED`). There is **no OpenSSL diagnostic-text matching**. Chain/name failures, any data or EOF
without that alert, other errors and timeouts fail. The connection/handshake/read budget is 20 seconds, with a
22-second outer watchdog also bounding DNS/runtime hangs. Probe exit 0 now means PASS, not the old OpenSSL alert
exit rule. This does not attest authenticated application behaviour; never weaken public mTLS.

Refresh enrolled phones' endpoint lists **while still on LAN**; confirm LAN still works and the household CA
is unchanged. Then turn Wi-Fi off: require normal mTLS/session cellular CRUD, SSE reconnect/update, attachments,
notifications and revocation, plus connection metadata `transport=funnel` and the forwarded cellular source.
Record observations and the owner's crt.sh check in [server-acceptance.md](server-acceptance.md). Never export
device keys. If raw-TCP Funnel through the host VPN fails, **stay LAN-only**, record it, and bring options to the owner;
do not change egress or weaken TLS/enrollment boundaries without approval.

## Upgrade

Once a release with remote updates is installed, log in as the stack owner on the server and run:

```bash
<data-mount>/porchlight/porchlight-upgrade             # latest
<data-mount>/porchlight/porchlight-upgrade <version>   # explicit signed version
```

The atomic 0750 wrapper stores the installation's root/run-dir/project. The trusted **installed** installer verifies
the GitHub manifest with its embedded key, refuses downgrades, downloads and fully verifies the versioned dist,
then hands off to its verified installer. `PORCHLIGHT_RELEASE_REPO` in the stack `.env` defaults from the signed
release's repository only if absent; explicit admin values, including empty to disable checks, win. A same-version
request is a no-op. See [upgrade.md](upgrade.md) for download policy and recovery limits.

Laptop-driven upgrades remain available (and bootstrap remote updates on older installations):

```bash
scripts/deploy-server.sh preflight
PORCHLIGHT_DEPLOY_DRY_RUN=1 scripts/deploy-server.sh upgrade "$next_version"
scripts/deploy-server.sh upgrade "$next_version"
```

Prepare a new trusted signed `dist/$next_version/` first; never rewrite a published version. The controller copies
and hash-checks it, invokes that release's installer with the production root/run-dir/project, then runs `status`.
Both paths quiesce writers, pin the encrypted backup outside retention, take a private temporary plain database
dump outside the API's backup mount, migrate and health-check. Failure automatically switches back, restores the
dump if migration began, and starts the old version; `ROLLED BACK` still exits 1. `ROLLBACK FAILED` requires the
manual encrypted-bundle restore fallback in [upgrade.md](upgrade.md). Preserve previous distributions and pinned
bundles; never copy the laptop recovery identity onto the server casually.

## Status/backup

```bash
scripts/deploy-server.sh status
scripts/deploy-server.sh backup
```

Status shows the active `release-current`, Compose projects/services and the API's internal healthcheck;
it does not print Compose config or secrets. Backup runs the **active** installer and prints only its
`PASS backup porchlight-<timestamp>.tar.age` line (or fails). Encrypted bundles are in
`<data-mount>/porchlight/backups`; scheduling, retention and independent laptop restore are described in
[backup-restore.md](backup-restore.md). A successful backup is not a restore drill.

For the M9 backup→laptop drill, record a specific bundle filename from `backup` and the active release version.
Copy only that ciphertext, using SSH host authentication, to a new private laptop `var/m9/` directory. Compare
remote and local SHA-256 before decrypting; keep the private identity local. Run:

```bash
# Set bundle_name to the exact PASS filename (not a path); v to its matching signed release.
umask 077
mkdir -m 700 -p var/m9/server-backups
chmod 700 var/m9/server-backups
# Refuse paths or shell metacharacters before using the filename in a remote command.
[[ $bundle_name =~ ^porchlight-[0-9]{8}T[0-9]{12}Z[.]tar[.]age$ ]] || exit 2
ssh -o BatchMode=yes <deploy-host> \
  "sha256sum -- '<data-mount>/porchlight/backups/$bundle_name'"
rsync -a --protect-args -e 'ssh -o BatchMode=yes' \
  "<deploy-host>:<data-mount>/porchlight/backups/$bundle_name" var/m9/server-backups/
chmod 600 "var/m9/server-backups/$bundle_name"
# Compare these hashes; stop on a mismatch before decrypting.
sha256sum "var/m9/server-backups/$bundle_name"
deploy/scripts/restore-drill.sh "var/m9/server-backups/$bundle_name" \
  --source "dist/$v" --identity var/backup-recovery.key
```

The drill uses a separate laptop `porchlight-drill` project, private `var/restore-drill`, loopback spare ports,
and compares rows/migrations/CA/blob bytes before starting writers. It must never restore into the production
root. Also verify passive desktop pull with a matching receipt ([passive-pull.md](passive-pull.md)). Complete
restart/reboot, VPN relay-change, phones and later Funnel gates in [server-acceptance.md](server-acceptance.md).
