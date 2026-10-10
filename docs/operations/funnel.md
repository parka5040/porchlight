# M9: enable Tailscale Funnel after LAN installation

Only the owner/controller runs these live steps. Offline fixtures do **not** attest Funnel connectivity, host VPN
compatibility, or cellular acceptance. Upgrade the installed stack to a signed release containing both
`porchlight server-cert` and this installer/Compose override before proceeding; a newer standalone installer does
not replace `release-current`. Use the same root/run-dir/project for every command.

## Owner: Tailscale pre-registration

Complete these in the Tailscale admin console **before** generating or using an auth key:

- Enable MagicDNS and HTTPS certificates. Review the certificate-transparency disclosure warning first.
- Choose a randomized tailnet name and a neutral machine name (no person, household, or address in either).
  The installer hostname must match `^[a-z0-9-]{1,63}$` (for example, `quiet-node`).
- Define `tag:porchlight` and its `tagOwners` (only the owner/authorized admins). Add the `funnel` node attribute
  for that tag in the tailnet policy, e.g. `nodeAttrs: [{target: ["tag:porchlight"], attr: ["funnel"]}]`, using the
  console's policy syntax/validation. Do not grant Funnel globally just to make setup work.
- Generate a **single-use, pre-approved, tagged** auth key for `tag:porchlight` (not reusable, not ephemeral).
  Device key expiry is off by default for tagged nodes; confirm the node's setting after registration. This does
  not mean the unused auth key never expires.
- Save the auth key privately in a nonempty regular 0600 file owned by the installer user, e.g.
  `var/tailscale-authkey` on the laptop. No symlinks, shell literals, command arguments containing the key, debug
  tracing, screenshots or pasted logs. Stream it over SSH stdin to a 0600 server file when needed; only its **path**
  is an installer option. The original input file is caller-owned; remove both transport copies after success.

## Pin the image (controller, on the server)

Choose an explicit upstream version after reviewing its release and containerboot behaviour. Do not use a floating
`latest` tag, invent a digest, or use the image ID instead of a repository digest:

```bash
# Replace <version> with the reviewed Tailscale version, not Porchlight's version.
docker pull tailscale/tailscale:<version>
docker image inspect --format '{{index .RepoDigests 0}}' tailscale/tailscale:<version>
```

Record/verify the returned `tailscale/tailscale@sha256:<64 lowercase hex>` against the registry/upstream publication
for that chosen version (including its multi-architecture manifest if applicable). Pull that exact digest and
inspect it again to confirm it is available locally; this keeps image-download time out of the 120-second login
budget. Pass the **whole reference** to `--tailscale-image`; only the 64 hex characters enter the stack `.env`.
No Tailscale image is bundled in Porchlight's signed release; digest selection is an explicit owner/controller trust
step, not an installer download of a mutable tag. During live bring-up of that digest, confirm
`docker compose ... exec -T tailscale tailscale status --json` works (using both installed Compose files and the
Funnel profile). The override fixes `TS_SOCKET=/var/run/tailscale/tailscaled.sock` for containerboot and the CLI;
verify the healthcheck and status poll reach that socket before accepting the pin.

## Installer commands (on the server)

After the LAN stack is healthy, using its trusted generated installer:

```bash
umask 077
release=<data-mount>/porchlight/release-current
# image is the verified public repository digest above (not an auth key).
image=tailscale/tailscale@sha256:<verified-64-lowercase-hex>
bash "$release/install.sh" funnel \
  --root <data-mount>/porchlight --run-dir /run/porchlight --project porchlight \
  --ts-hostname quiet-node --tailscale-image "$image" \
  --ts-authkey-file <data-mount>/porchlight/tmp/m9-ts-authkey && \
  rm -f <data-mount>/porchlight/tmp/m9-ts-authkey
# && deletes the transport copy only after success; retain it privately on failure.
```

The examples' angle-bracket values are placeholders, not runnable shell values. The controller follow-up adds the
laptop wrappers `scripts/deploy-server.sh funnel --ts-hostname quiet-node --tailscale-image <ref>` and
`scripts/deploy-server.sh funnel-check`; use those after that follow-up is merged (including its private key-file
input handling). Until then, the direct signed installer interface above is the implemented path.

