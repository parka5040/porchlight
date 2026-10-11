# Install from a signed release

## First install from GitHub Releases

Download `porchlight-<V>-dist.tar`, `manifest.json` and `manifest.json.minisig` from the chosen GitHub Release.
Before running anything, compare the minisign key fingerprint in the release notes with the independently trusted
public anchor in `deploy/release-keys/minisign.pub`; do not trust a key merely because it arrived with the download.
Verify the separately downloaded manifest **before extraction** using that trusted key
(`minisign -Vm manifest.json -x manifest.json.minisig -p <trusted-public-key-file>`). Keep its signature alongside it.
From an independently trusted checkout, run
`python3 -I -B deploy/lib/operations.py extract <downloaded-tar> <new-private-directory> <verified-manifest.json>`.
This checks the entire archive's names/types/sizes against the signed inventory before writing members, caps
expanded bytes/member count from that inventory, bounds extended headers, and rejects links/special files.
Compare the extracted manifest with the verified download and check the extracted `install.sh` size/SHA-256
against its signed entry before executing it. Never run an unverified archive's helper. The generated
`install.sh` then verifies every release member before installing.

Run `bash <extracted>/install.sh server --source <extracted> ...` with the server options below, or `desktop` for
a first desktop install. Keep the complete signed distribution for recovery. **For 0.1.6 → next release**, use
`scripts/deploy-server.sh upgrade <new-version>` with the laptop-verified signed dist: the old network wrapper
extracts unsigned archives before handoff, and old `upgrade --source` retains unsafe rollback code. Later releases
can use the [one-command wrapper](upgrade.md), which hands off to the verified new installer.

## Local artifacts or SSH

Bootstrap trust: obtain **only the generated `dist/<version>/install.sh`** from the owner's trusted signed distribution and confirm the
embedded public-key fingerprint out of band. Verification helpers (bash and Python) are inlined in that single file;
it can be copied alone to a host and used with `--source`. No source `lib/` code is loaded before verification. The
checkout `deploy/install.sh` is a template, not an executable trust root. Running an installer from an untrusted source cannot bootstrap its own
trust. It verifies the selected source's minisign signature and **all** member hashes before using artifacts. Never
substitute a public key from the same untrusted source. Native bash, Python 3.12+, minisign, tar, flock and (for restore) age
are required; SSH sources additionally require rsync/ssh. The installer never fetches packages; `upgrade --release`
fetches signed artifacts over HTTPS (plain HTTP is limited to the explicit loopback test harness).
Source format is a local directory or `user@host:/absolute/path` (`rsync --protect-args`); remote installer code is not
executed during fetching. Artifact install/upgrade/restore modes use `--source`; `funnel`, `server-cert`, `enroll-host` and `backup`
operate on the already installed `release-current` instead (upgrade first when adding capabilities).
For upgrades, execute the **new** verified installer, never the old installer with a new `--source`. For restore,
execute the newest verified installer with the bundle's matching signed `--source` artifacts, not its old installer.

## Server on this laptop

Live clients assume the default 120-second SSE heartbeat. Keep `PORCHLIGHT_STREAM_HEARTBEAT_SECS=120` (or shorter); increasing it requires coordinated client `STREAM_HEARTBEAT_MS`/`STREAM_WATCHDOG_MS` changes and idle-stream verification. Both client watchdog and line-read timeout are 250 seconds with current constants. See `client-live.md` for resume and recovery behaviour.

Use spare ports, loopback binds, separate networks and all runtime state under `var/`. Do not touch the production server.
The operator supplies an existing **public age recipient** from the laptop recovery identity, never the identity:

```bash
umask 077
. scripts/env.sh
v=$(tr -d '\n\r' < VERSION)
recipient=$(age-keygen -y var/backup-recovery.key)
# Supply var/install-admin.pass yourself (0600, password policy applies); never echo it in a command/log.
ENROLL_CIDRS=127.0.0.1/32 LAN_BIND=127.0.0.1 PG_PORT=25432 LAN_PORT=28443 ENROLL_PORT=28444 \
  ENROLL_HOST=localhost:28444 PORCHLIGHT_ENDPOINTS=https://localhost:28443 \
  bash "dist/$v/install.sh" server --source "dist/$v" --root "$PWD/var/install-test" \
  --run-dir "$PWD/var/install-test/run" --project porchlight-install-test --no-funnel \
  --backup-recipient "$recipient" --admin-name 'Laptop Admin' --admin-password-file "$PWD/var/install-admin.pass"
```