The installer validates inputs before modifying anything and refuses a missing LAN stack or a run with neither an
auth key nor existing nonempty `state/tailscaled.state`. It prepares private owner-held directories before any Compose
call. It starts the sidecar and uses one 120-second budget for `BackendState=Running` **and** application of the serve
config. `Self.DNSName` (trailing dot stripped) must match `^[a-z0-9-]+(\.[a-z0-9-]+)*\.ts\.net$` and valid DNS labels.
The status `Self` must also grant HTTPS and Funnel on 443; Running alone is not success. The capability names are
verified against upstream Tailscale `tailcfg/nodecap` and `ipn/serve`: `https`, bare `funnel`, and
`https://tailscale.com/cap/funnel-ports?ports=443,8443,10000` (the comma-separated port list may differ, but must include
443 directly or in an inclusive numeric range, e.g. `400-500`). The incorrect `https://tailscale.com/cap/funnel`
spelling is not accepted. **LIVE-CONFIRM only the JSON container shape** against the pinned image: `Self.CapMap`
keys are authoritative when non-null; missing/null `CapMap` falls back to the older `Self.Capabilities` string list.
Modern status may also include a deprecated-capabilities sentinel in that list; it is not a permission grant.
Missing data/grants or malformed containers/port queries fail closed before SAN/endpoint updates. Specific diagnostics
name missing `https` (enable HTTPS certificates), missing `funnel` (add the `funnel` nodeAttr for `tag:porchlight` in
tailnet policy), or missing/malformed/excluding-443 `funnel-ports` (check policy permits 443). Verified names do not
claim live compatibility with the chosen image's status schema.
Separately, `tailscale serve status --json` must show `TCP["443"]` forwarding to `api:9443` with `ProxyProtocol: 2` and
`AllowFunnel["<fqdn>:443"]: true`. A stored AllowFunnel configuration alone does not prove node permission.

Only then does it delete the staged key and, if a key reference was active, recreate the sidecar with an empty
reference. `TS_AUTH_ONCE=true` reuses saved state on future starts; an absent key is not bind-mounted or recreated as a
directory. It queries `porchlight server-cert --show --json`, reissues the **current SANs plus the FQDN**, then publishes
`https://porchlight.lan:8443,https://<fqdn>` and recreates API. If the FQDN is already in the SANs and endpoints already
match, certificate reissue and API restart are skipped. Every successful activation, **including an otherwise
idempotent rerun**, force-recreates the backup scheduler to refresh its `.env` bind (unless `--no-backup` was supplied).
Success prints `PASS funnel <fqdn>`; live acceptance below is still required.

Re-run the same command **without** `--ts-authkey-file` after first success; `--no-authkey` explicitly requires and
reuses existing node state. A keyless rerun does not force-recreate the sidecar or interrupt an already matching API.
A failed startup/login/status/serve check **stops and removes the sidecar**, persists `FUNNEL_ENABLED=0` and an empty
`TS_AUTHKEY_SPEC`, and leaves endpoints/SANs unchanged. The staged 0600 key, hostname and digest remain private for an
explicit immediate rerun of the same command without `--ts-authkey-file`; no background login keeps retrying. Use a
new single-use key if the retained key expired/revoked, or remove `$ROOT/tailscale/authkey` if abandoning the retry.
If removal fails (e.g. Docker is unreachable), the installer reports that it **cannot confirm LAN-only**; manually
remove the sidecar with both Compose files/profile before considering ingress withdrawn. Later activation failures
also disable/remove the sidecar; a failed certificate update keeps the endpoint unchanged and attempts to restart API.
After a successful rotation, a later failure reports whether only the certificate or both certificate/endpoints already
include the FQDN; saved node state supports rerunning the same `funnel` command with `--no-authkey` to complete.
Cleanup is armed **before** the enabling `.env` write and performs state restoration before diagnostics, even if SSH
output has closed. It ignores HUP/INT/TERM during cleanup, best effort, including in child processes. Cleanup may take
up to **60 seconds for Docker operations** (30 seconds for sidecar removal and 30 for API recovery), plus local state
writes. Real Go Docker/Compose clients may re-arm inherited signal handlers; a second terminal signal can still
interrupt them. At live bring-up, confirm removal/recovery after a double Ctrl-C; inspect state privately if output
was lost rather than assuming LAN-only. Public certificate output is printed only after API resumes; a lost output
channel must not strand a stopped LAN API or armed auth reference.
Investigate `docker compose ... logs --tail 30 tailscale` privately; do not paste login URLs, `.env`, container
environments, PKI, or tailscaled state. Do not work around the host VPN; bring options to the owner.

For manual certificate replacement (complete SAN set, not an append operation):

```bash
bash <data-mount>/porchlight/release-current/install.sh server-cert \
  --root <data-mount>/porchlight --run-dir /run/porchlight --project porchlight \
  --san-dns porchlight.lan --san-dns localhost --san-dns <tailscale-fqdn> \
  --san-ip 127.0.0.1 --san-ip <server-lan-ip>
```

This stops API, runs `dc run --rm --no-deps -T api server-cert ...`, then recreates API; on failure it attempts to
restart API before returning nonzero. Public previous/current SANs and expiry metadata are printed on success;
safe subcommand stderr is surfaced on failure. If rotation succeeds but API recreation fails, it explicitly reports
that the certificate **remains rotated**, returns nonzero, attempts bounded API recovery, and reports whether recovery
succeeded or API is not confirmed running/healthy. It prints an exact shell-quoted `Recovery command:` using the
installed root/run-dir/project/Compose files (including the Funnel override if enabled). Run that command if recovery
failed, then verify API; do not blindly rotate again or assume the old certificate was restored. Include every
still-needed SAN. The server always retains `porchlight.lan`,
reuses its key and household CA, and retains the new SANs through automatic renewal. This mode does not update the
endpoint list. Device re-enrollment is not needed merely to add the Funnel server SAN.

## Persistent state and Compose

- `$ROOT/tailscale/` and `state/`: 0700, owned by the invoking installer user. Root-owned or symlinked directories
  are refused before any change, as is a symlinked stack root for `funnel`. The opened directory fds are rechecked;
  a failed recheck reports a clean error before Compose. **Docker must never create these directories**: both binds disable host-path
  creation. Run `install.sh funnel` to prepare them, not a direct Compose up on a fresh root. `serve.json`: 0600;
  `authkey`: 0600, present only until login/serve acceptance succeeds (or a disabled explicit retry is pending).
- Directory `$ROOT/tailscale` is mounted read-only at `/config`, `state/` separately writable at `/var/lib/tailscale`.
  During an explicit login attempt, `TS_AUTHKEY_SPEC=file:/config/authkey`; after success or failure it is empty,
  even if the staged key is retained for retry. This directory mount
  avoids a permanently required secret-file mount and lets containerboot observe atomic serve-config replacements.
- `serve.json` is exactly
  `{"TCP":{"443":{"TCPForward":"api:9443","ProxyProtocol":2}},"AllowFunnel":{"${TS_CERT_DOMAIN}:443":true}}`.
  Containerboot substitutes the node FQDN. There is no HTTPS termination or reverse HTTP proxy in the sidecar:
  household-CA TLS terminates in API. Do not configure `HTTPS` or `TerminateTLS` here.
- `.env` gains `FUNNEL_ENABLED=1`, `TS_HOSTNAME`, `TAILSCALE_IMAGE_SHA256`, `TS_AUTHKEY_SPEC`, and the updated
  `PORCHLIGHT_ENDPOINTS`. It never contains key bytes. Ambient caller Funnel variables do not override it.
- Every installer Compose call adds `-f compose.funnel.yml --profile funnel` iff the installed `.env` has
  `FUNNEL_ENABLED=1`. Upgrade/start keep an enabled sidecar; backup uses the same configuration. The sidecar has no
  host ports, uses `TS_USERSPACE=true`, advertises only `tag:porchlight`, and retains ingress IP
  `${FUNNEL_IP:-172.29.251.10}`; API trusts that peer alone for PROXY v2. Do not publish port 9443.
- The sidecar sets `security_opt: ["no-new-privileges:true"]`. Its user and capabilities are unchanged for now.
  A reduced `cap_drop` set (and later an unprivileged user) is a **live-bring-up experiment**, not an offline-attested
  setting: test containerboot, socket access, key reads and persistent-state writes against the pinned image first.
- Household backups include `.env` and household PKI, **not** the separate Tailscale node identity. Every restore
  (including `--no-start`) explicitly sets `FUNNEL_ENABLED=0` and clears the auth reference; it returns **LAN-only**,
  retaining the restored hostname, digest and endpoints. Thus existing endpoint lists can still include an inactive
  Funnel URL until explicitly re-enabled; LAN remains available. The pre-restore sidecar is removed using the old
  configuration. This applies in both directions, including a pre-Funnel bundle restored onto an enabled stack.