Without `--admin-password-file`, prompts ask for display name and hidden password. Additional options:
`--admin-username`, `--household`, `--tz`, repeated `--san-dns`/`--san-ip`. The init output contains a bootstrap PAT and is saved only to
`<root>/init.json` (0600); never paste it into logs. Recover/save the PAT privately, then delete the init output.
Initial inputs (including password policy and the server's bundled storage-timezone allowlist) are checked
before publishing `.env`. `Factory`, host-only `localtime`, and `posix/` or `right/` names are rejected; supported
IANA aliases such as `US/Eastern` are accepted and canonicalized by the server. A private, durable
`.provisioning` marker binds the release and provisioning options until API/backup startup completes.
Retries compare all effective non-secret deployment inputs with `.env` (UID/GID, CIDRs, ports, bind address,
endpoints, subnets and minimum client version); changed inputs are refused before service changes. Compose uses
this persisted configuration, never inherited overrides. Retry with the original deployment inputs; after
completion change configuration through the supported operations. Credentials are never regenerated to correct inputs.
If image loading, migration, init or startup fails, correct the cause and rerun the same `server` command (or
controller `install`) with the same release/options; a corrected password is allowed. Credentials are reused,
recoverable init is resumed, and acknowledged init output/PKI is not regenerated. A flushed delivery is recovered
even if init acknowledgement cleanup or the installer itself was killed. After preserving that delivery, resume
reruns server init to check publication/fingerprints and acknowledge/remove `data/.init-staging`, discarding any
redelivered stdout rather than creating another plaintext copy. The server's `AlreadyInitialized` exit is accepted
only after PKI/database verification and complete staging cleanup. Do not delete `.env`, PKI,
`init.pending.json`, `init-attempt.json` or provisioning markers to retry; after success only `init.json` remains. Upgrade refuses
unfinished provisioning. Older installers without a marker require the [legacy recovery procedure](server-install.md#failed-first-init-recovery).

By default certificate SANs include `porchlight.lan`, localhost/127.0.0.1, plus the hostnames/IPs in `ENROLL_HOST`
and `PORCHLIGHT_ENDPOINTS`. Explicit SAN options replace this default list; supply every required endpoint name. Enrollment peer-CIDR
checking is intentionally strict: verify Docker's source IP preservation with the M2 gate before enrolling devices.
Do not add arbitrary Docker gateway CIDRs to a production LAN policy.

The root defaults to `/var/lib/porchlight`: `pgdata/` (PG18 volume at `/var/lib/postgresql`), `data/` (PKI, secrets,
recipient and blobs), `backups/`, `.env` (random role secrets, 0600), and versioned deployment files. The socket
bind defaults to `/run/porchlight`. Root/run/project options must be repeated for subsequent commands on a scratch
install (or exported through PORCHLIGHT_ROOT/PORCHLIGHT_RUN_DIR/PORCHLIGHT_PROJECT). UID/GID must own data,
backups and socket dirs; don't start the rootless API against root-owned directories.

Postgres → one-shot migrator → explicit init → healthy API → backup scheduler. Only migrate sees the migrator
credential; API sees app only; backup sees its read-all-data role only. API listeners are LAN mTLS 8443, enrollment
8444, unpublished ingress 9443. Host DB bind is loopback only. `BACKEND_SUBNET`, `INGRESS_SUBNET`, ports and binds
are overridable. `server --funnel` is refused: install LAN first (`--no-funnel`), then explicitly use
`install.sh funnel` after completing the [Funnel prerequisites](funnel.md).

## Change the enrollment host (existing stack)

Use the generated, installed release's installer; upgrade first if it lacks this mode:

```bash
bash <data-mount>/porchlight/release-current/install.sh enroll-host --host <server-lan-ip> \
  --root <data-mount>/porchlight --run-dir /run/porchlight --project porchlight
# From the laptop (honours PORCHLIGHT_DEPLOY_HOST and PORCHLIGHT_DEPLOY_DRY_RUN=1):
scripts/deploy-server.sh enroll-host <server-lan-ip>
```

`--host <DNS-name|IPv4>[:port]` defaults the port to the installed `.env`'s `ENROLL_PORT` (8444 if absent).
IPv6 is refused. The host must already be in the **current** certificate SANs; if missing, run
`install.sh server-cert` with the complete required SAN set first. Only `ENROLL_HOST` changes: neither the
certificate, enrollment CIDRs/listener ports nor `PORCHLIGHT_ENDPOINTS` are changed. An API health-checked
recreation and backup scheduler bind-mount refresh precede the durable `.enroll-host-applied` marker. Matching
desired `.env` alone is not completion: after an interruption, rerun the same command to reconcile both services.
Only a matching applied marker permits a no-restart rerun. A handled failure restores the previous `.env` and
recreates API/backup with it; recovery failures include an operator diagnostic. Success prints
`PASS enroll-host <server-lan-ip>:8444` for this example.
Generate **new** enrollment URIs after the change; previously generated URIs retain their old host.
For phones with no LAN DNS, [enable Funnel first, then enroll by LAN IP](devices.md#phones-without-lan-dns).

## Desktop

```bash
bash dist/0.1.0/install.sh desktop --source /path/to/dist/0.1.0
# SSH alternative; installer verifies after copying
bash dist/0.1.0/install.sh desktop --source owner@laptop:/path/to/dist/0.1.0
porchlight-update /path/to/dist/0.2.0
porchlight-update --rollback
```

Desktop in-app updates download only the signed desktop subset from the household server, never GitHub. The
trusted installed installer verifies minisign first, then every present member's hash/size and the complete
`release.json.desktop.files` inventory, including `release.json` itself. Unlisted files, missing required members,
and symlinks are refused. Full distributions still work; legacy releases without `release.json` require the
complete inventory. Lower versions are refused (`porchlight-update --rollback` remains the explicit exception).
An identical same-version manifest **and complete installed tree** yield `PASS already installed <V>` without
changing launchers or rollback history; differing same-version manifests are refused. Every new/retried tree is
assembled in a private sibling under `releases/` on the target filesystem, checked in full (including launcher,
runtime and trusted updater), then renamed into place. Incomplete existing trees are moved to
`releases/.incomplete.<version>.*` for inspection and rebuilt; a matching manifest alone is never completion.
Desktop expansion is capped at 4 GiB and 100,000 members, with bounded tar headers.

Defaults: `~/.local/share/porchlight/releases/<version>/app`, atomic `current`, `previous` for rollback,
`~/.local/bin/porchlight-update`, launcher and hicolor scalable icon in XDG_DATA_HOME. The installer writes no XDG
autostart entry; "Start at login" is a setting inside the app. HOME/XDG_DATA_HOME/XDG_CONFIG_HOME/XDG_STATE_HOME are honored, including paths with spaces.
Add `--niri` only to explicitly opt into the idempotent `spawn-at-startup ... "--tray"` line (the app then refuses to
add its own XDG autostart, so Porchlight never starts twice). Do not run the
desktop installer as root. niri edits preserve existing text, refuse symlink configs, keep the original `config.kdl.bak`
and publish by atomic rename before switching the app; reinstalling the current release preserves `previous`.
If an existing niri startup line names a different `bin/porchlight` path, the installer warns on stderr and leaves
that line unchanged; update it manually rather than adding a second startup mechanism. The app's C4 tray/start-minimized handling must be verified after client integration;
this workstream installs the launch entries, not tray UI behavior.

The packaged `app/bin/porchlight` launcher automatically enables Java's non-reparenting-WM mode for Wayland
sessions outside GNOME, KDE, Cinnamon/X-Cinnamon, Pantheon and Unity, avoiding black/unresized content under
niri/xwayland-satellite and other non-reparenting bridges. No manual niri environment configuration is needed.
`PORCHLIGHT_AWT_NONREPARENTING=0|1` disables/forces this behavior; an already-set
`_JAVA_AWT_WM_NONREPARENTING` always takes precedence. Plain X11 sessions keep Java's own detection.

Desktop key custody prefers Secret Service (any provider: GNOME Keyring, KeePassXC, KWallet/ksecretd); no KDE
component is required. The desktop installer prints a hint: run a Secret Service provider in the same user D-Bus
session before first enrollment, or Porchlight keeps the device key secret in an owner-only file (0600 in a 0700
folder; keep the disk encrypted). Porchlight requests standard activation, nudges KDE's
`org.kde.secretservicecompat` only on `ServiceUnknown`, waits briefly for the standard name, and retries once;
other activation errors fail closed. A locked collection gets the provider's own unlock prompt. The backend is
recorded at enrollment and never switched silently (re-enroll to change it). It does not install/start another
user's daemon. See [client trust](client-trust.md#desktop-key-backends-owner-decision-2026-10-07) for the backends
and the live controller check.

## Production server (M9): LAN first, then Funnel

The owner applies `deploy/nixos-porchlight.nix` (Docker, minisign/age, group, persistent SSD root and boot-time
socket directory) and rebuilds NixOS. The laptop resolves `porchlight.lan` through its own `/etc/hosts`;
phones without LAN DNS enroll by `<server-lan-ip>` and sync via Funnel (see above). Follow
[server installation](server-install.md) for the controller's signed release transfer, fresh production init and
device enrollment. Never copy laptop test data or PKI: production gets its own household CA.

Use root `<data-mount>/porchlight`, run dir `/run/porchlight`, project `porchlight`; signed releases arrive in
`<data-mount>/porchlight-dist/<version>/`. Production values are `ENROLL_CIDRS=<lan-cidr>`,
`ENROLL_HOST=porchlight.lan:8444`, `PORCHLIGHT_ENDPOINTS=https://porchlight.lan:8443`; initial SANs must include
`porchlight.lan`, `localhost`, `127.0.0.1`, `<server-lan-ip>`. Enrollment remains LAN-only on 8444; LAN mTLS is
8443; Funnel reaches only the unpublished 9443 listener, with PROXY v2 from the sidecar's static IP.

[Funnel setup and verification](funnel.md) contains the owner's Tailscale pre-registration checklist, verified
image-digest procedure, key-file handling, exact `funnel` / `server-cert` interfaces, and live acceptance commands.
The follow-up's laptop `funnel-check` verifies the public port-443 TLS server chain against the household CA's public
`data/pki/ca.crt`, with SNI and an exact FQDN SAN match; the subsequent certificate-required alert is expected (M9-R5).
This is not an unauthenticated health curl: mTLS remains mandatory. An enrolled phone on cellular supplies the
end-to-end check with its normal credentials, after receiving the updated endpoint list on LAN.
Funnel is never enabled implicitly by `server`. If raw-TCP Funnel through the host VPN fails, **stay LAN-only and bring
options to the owner**; do not bypass the VPN, change routing, or publish 9443 as a workaround.