- Only when the **restored bundle** had `FUNNEL_ENABLED=1`, restore prints the exact re-enable command **after
  `PASS restore`**, with `--no-authkey` when node state survives on that root, or with a **new** owner-held key on a new
  root. Failed restores and disabled/pre-Funnel bundles get no re-enable block, even if the displaced stack was enabled.
  A same-root restore retains identity; a fresh-root restore cannot start
  Tailscale or let Docker create root-owned directories. Check the restored API, then explicitly run `funnel` to
  provision/validate sidecar config and handle any changed FQDN. Never copy an identity to two running hosts;
  stop/quiesce the old node first.

## Live acceptance and VPN rule

After `PASS`, the controller must verify the following, without printing secrets:

1. Repeat activation without the auth-key option; restart the stack/host and confirm the same FQDN and identity.
   Check key absence, cleared reference, private modes, Running state, current SANs, and both endpoints. Inspect only
   specific public status fields, never a rendered full production Compose config or environment. **LIVE-CONFIRM**
   only the JSON capability container shape above; capability names are verified upstream. With the owner's approval, remove the `funnel` nodeAttr for `tag:porchlight`,
   wait for the status capability to disappear, and confirm `install.sh funnel --no-authkey` refuses and disables/removes
   the sidecar despite stored AllowFunnel config. Restore the nodeAttr and explicitly re-enable before continuing.
2. Obtain only the **public** `data/pki/ca.crt` over the trusted SSH connection. Per controller ruling **M9-R5**, the
   follow-up's laptop `funnel-check` performs a TLS handshake to `https://<fqdn>:443`, sending SNI for that FQDN,
   verifying the server chain against the household CA and the SAN against the exact FQDN. The server's subsequent
   **certificate-required alert is expected**, because this probe deliberately has no client certificate. A verified
   server certificate followed by that alert proves the public TLS path, not a completed authenticated handshake or
   application request. Fail on chain/name errors, timeouts, or any unexpected alert; never use `-k` or weaken mTLS.
   There is no unauthenticated health/version HTTP route, and a CA-only health curl is not the probe. For example,
   after copying only the CA certificate to `var/m9-funnel/ca.crt` on the laptop and setting `fqdn` to the public name:

   ```bash
   # Laptop only; log contains public TLS diagnostics, never device keys or credentials.
   timeout --signal=KILL 20 openssl s_client -connect "${fqdn}:443" -servername "$fqdn" \
     -tls1_3 -verify_hostname "$fqdn" -verify_return_error -CAfile "$PWD/var/m9-funnel/ca.crt" \
     -no-CApath -no-CAstore -brief -ign_eof </dev/null >var/m9-funnel/tls-check.log 2>&1
   ```

   This normally exits nonzero for the expected alert: require both `Verification: OK` and
   `tlsv13 alert certificate required` (alert 116) in the diagnostics, not merely any nonzero exit. The follow-up
   wrapper automates this interpretation. An enrolled device then exercises `/v1/connection` with its normal session
   and mTLS credentials to verify `transport=funnel` and the forwarded source address. Never export device private
   keys just for a probe.
3. Confirm on real enrolled phones over cellular: CRUD, SSE reconnect/update, attachments, notifications and
   revocation; enrollment remains LAN-only. Record human attestations in `server-acceptance.md` via the controller
   follow-up. Refresh clients on LAN so they receive the new endpoint list before moving to cellular.
4. Owner checks crt.sh for the public machine/tailnet name (certificate-transparency metadata can be public even
   though raw TCP TLS uses the private household CA). Only the intended neutral names may appear; do not claim a
   private Funnel name or invisible metadata.
5. Exercise a VPN relay change (if the server uses one), server restart, and backup → laptop restore. If raw-TCP Funnel through the host VPN
   fails, **stay LAN-only and bring options to the owner**. Do not route around the VPN, disable it, loosen enrollment
   CIDRs, switch to public TLS termination, or expose 9443 as a workaround. To withdraw a partially enabled Funnel,
   stop its sidecar with both Compose files/profile, set the stack's `FUNNEL_ENABLED=0`, clear `TS_AUTHKEY_SPEC` and
   reset `PORCHLIGHT_ENDPOINTS=https://porchlight.lan:8443` privately, remove any staged auth key, then recreate API
   and backup using the LAN Compose file. Preserve node state for an owner-approved retry; unused extra certificate
   SANs do not expose a listener. This is an explicit controller recovery step, not an automatic routing change.
