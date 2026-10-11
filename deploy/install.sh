#!/usr/bin/env bash
# This installer itself is a trusted bootstrap; release.sh embeds the owner's pinned public key.
set -euo pipefail
umask 077
PUBLIC_KEY='@MINISIGN_PUBLIC_KEY@'
scriptdir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# Template: release.sh generates a standalone installer with both helpers inlined.
# @INLINE_OPERATIONS@
declare -F verify_release >/dev/null || { printf "use a generated dist/<version>/install.sh\n" >&2; exit 2; }
mode=${1:-}; [[ -n $mode ]] || { printf 'usage: install.sh server|desktop|upgrade|backup|restore|server-cert|enroll-host|funnel [bundle] [options]\n' >&2; exit 2; }; shift
bundle=''; if [[ $mode == restore ]]; then bundle=${1:?restore requires bundle}; shift; fi
source=$scriptdir
stackroot=${PORCHLIGHT_ROOT:-/var/lib/porchlight}
run_dir=${PORCHLIGHT_RUN_DIR:-/run/porchlight}
project=${PORCHLIGHT_PROJECT:-porchlight}
force=0 niri=0 funnel=0 no_backup=0 no_start=0 identity='' recipient='' password_file=''
admin_username=admin admin_name='' household=Porchlight zone=America/New_York
rollback=0 release_request='' release_repo='' source_given=0 repo_given=0
incoming_dir='' upgrade_pending=0 upgrade_migrating=0 upgrade_exposed=0 upgrade_rolling_back=0 old_version='' rollback_dump='' rollback_bundle='' dump_created=0 maintenance_owned=0
agent_gate_owned=0
ts_hostname='' tailscale_image='' ts_authkey_file='' no_authkey=0
san_args=()
enroll_host=''
api_health_timeout=30
while (($#)); do
  case $1 in
    --source) source=${2:?}; source_given=1; shift 2 ;;
    --release) release_request=${2:?}; shift 2 ;;
    --repo) release_repo=${2-}; repo_given=1; shift 2 ;;
    --root) stackroot=${2:?}; shift 2 ;;
    --run-dir) run_dir=${2:?}; shift 2 ;;
    --project) project=${2:?}; shift 2 ;;
    --host) enroll_host=${2:?}; shift 2 ;;
    --api-health-timeout) api_health_timeout=${2:?}; shift 2 ;;
    --force) force=1; shift ;;
    --niri) niri=1; shift ;;
    --no-funnel) funnel=0; shift ;;
    --no-start) no_start=1; shift ;; # restore/drill: leave API and backup stopped
    --san-dns) san_args+=(--san-dns "${2:?}"); shift 2 ;;
    --san-ip) san_args+=(--san-ip "${2:?}"); shift 2 ;;
    --no-backup) no_backup=1; shift ;; # isolated drill: compare counts before scheduler starts
    --funnel) funnel=1; shift ;;
    --ts-hostname) ts_hostname=${2:?}; shift 2 ;;
    --tailscale-image) tailscale_image=${2:?}; shift 2 ;;
    --ts-authkey-file) ts_authkey_file=${2:?}; shift 2 ;;
    --no-authkey) no_authkey=1; shift ;; # explicitly reuse an existing tailscaled identity
    --identity) identity=${2:?}; shift 2 ;;
    --backup-recipient) recipient=${2:?}; shift 2 ;;
    --admin-password-file) password_file=${2:?}; shift 2 ;;
    --admin-username) admin_username=${2:?}; shift 2 ;;
    --admin-name) admin_name=${2:?}; shift 2 ;;
    --household) household=${2:?}; shift 2 ;;
    --tz) zone=${2:?}; shift 2 ;;
    --rollback) rollback=1; shift ;;
    *) printf 'unknown option\n' >&2; exit 2 ;;
  esac
done
[[ $api_health_timeout =~ ^[1-9][0-9]{0,2}$ ]] && ((api_health_timeout <= 600)) || { printf 'API health timeout must be 1-600 seconds\n' >&2; exit 2; }
if ((funnel)); then printf 'Install LAN first, then use install.sh funnel (not server --funnel)\n' >&2; exit 2; fi
[[ $stackroot == /* && $stackroot != / && $run_dir == /* && $run_dir != / ]] || { printf 'absolute non-root paths required\n' >&2; exit 2; }
if [[ -n $release_request || $repo_given == 1 ]]; then
  [[ $mode == upgrade && -n $release_request && $source_given == 0 ]] || { printf 'use upgrade --release <V|latest> [--repo owner/repo], without --source\n' >&2; exit 2; }
  [[ $release_request == latest ]] || version_code "$release_request" >/dev/null || { printf 'invalid release version\n' >&2; exit 2; }
fi
# One lock covers installed-state reads, mutations, rollback and verified exec handoff.
if [[ $mode != desktop ]]; then
  if [[ $mode != server && $mode != restore && ! -d $stackroot ]]; then
    printf 'Existing installed stack required\n' >&2; exit 2
  fi
  mkdir -m 700 -p "$stackroot"
  [[ ! -L $stackroot/.installer.lock ]] || { printf 'installer lock must not be a symlink\n' >&2; exit 2; }
  if [[ ${PORCHLIGHT_INSTALL_LOCK_ROOT:-} != "$stackroot" || ! /proc/$$/fd/9 -ef $stackroot/.installer.lock ]]; then
    exec 9>>"$stackroot/.installer.lock"
  fi
  flock -n 9 || { printf 'installer already running for this stack\n' >&2; exit 2; }
  export PORCHLIGHT_INSTALL_LOCK_ROOT=$stackroot
  if [[ -e $run_dir/maintenance || -L $run_dir/maintenance ]]; then
    printf 'Stale maintenance fence: %s/maintenance. Writes remain disabled; inspect upgrade logs/recovery dump before removing it (see operations runbook).\n' "$run_dir" >&2; exit 2
  fi
fi
if [[ $mode == upgrade ]]; then
  [[ ! -e $stackroot/.provisioning ]] || { printf 'initial provisioning incomplete; retry the original server command\n' >&2; exit 2; }
  [[ -s $stackroot/.env && -L $stackroot/release-current && -f $stackroot/release-current/compose.yml ]] || { printf 'Existing installed stack required\n' >&2; exit 2; }
  old_version=$(ops_python env-value "$stackroot/.env" PORCHLIGHT_VERSION)
  version_code "$old_version" >/dev/null || { printf 'invalid installed version\n' >&2; exit 2; }
  if [[ -n $release_request ]]; then
    if (( ! repo_given )); then release_repo=$(ops_python env-value "$stackroot/.env" PORCHLIGHT_RELEASE_REPO) || release_repo=''; fi
    [[ -n $release_repo ]] || { printf 'set PORCHLIGHT_RELEASE_REPO in the stack .env or pass --repo owner/repo\n' >&2; exit 2; }
    ops_python release-repo "$release_repo" >/dev/null || { printf 'invalid release repository\n' >&2; exit 2; }
  fi
fi
# Validate new modes before even creating staging directories or contacting Docker.
if [[ $mode == funnel ]]; then
  [[ $ts_hostname =~ ^[a-z0-9-]{1,63}$ ]] || { printf '--ts-hostname must match ^[a-z0-9-]{1,63}$\n' >&2; exit 2; }
  [[ $tailscale_image =~ ^tailscale/tailscale@sha256:[0-9a-f]{64}$ ]] || { printf '--tailscale-image requires tailscale/tailscale@sha256:<64 lowercase hex>\n' >&2; exit 2; }
  command -v timeout >/dev/null || { printf 'Funnel requires the timeout utility\n' >&2; exit 2; }
  if ((no_authkey)) && [[ -n $ts_authkey_file ]]; then printf 'Choose --no-authkey or --ts-authkey-file, not both\n' >&2; exit 2; fi
  if ! python3 -I -B - "$stackroot" "$ts_authkey_file" "$no_authkey" >/dev/null 2>&1 <<'PY_VALIDATE'
import os,pathlib,stat,sys
root=pathlib.Path(sys.argv[1]);node=root/'tailscale/state/tailscaled.state'
for path in [root,root/'.env',root/'tailscale',root/'tailscale/state',root/'tailscale/serve.json',root/'tailscale/authkey',node]:
 if path.is_symlink(): raise ValueError('symlink state')
for path in [root/'tailscale',root/'tailscale/state']:
 if path.exists() and (not path.is_dir() or path.stat().st_uid!=os.getuid()): raise ValueError('owner-held state directory required')
for path in [root/'tailscale/serve.json',root/'tailscale/authkey',root/'.env',node]:
 if path.exists() and not path.is_file(): raise ValueError('regular file required')
key=pathlib.Path(sys.argv[2]) if sys.argv[2] else root/'tailscale/authkey'
use_key=sys.argv[3]!='1' and (bool(sys.argv[2]) or key.exists())
if use_key:
 fd=os.open(key,os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK)
 try:
  s=os.fstat(fd)
  assert stat.S_ISREG(s.st_mode) and stat.S_IMODE(s.st_mode)==0o600 and s.st_uid==os.getuid() and s.st_size>0
 finally: os.close(fd)
else:
 assert node.is_file() and node.stat().st_size>0
PY_VALIDATE
  then printf 'Funnel requires an auth key or existing node state, owner-held state directories, and an owner-held 0600 nonempty auth-key file (no symlinks)\n' >&2; exit 2; fi
fi
if [[ $mode == enroll-host ]]; then
  # Read only the installed port, never caller interpolation; reject before staging/Docker.
  if ! enroll_host=$(python3 -I -B - "$enroll_host" "$stackroot/.env" 2>/dev/null <<'PY_ENROLL_HOST'
import ipaddress,pathlib,re,sys
value=sys.argv[1];p=pathlib.Path(sys.argv[2])
assert not p.is_symlink() and p.is_file()
e=dict(line.split('=',1) for line in p.read_text().splitlines() if '=' in line)
assert value and value.count(':')<=1
host,sep,port=value.partition(':')
port=port if sep else e.get('ENROLL_PORT','8444')
assert re.fullmatch(r'[0-9]{1,5}',port) and 1<=int(port)<=65535
if re.fullmatch(r'[0-9.]+',host):
 host=str(ipaddress.IPv4Address(host))
else:
 assert len(host)<=253 and all(re.fullmatch(r'[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?',part) for part in host.split('.'))
 host=host.lower()
print(host+':'+str(int(port)))
PY_ENROLL_HOST
  ); then printf 'enroll-host requires --host <DNS name|IPv4>[:port], port 1-65535, and an installed regular .env (IPv6 is not supported)\n' >&2; exit 2; fi
  command -v timeout >/dev/null || { printf 'enroll-host requires the timeout utility\n' >&2; exit 2; }
fi
if [[ $mode == funnel || $mode == server-cert || $mode == enroll-host ]]; then
  [[ -s $stackroot/.env && -L $stackroot/release-current && -f $stackroot/release-current/compose.yml && -d $stackroot/data/pki ]] || { printf 'Existing installed stack required; install LAN first\n' >&2; exit 2; }
  if [[ $mode == funnel ]]; then
    [[ -f $stackroot/release-current/compose.funnel.yml ]] || { printf 'Installed release lacks compose.funnel.yml; upgrade first\n' >&2; exit 2; }
  elif [[ $mode == server-cert ]]; then
    ((${#san_args[@]})) || { printf 'server-cert requires the complete SAN set (--san-dns / --san-ip)\n' >&2; exit 2; }
  fi
fi
# Restore alone accepts explicit target network settings, persisted before any target Compose call.
restore_overrides=()
if [[ $mode == restore ]]; then
  for key in PG_PORT LAN_PORT ENROLL_PORT LAN_BIND ENROLL_CIDRS ENROLL_HOST PORCHLIGHT_ENDPOINTS BACKEND_SUBNET INGRESS_SUBNET; do
    if [[ -v $key ]]; then restore_overrides+=("$key" "${!key}"); fi
  done
fi
# Caller dev credentials must never override the installed/restored stack .env in Compose interpolation.
unset POSTGRES_PASSWORD PORCHLIGHT_MIGRATOR_PASSWORD PORCHLIGHT_APP_PASSWORD PORCHLIGHT_AGENT_PASSWORD PORCHLIGHT_BACKUP_PASSWORD PORCHLIGHT_VERSION
unset FUNNEL_ENABLED TS_HOSTNAME TAILSCALE_IMAGE_SHA256 TS_AUTHKEY_SPEC TS_AUTHKEY FUNNEL_IP
unset PORCHLIGHT_RELEASE_REPO PORCHLIGHT_RELEASE_PUBLIC_KEY PORCHLIGHT_UPGRADE_COMMAND PORCHLIGHT_RELEASE_DIR
[[ $mode == server ]] || unset PORCHLIGHT_ENDPOINTS
[[ $mode != enroll-host ]] || unset ENROLL_HOST ENROLL_PORT
export PORCHLIGHT_ROOT="$stackroot" PORCHLIGHT_RUN_DIR="$run_dir"
# Private staging belongs to runtime state even for desktop installs.
runtime=${XDG_STATE_HOME:-$HOME/.local/state}/porchlight
if [[ $mode != desktop ]]; then runtime=$stackroot/tmp; fi
mkdir -m 700 -p "$runtime"
stage=$(mktemp -d "$runtime/install.XXXXXX")
desktop_stage='' niri_pending='' desktop_autostart_pending='' api_stopped=0 funnel_pending=0 enroll_pending=0
install_error='' cert_error_log='' cert_rotated=0 endpoints_updated=0 funnel_fqdn='' api_recreate_failed=0
# Preserve the caller's diagnostic channel even when EXIT runs inside a logged upgrade step.
exec {install_stderr}>&2
# Even a dead SSH pipe/tty must not turn diagnostics into another failure.
say() { (trap '' PIPE; printf "$@" 2>/dev/null) >&"$install_stderr" || true; }
report() { (trap '' PIPE; "$@" 2>/dev/null || true); }
fail_install() { install_error=$1; return 1; }
cleanup_install() {
  trap '' INT TERM HUP PIPE
  local code=$1 disable_failed=0 remove_failed=0 recover_failed=0 key_retained=0 cert_error=''
  local enroll_restore_failed=0 enroll_backup_failed=0
  # Inherit ignored signals in cleanup children too; best effort (Go clients may re-arm them).
  # Cleanup waits are bounded; emit diagnostics only after restoration.
  set +e
  if ((upgrade_pending)); then
    code=1
    if rollback_upgrade > "$stage/rollback.log" 2>&1; then
      say 'ROLLED BACK to %s; logs: %s\n' "$old_version" "$stage"
    else
      say 'ROLLBACK FAILED; logs: %s\n' "$stage"
      report_rollback
    fi
  fi
  # Every handled exit releases the SQL login gate, after any destructive rollback has finished.
  if ! resume_agent > "$stage/agent-resume.log" 2>&1; then
    code=1
    say 'Could not restore porchlight_agent LOGIN; inspect PostgreSQL privately and restore LOGIN after recovery.\n'
  fi
  # Complete ALL state restoration before emitting anything to the caller's output channel.
  if ((enroll_pending)); then
    # Private snapshot restores even an absent key and atomically replaces the new inode.
    mv -f -- "$stage/env-before" "$stackroot/.env" > "$stage/enroll-restore.log" 2>&1 || enroll_restore_failed=1
    if (( ! enroll_restore_failed )); then
      dc_timeout=$api_health_timeout dc up -d --no-deps --force-recreate --wait --wait-timeout "$api_health_timeout" api > "$stage/enroll-api-recover.log" 2>&1 || recover_failed=1
      if (( ! no_backup )); then
        dc_timeout=120 dc up -d --no-deps --force-recreate --wait --wait-timeout 120 backup > "$stage/enroll-backup-recover.log" 2>&1 || enroll_backup_failed=1
      fi
    fi
  fi
  if ((funnel_pending)); then
    set_stack_env FUNNEL_ENABLED 0 TS_AUTHKEY_SPEC '' > "$stage/disable.log" 2>&1 || disable_failed=1
    # The forced override still selects the sidecar after disabling persisted activation.
    dc_force_funnel=1 dc_timeout=30 dc rm -sf tailscale > "$stage/tailscale-remove.log" 2>&1 || remove_failed=1
    [[ ! -f $stackroot/tailscale/authkey ]] || key_retained=1
  fi
  if ((api_stopped)); then
    dc_timeout=$api_health_timeout dc up -d --no-deps --wait --wait-timeout "$api_health_timeout" api > "$stage/api-recover.log" 2>&1 || recover_failed=1
  fi
  if [[ -n $niri_pending ]]; then
    { if [[ -e $stage/niri-before ]]; then cp -p "$stage/niri-before" "$niri_pending"; else rm -f -- "$niri_pending"; fi; } > "$stage/niri-recover.log" 2>&1
  fi
  if [[ -n $desktop_autostart_pending ]]; then
    { if [[ -e $stage/autostart-before ]]; then cp -p "$stage/autostart-before" "$desktop_autostart_pending"; else rm -f -- "$desktop_autostart_pending"; fi; } > "$stage/autostart-recover.log" 2>&1
  fi
  [[ -z $desktop_stage ]] || rm -rf -- "$desktop_stage"
  # Task 3's stderr contract excludes secrets. Save only that diagnostic, never Tailscale logs.
  if [[ -n $cert_error_log ]]; then cert_error=$(tail -20 "$cert_error_log" 2>/dev/null); fi
  if ((upgrade_exposed && code)); then
    # Never restore a pre-exposure snapshot over acknowledged writes.
    rm -f -- "$rollback_dump"
    if ((maintenance_owned)); then rm -f -- "$run_dir/maintenance" || say 'Could not remove maintenance fence; writes remain disabled\n'; fi
    say 'FORWARD RECOVERY: new API is exposed; keeping release %s and its database. Inspect private logs: %s\n' "$version" "$stage"
    report_aux_recovery_command
  elif (( ! upgrade_pending )); then
    if ((code)) && [[ $mode == funnel || $mode == server-cert || $mode == enroll-host ]]; then
      # Never preserve raw command output (Tailscale can include credentials/login URLs).
      # Keep fixed step names and outcome flags, privately, after ALL recovery attempts.
      local diagnostic step
      diagnostic=$(mktemp -d "$runtime/failure.XXXXXX")
      if [[ -n $diagnostic ]]; then
        {
          printf 'mode=%s\nstatus=%s\napi-recovery-failed=%s\nsidecar-remove-failed=%s\n' "$mode" "$code" "$recover_failed" "$remove_failed"
          for step in enable disable api-start api-recover tailscale-remove; do
            if [[ -e $stage/$step.log ]]; then printf 'attempted=%s\n' "$step"; fi
          done
        } > "$diagnostic/summary.txt"
        chmod 600 "$diagnostic/summary.txt"
        say 'Private sanitized failure diagnostics: %s\n' "$diagnostic"
      fi
    fi
    rm -rf -- "$stage" >/dev/null 2>&1
  fi
  if [[ -n $incoming_dir ]]; then say 'Incoming release retained: %s\n' "$incoming_dir"; fi
  [[ -z $install_error ]] || say '%s\n' "$install_error"
  [[ -z $cert_error ]] || say '%s\n' "$cert_error"
  if ((funnel_pending)); then
    if ((remove_failed)); then
      say 'ERROR: cannot confirm sidecar removal; manually remove tailscale using the Funnel Compose override before treating this stack as LAN-only.\n'
    elif (( ! disable_failed )); then
      say 'Funnel sidecar stopped and removed; leaving LAN-only (FUNNEL_ENABLED=0).\n'
    fi
    if ((disable_failed)); then say 'ERROR: could not persist Funnel disabled state; repair .env before upgrade/backup.\n'; fi
    if ((key_retained)); then
      say 'Staged auth key retained privately (0600); auth reference must remain disabled. Retry the same install.sh funnel command without --ts-authkey-file, or supply a new key if expired.\n'
    else
      say 'Node state retained. Retry the same install.sh funnel command with --no-authkey.\n'
    fi
    if ((cert_rotated)); then
      if ((endpoints_updated)); then
        say 'Certificate/endpoints already include %s; rerun the same install.sh funnel command with --no-authkey to complete.\n' "$funnel_fqdn"
      else
        say 'Certificate already includes %s; endpoints not updated; rerun the same install.sh funnel command with --no-authkey to complete.\n' "$funnel_fqdn"
      fi
    else
      say 'Certificate/endpoints unchanged by this activation.\n'
    fi
    say 'Check HTTPS, tag/nodeAttr and host VPN prerequisites. Inspect docker compose logs --tail 30 tailscale privately; never paste credentials or login URLs.\n'
  fi
  if ((enroll_pending)); then
    if ((enroll_restore_failed)); then
      say 'ERROR: could not restore previous ENROLL_HOST; repair .env before generating enrollment URIs.\n'
    elif ((recover_failed)); then
      say 'previous ENROLL_HOST restored; API recovery failed; retry API recreation.\n'
    else
      say 'previous ENROLL_HOST restored; API recovery succeeded.\n'
    fi
    if ((enroll_backup_failed)); then say 'Backup recovery failed; recreate backup to refresh its .env bind mount.\n'; fi
    if ((recover_failed || enroll_restore_failed)); then report_api_recovery_command; fi
  fi
  if [[ $mode == server-cert ]] && ((cert_rotated && api_recreate_failed)); then
    if ((recover_failed)); then
      say 'API not running or not healthy; API recovery failed. The certificate remains rotated.\n'
    else
      say 'API recovery succeeded; the certificate remains rotated (initial API recreate failed).\n'
    fi
    report_api_recovery_command
  elif [[ $mode != enroll-host ]] && ((recover_failed)); then
    say 'API recovery failed; start api with the installed Compose files\n'
  fi
  return "$code"
}
trap 'cleanup_install "$?"' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

check_docker() {
  if ! command -v docker >/dev/null || ! docker info >/dev/null 2>&1; then
    if [[ -e /etc/NIXOS ]]; then printf 'Enable Docker in NixOS: virtualisation.docker.enable = true;\n' >&2; fi
    printf 'A running Docker daemon and Compose plugin are required\n' >&2; return 1
  fi
  docker compose version >/dev/null
}
active_release() { readlink -f -- "$stackroot/release-current"; }
funnel_enabled() {
  [[ $(python3 -I -B - "$stackroot/.env" <<'PY_ENABLED'
import pathlib,sys
p=pathlib.Path(sys.argv[1]);values=dict(line.split('=',1) for line in p.read_text().splitlines() if '=' in line) if p.exists() else {}
print(values.get('FUNNEL_ENABLED','0'))
PY_ENABLED
) == 1 ]]
}
dc() {
  local release
  release=$(active_release)
  local -a files=(-f "$release/compose.yml") runner=()
  if funnel_enabled || [[ ${dc_force_funnel:-0} == 1 ]]; then files+=(-f "$release/compose.funnel.yml" --profile funnel); fi
  # A wedged Docker client must not extend the login budget by ignoring SIGTERM.
  if [[ -n ${dc_timeout:-} ]]; then runner=(timeout --foreground --signal=KILL "$dc_timeout"); fi
  # Persisted interpolation is authoritative on first install, retry and later upgrades alike.
  "${runner[@]}" env -u PORCHLIGHT_UID -u PORCHLIGHT_GID -u PG_PORT -u LAN_PORT -u ENROLL_PORT \
    -u LAN_BIND -u ENROLL_CIDRS -u ENROLL_HOST -u PORCHLIGHT_ENDPOINTS -u BACKEND_SUBNET \
    -u INGRESS_SUBNET -u PORCHLIGHT_MIN_CLIENT_VERSION docker compose -p "$project" --env-file "$stackroot/.env" "${files[@]}" "$@"
}
report_api_recovery_command() {
  # Paths/project are public, but may contain spaces; print a runnable, shell-quoted command.
  local release command
  release=$(active_release)
  local -a args=(env "PORCHLIGHT_ROOT=$stackroot" "PORCHLIGHT_RUN_DIR=$run_dir" docker compose -p "$project" --env-file "$stackroot/.env" -f "$release/compose.yml")
  if funnel_enabled; then args+=(-f "$release/compose.funnel.yml" --profile funnel); fi
  args+=(up -d --no-deps --force-recreate --wait --wait-timeout "$api_health_timeout" api)
  printf -v command '%q ' "${args[@]}"
  say 'Recovery command: %s\n' "${command% }"
}
report_aux_recovery_command() {
  local release command
  release=$(active_release)
  local -a args=(env "PORCHLIGHT_ROOT=$stackroot" "PORCHLIGHT_RUN_DIR=$run_dir" docker compose -p "$project" --env-file "$stackroot/.env" -f "$release/compose.yml")
  if funnel_enabled; then args+=(-f "$release/compose.funnel.yml" --profile funnel); fi
  if ((no_backup)) && ! funnel_enabled; then
    args+=(exec -T api /usr/local/bin/porchlight healthcheck)
  else
    args+=(up -d --no-deps --wait)
    if (( ! no_backup )); then args+=(backup); fi
    if funnel_enabled; then args+=(tailscale); fi
  fi
  printf -v command '%q ' "${args[@]}"
  say 'Fix the reported backup/sidecar error, then run (do not restore the old dump):\nRecovery command: %s\n' "${command% }"
}
set_stack_env() {
  # Preserve unrelated settings/secrets, replacing each installer-owned key exactly once.
  python3 -I -B - "$stackroot/.env" "$@" <<'PY_ENV'
import os,pathlib,sys,tempfile
p=pathlib.Path(sys.argv[1]);updates=dict(zip(sys.argv[2::2],sys.argv[3::2],strict=True))
current=p.read_text().splitlines()
if all([line for line in current if line.partition('=')[0]==key]==[key+'='+value] for key,value in updates.items()): sys.exit(0)
lines=[line for line in current if line.partition('=')[0] not in updates]
fd,name=tempfile.mkstemp(prefix='.env.',dir=p.parent)
try:
 with os.fdopen(fd,'w') as f:
  f.write('\n'.join(lines+[key+'='+value for key,value in updates.items()])+'\n');f.flush();os.fsync(f.fileno())
 os.replace(name,p)
finally:
 if os.path.exists(name): os.unlink(name)
PY_ENV
}
change_enroll_host() {
  check_docker
  if ! dc_timeout=30 dc run --rm --no-deps -T api server-cert --show --json > "$stage/current-sans.json" 2> "$stage/current-sans.log"; then
    cert_error_log=$stage/current-sans.log
    fail_install 'Cannot read current server SANs; ENROLL_HOST unchanged'; return 1
  fi
  if ! python3 -I -B - "$stage/current-sans.json" "${enroll_host%:*}" > "$stage/enroll-sans.log" 2>&1 <<'PY_ENROLL_SANS'
import ipaddress,json,sys
s=json.load(open(sys.argv[1]))['current'];host=sys.argv[2]
assert isinstance(s['dns'],list) and isinstance(s['ips'],list)
assert all(isinstance(name,str) for name in s['dns']+s['ips'])
try: ip=ipaddress.IPv4Address(host)
except ValueError: assert host.lower() in {name.lower() for name in s['dns']}
else: assert ip in {ipaddress.ip_address(value) for value in s['ips']}
PY_ENROLL_SANS
  then fail_install 'Enrollment host not in current server SANs (or invalid SAN metadata); use install.sh server-cert with the complete SAN set first; ENROLL_HOST unchanged'; return 1; fi
  if python3 -I -B - "$stackroot/.env" "$enroll_host" "$stackroot/.enroll-host-applied" <<'PY_ENROLL_UNCHANGED'
import pathlib,sys
lines=pathlib.Path(sys.argv[1]).read_text().splitlines();applied=pathlib.Path(sys.argv[3])
sys.exit(0 if applied.is_file() and applied.read_text()==sys.argv[2]+'\n' and [line for line in lines if line.partition('=')[0]=='ENROLL_HOST']==['ENROLL_HOST='+sys.argv[2]] else 1)
PY_ENROLL_UNCHANGED
  then report printf 'PASS enroll-host %s\n' "$enroll_host"; return; fi
  cp -p -- "$stackroot/.env" "$stage/env-before"
  enroll_pending=1
  rm -f -- "$stackroot/.enroll-host-applied"
  set_stack_env ENROLL_HOST "$enroll_host" > "$stage/enroll-env.log" 2>&1
  if ! dc_timeout=120 dc up -d --no-deps --force-recreate --wait --wait-timeout 120 api > "$stage/enroll-api.log" 2>&1; then
    fail_install 'enroll-host API recreate failed; restoring previous configuration'; return 1
  fi
  # Atomic .env replacement also requires refreshing the scheduler's file bind mount.
  if (( ! no_backup )); then
    if ! dc_timeout=120 dc up -d --no-deps --force-recreate --wait --wait-timeout 120 backup > "$stage/enroll-backup.log" 2>&1; then
      fail_install 'enroll-host backup recreate failed; restoring previous configuration'; return 1
    fi
  fi
  if (( ! no_backup )); then ops_python atomic-text "$stackroot/.enroll-host-applied" "$enroll_host"; fi
  enroll_pending=0
  report printf 'PASS enroll-host %s\n' "$enroll_host"
}
rotate_server_cert() {
  api_stopped=1
  dc stop api > "$stage/cert-stop.log" 2>&1
  if ! dc run --rm --no-deps -T api server-cert "${san_args[@]}" > "$stage/server-cert.out" 2> "$stage/server-cert.err"; then
    cert_error_log=$stage/server-cert.err
    fail_install 'server-cert failed; restoring LAN API service (certificate inputs must be the complete valid SAN set)'
  fi
  cert_rotated=1
  # Public metadata stays private until API recovery; no writes to a possibly closed pipe here.
}
resume_api() {
  # `restart` would retain old container environment; recreate to publish new endpoints.
  if ! dc_timeout=$api_health_timeout dc up -d --no-deps --force-recreate --wait --wait-timeout "$api_health_timeout" api > "$stage/api-start.log" 2>&1; then
    api_recreate_failed=1
    fail_install 'Certificate rotated; API recreate failed; attempting API recovery'
    return 1
  fi
  api_stopped=0
}
verify_source() {
  fetch_source "$source" "$stage/release"
  if [[ $mode == desktop ]]; then
    verify_release_subset "$stage/release" "$PUBLIC_KEY"
  else
    verify_release "$stage/release" "$PUBLIC_KEY"
  fi
  version=$(ops_python manifest-version "$stage/release/manifest.json")
}
check_upgrade_version() {
  local new_code old_code
  new_code=$(version_code "$version"); old_code=$(version_code "$old_version")
  if ((new_code < old_code)); then printf 'refusing downgrade\n' >&2; exit 2; fi
  if ((new_code == old_code)); then printf 'PASS already at %s\n' "$version"; exit 0; fi
}
fetch_release_asset() {
  ops_python fetch-release "$release_repo" "${PORCHLIGHT_RELEASE_BASE_URL:-}" "${PORCHLIGHT_INSTALL_TESTING:-0}" "$1" "$2" "$3"
}
upgrade_from_release() {
  fetch_release_asset "$release_request" manifest.json "$stage/manifest.json"
  fetch_release_asset "$release_request" manifest.json.minisig "$stage/manifest.json.minisig"
  verify_manifest "$stage" "$PUBLIC_KEY"
  version=$(ops_python manifest-version "$stage/manifest.json")
  [[ $release_request == latest || $release_request == "$version" ]] || { printf 'requested release version mismatch\n' >&2; exit 2; }
  check_upgrade_version
  [[ ! -L $stackroot/incoming ]] || { printf 'incoming directory must not be a symlink\n' >&2; return 1; }
  mkdir -m 700 -p "$stackroot/incoming"
  incoming_dir=$(mktemp -d "$stackroot/incoming/install.XXXXXX")
  mv "$stage/manifest.json" "$stage/manifest.json.minisig" "$incoming_dir/"
  fetch_release_asset "$version" "porchlight-$version-dist.tar" "$incoming_dir/dist.tar"
  ops_python extract "$incoming_dir/dist.tar" "$incoming_dir/release" "$incoming_dir/manifest.json"
  cmp -s "$incoming_dir/manifest.json" "$incoming_dir/release/manifest.json" || { printf 'downloaded manifest differs from verified manifest\n' >&2; return 1; }
  verify_release "$incoming_dir/release" "$PUBLIC_KEY"
  local -a args=(upgrade --source "$incoming_dir/release" --root "$stackroot" --run-dir "$run_dir" --project "$project")
  if ((no_backup)); then args+=(--no-backup); fi
  rm -rf -- "$stage"
  exec bash "$incoming_dir/release/install.sh" "${args[@]}"
}
upgrade_settings() {
  local repo status
  repo=$(ops_python env-value "$stackroot/.env" PORCHLIGHT_RELEASE_REPO) || {
    status=$?
    [[ $status == 3 ]] || return "$status"
    repo=$(ops_python release-source "$stage/release") || return
    if [[ -n $repo ]]; then set_stack_env PORCHLIGHT_RELEASE_REPO "$repo" || return; fi
  }
  set_stack_env PORCHLIGHT_UPGRADE_COMMAND "$stackroot/porchlight-upgrade" PORCHLIGHT_RELEASE_PUBLIC_KEY "$PUBLIC_KEY" || return
  {
    # shellcheck disable=SC2016 # The wrapper expands its own version argument at invocation.
    printf '#!/usr/bin/env bash\nexec bash %q upgrade --release "${1:-latest}"' "$stackroot/release-current/install.sh"
    printf ' --root %q --run-dir %q --project %q\n' "$stackroot" "$run_dir" "$project"
  } > "$stage/porchlight-upgrade" || return
  chmod 750 "$stage/porchlight-upgrade" || return
  mv -f -- "$stage/porchlight-upgrade" "$stackroot/porchlight-upgrade"
}
switch_release() {
  mkdir -p "$stackroot/releases"
  local target=$stackroot/releases/$version
  if [[ -e $target ]]; then
    verify_release "$target" "$PUBLIC_KEY"
    cmp -s "$stage/release/manifest.json" "$target/manifest.json" || { printf 'immutable release version differs\n' >&2; return 1; }
  else
    cp -a "$stage/release" "$target"
  fi
  # PostgreSQL's entrypoint reads its init files as the container's postgres user, whatever umask or
  # archive extraction created them with (GitHub tars carry no directory entries). The stack root stays 0700.
  [[ ! -d $target/postgres ]] || chmod -R u+rwX,go+rX,go-w "$target/postgres" || return
  ln -s "releases/$version" "$stackroot/.release-current.$$"
  mv -Tf "$stackroot/.release-current.$$" "$stackroot/release-current"
}
load_images() {
  docker load -i "$stage/release/porchlight-api-$version.tar" >/dev/null
  docker load -i "$stage/release/porchlight-backup-$version.tar" >/dev/null
}
set_version() { set_stack_env PORCHLIGHT_VERSION "${1:-$version}"; }
migrate() { dc up -d --wait postgres || return; dc up --no-deps --force-recreate --exit-code-from migrate migrate; }
quiesce_agent() {
  # Arm before ALTER so a signal or lost response still restores LOGIN in EXIT.
  agent_gate_owned=1
  dc_timeout=60 dc exec -T postgres psql -X -q -U postgres -d postgres -v ON_ERROR_STOP=1 \
    -c 'ALTER ROLE porchlight_agent NOLOGIN;' || return
  # NOLOGIN is committed before draining backends; wait for termination before either snapshot.
  # Select pids first: SQL fixes no AND order, and a terminate predicate is pushed below the role join onto
  # every backend (this session included). Re-read activity after clearing the transaction's cached snapshot.
  dc_timeout=60 dc exec -T postgres psql -X -q -U postgres -d postgres -v ON_ERROR_STOP=1 \
    -c "DO \$\$ DECLARE target integer; BEGIN
FOR target IN SELECT pid FROM pg_stat_activity WHERE usename = 'porchlight_agent' AND pid <> pg_backend_pid() LOOP
PERFORM pg_terminate_backend(target, 5000); END LOOP;
PERFORM pg_stat_clear_snapshot();
IF EXISTS (SELECT 1 FROM pg_stat_activity WHERE usename = 'porchlight_agent') THEN RAISE EXCEPTION 'agent sessions did not terminate'; END IF;
END \$\$;"
}
resume_agent() {
  if ((agent_gate_owned)); then
    dc_timeout=60 dc exec -T postgres psql -X -q -U postgres -d postgres -v ON_ERROR_STOP=1 \
      -c 'ALTER ROLE porchlight_agent LOGIN;' || return
    agent_gate_owned=0
  fi
}
restore_db() {
  dc up -d --wait postgres || return
  dc exec -T postgres dropdb -U postgres --if-exists --force porchlight || return
  # Shared bootstrap recreates database defaults and every role before dump GRANTs are applied.
  dc exec -T postgres sh /docker-entrypoint-initdb.d/10-porchlight.sh > "$stackroot/restore-bootstrap.log" 2>&1 || return
  dc exec -T postgres pg_restore -U postgres -d porchlight --exit-on-error < "$1"
}
report_rollback() {
  if [[ -n $rollback_bundle ]]; then
    say 'Rollback: use the newest verified installer restore %q --source <matching-release> --identity <laptop-age-identity> --root %q --run-dir %q --project %q --force\n' "$rollback_bundle" "$stackroot" "$run_dir" "$project"
  else
    say 'Pre-upgrade backup did not complete; inspect private logs and restore the last verified encrypted bundle using the newest verified installer, matching release artifacts and the owner age identity.\n'
  fi
}
rollback_upgrade() {
  upgrade_rolling_back=1
  dc stop api backup || return
  ln -s "releases/$old_version" "$stackroot/.release-current.rollback.$$" || return
  mv -Tf "$stackroot/.release-current.rollback.$$" "$stackroot/release-current" || return
  cp -p -- "$stage/env-before-upgrade" "$stage/env-rollback" || return
  mv -f -- "$stage/env-rollback" "$stackroot/.env" || return
  set_version "$old_version" || return
  if [[ -f $stage/upgrade-before ]]; then
    cp -p -- "$stage/upgrade-before" "$stage/upgrade-rollback" || return
    mv -f -- "$stage/upgrade-rollback" "$stackroot/porchlight-upgrade" || return
  else
    rm -f -- "$stackroot/porchlight-upgrade" || return
  fi
  if ((upgrade_migrating)); then restore_db "$rollback_dump" || return; fi
  # Do not re-run a failed new migrator through Compose's dependency graph.
  start --no-deps || return
  if ((maintenance_owned)); then rm -f -- "$run_dir/maintenance" || return; maintenance_owned=0; fi
  # The database is back on the dump; the pinned encrypted bundle stays the long-term recovery point.
  if ((dump_created)); then rm -f -- "$rollback_dump"; fi
}
prepare_backup_work() { mkdir -m 700 -p "$stackroot/backup-work"; chmod 700 "$stackroot/backup-work"; }
start() {
  prepare_backup_work || return
  dc up -d "$@" --wait api || return
  dc exec -T api /usr/local/bin/porchlight healthcheck || return
  if ((upgrade_pending && ! upgrade_rolling_back)); then
    # One simple command disarms restoration before ANY trap can remove the fence.
    # The new API has been read-only through both health checks; only now may it accept writes.
    upgrade_pending=0 upgrade_exposed=1
    resume_agent || return
    rm -f -- "$run_dir/maintenance" || return
    maintenance_owned=0
    rm -f -- "$rollback_dump" || return
  fi
  if (( ! no_backup )); then dc up -d "$@" --wait backup || return; fi
  if funnel_enabled; then dc up -d "$@" --wait tailscale || return; fi
}
backup_now() { prepare_backup_work; dc run --rm --no-deps -T backup now; }

enable_funnel() {
  # Docker must never create these bind sources as root. Recheck owner/type on opened directory fds.
  if ! python3 -I -B - "$stackroot" > "$stage/dirs.out" 2> "$stage/dirs.log" <<'PY_DIRS'
import os,sys
root=os.open(sys.argv[1],os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
try:
 for name in ('tailscale','state'):
  try: os.mkdir(name,0o700,dir_fd=root)
  except FileExistsError: pass
  fd=os.open(name,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=root)
  try:
   assert os.fstat(fd).st_uid==os.getuid(), 'owner-held state directory required'
   os.fchmod(fd,0o700)
  except BaseException:
   os.close(fd);raise
  os.close(root);root=fd
finally: os.close(root)
PY_DIRS
  then fail_install 'Funnel state directories must be owner-held real directories'; return 1; fi
  check_docker
  # Directory bind mounts let containerboot see atomic updates, and never create an absent key as a directory.
  # shellcheck disable=SC2016 # containerboot, not this shell, substitutes TS_CERT_DOMAIN.
  printf '%s' '{"TCP":{"443":{"TCPForward":"api:9443","ProxyProtocol":2}},"AllowFunnel":{"${TS_CERT_DOMAIN}:443":true}}' > "$stage/serve.json"
  chmod 600 "$stage/serve.json"
  mv -f "$stage/serve.json" "$stackroot/tailscale/serve.json"
  if [[ -n $ts_authkey_file ]]; then
    if ! python3 -I -B - "$ts_authkey_file" "$stackroot/tailscale/authkey" >/dev/null 2>&1 <<'PY_KEY'
import os,pathlib,stat,sys,tempfile
# Recheck on the opened fd, not just the earlier path validation (no FIFO/symlink races).
fd=os.open(sys.argv[1],os.O_RDONLY|os.O_NOFOLLOW|os.O_NONBLOCK)
with os.fdopen(fd,'rb') as src:
 s=os.fstat(src.fileno())
 assert stat.S_ISREG(s.st_mode) and stat.S_IMODE(s.st_mode)==0o600 and s.st_uid==os.getuid() and s.st_size>0
 p=pathlib.Path(sys.argv[2]);out,name=tempfile.mkstemp(prefix='.authkey.',dir=p.parent)
 try:
  with os.fdopen(out,'wb') as dst: dst.write(src.read());dst.flush();os.fsync(dst.fileno())
  os.replace(name,p)
 finally:
  if os.path.exists(name): os.unlink(name)
PY_KEY
    then fail_install 'Cannot copy private Tailscale auth-key file'; return 1; fi
  fi
  local authspec='' deadline remaining fqdn='' status_code serve_ready=0 cap_missing='' cap_deadline=0 cap_grace
  # The netmap's capabilities can lag BackendState=Running; tests shorten this grace (integer seconds, 0-120).
  cap_grace=${PORCHLIGHT_FUNNEL_CAP_GRACE_SECS:-20}
  [[ $cap_grace =~ ^[0-9]{1,3}$ ]] && (( cap_grace <= 120 )) || cap_grace=20
  if (( ! no_authkey )) && [[ -f $stackroot/tailscale/authkey ]]; then authspec=file:/config/authkey; fi
  # Arm cleanup before the write: a signal may be delivered while Python atomically enables .env.
  funnel_pending=1
  set_stack_env FUNNEL_ENABLED 1 TS_HOSTNAME "$ts_hostname" TAILSCALE_IMAGE_SHA256 "${tailscale_image##*@sha256:}" TS_AUTHKEY_SPEC "$authspec" > "$stage/enable.log" 2>&1
  # One bounded startup/login budget, including Docker exec hangs and image startup.
  deadline=$((SECONDS+120))
  if ! dc_timeout=120 dc up -d --wait --wait-timeout 120 tailscale > "$stage/tailscale-up.log" 2>&1; then
    fail_install 'Tailscale startup/login failed within the 120 s budget; endpoints/SANs unchanged'; return 1
  fi
  while (( (remaining=deadline-SECONDS) > 0 )); do
    if dc_timeout=$remaining dc exec -T tailscale tailscale status --json > "$stage/tailscale-status.json" 2> "$stage/tailscale-status.log"; then
      if fqdn=$(python3 -I -B - "$stage/tailscale-status.json" 2> "$stage/tailscale-parse.log" <<'PY_STATUS'
import json,re,sys
from urllib.parse import urlsplit,parse_qs
s=json.load(open(sys.argv[1]))
if s.get('BackendState')!='Running': sys.exit(3)
node=s['Self'];name=node['DNSName'].removesuffix('.')
assert len(name)<=253 and re.fullmatch(r'[a-z0-9-]+(?:\.[a-z0-9-]+)*\.ts\.net',name)
assert all(re.fullmatch(r'[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?',part) for part in name.split('.'))
# Names verified in upstream tailcfg/nodecap; LIVE-CONFIRM only the pinned image's JSON shape.
# ServeConfig alone can contain AllowFunnel even when the node is not allowed to use it.
try:
 capmap=node.get('CapMap');legacy=node.get('Capabilities')
 assert capmap is None or isinstance(capmap,dict)
 assert legacy is None or (isinstance(legacy,list) and all(isinstance(c,str) for c in legacy))
 caps=set(capmap) if capmap is not None else set(legacy or [])
except (AssertionError,TypeError,ValueError):
 sys.exit(4)
if 'https' not in caps: sys.exit(5)
if 'funnel' not in caps: sys.exit(6)
try:
 allowed=False
 for cap in caps:
  if not cap.startswith('https://tailscale.com/cap/funnel-ports'): continue
  url=urlsplit(cap)
  assert (url.scheme,url.netloc,url.path,url.fragment)==('https','tailscale.com','/cap/funnel-ports','')
  query=parse_qs(url.query,keep_blank_values=True,strict_parsing=True)
  assert set(query)=={'ports'} and len(query['ports'])==1
  for entry in query['ports'][0].split(','):
   port=re.fullmatch(r'([0-9]+)(?:-([0-9]+))?',entry)
   assert port
   low=int(port[1]);high=int(port[2] or port[1])
   assert 1 <= low <= high <= 65535
   allowed |= low <= 443 <= high
 assert allowed
except (AssertionError,TypeError,ValueError):
 sys.exit(7)
print(name)
PY_STATUS
      ); then break
      else
        status_code=$?
        case $status_code in
          4) fail_install 'Funnel capability data malformed after login; check Self.CapMap / Self.Capabilities'; return 1 ;;
          # Retry a capability gap for cap_grace seconds (within the login budget), then report it.
          5) cap_missing='Funnel https capability missing; enable HTTPS certificates in the tailnet' ;;
          6) cap_missing='Funnel funnel capability missing; add the funnel nodeAttr for tag:porchlight in the tailnet policy' ;;
          7) cap_missing='Funnel funnel-ports capability missing, malformed or excludes 443; check that the tailnet policy permits port 443' ;;
        esac
        if ((status_code == 5 || status_code == 6 || status_code == 7)); then
          ((cap_deadline)) || cap_deadline=$((SECONDS+cap_grace))
          if ((SECONDS >= cap_deadline)); then fail_install "$cap_missing"; return 1; fi
        elif ((status_code != 3)); then fail_install 'Tailscale returned an invalid Running status/FQDN; endpoint unchanged'; return 1; fi
      fi
    else
      status_code=$?
      if ((status_code == 124 || status_code == 137)); then break; fi
    fi
    (( SECONDS < deadline )) && sleep 1
  done
  if [[ -z $fqdn && -n $cap_missing ]]; then fail_install "$cap_missing"; return 1; fi
  if [[ -z $fqdn ]]; then fail_install 'Tailscale did not reach Running within 120 s; endpoints/SANs unchanged'; return 1; fi
  funnel_fqdn=$fqdn
  # Capability acceptance is separate from the applied forwarding configuration.
  while (( (remaining=deadline-SECONDS) > 0 )); do
    if dc_timeout=$remaining dc exec -T tailscale tailscale serve status --json > "$stage/tailscale-serve.json" 2> "$stage/tailscale-serve.log"; then
      if python3 -I -B - "$stage/tailscale-serve.json" "$fqdn" >/dev/null 2>&1 <<'PY_SERVE'
import json,sys
s=json.load(open(sys.argv[1]));tcp=s['TCP']['443']
assert tcp['TCPForward']=='api:9443' and tcp['ProxyProtocol']==2
assert s['AllowFunnel'][sys.argv[2]+':443'] is True
PY_SERVE
      then serve_ready=1; break; fi
    else
      status_code=$?
      if ((status_code == 124 || status_code == 137)); then break; fi
    fi
    (( SECONDS < deadline )) && sleep 1
  done
  if (( ! serve_ready )); then fail_install 'Tailscale did not apply raw TCP/PROXY v2 AllowFunnel within 120 s; check HTTPS and funnel nodeAttr; endpoints/SANs unchanged'; return 1; fi
  rm -f -- "$stackroot/tailscale/authkey"
  set_stack_env TS_AUTHKEY_SPEC ''
  if [[ -n $authspec ]]; then
    if ! dc_timeout=120 dc up -d --no-deps --force-recreate --wait --wait-timeout 120 tailscale > "$stage/tailscale-recreate.log" 2>&1; then
      fail_install 'Tailscale restart without auth key failed; saved node state retained for retry'; return 1
    fi
  fi
  if ! dc run --rm --no-deps -T api server-cert --show --json > "$stage/current-sans.json" 2> "$stage/current-sans.log"; then
    cert_error_log=$stage/current-sans.log
    fail_install 'Cannot read current server SANs; endpoint unchanged'; return 1
  fi
  if ! python3 -I -B - "$stage/current-sans.json" "$fqdn" > "$stage/funnel-sans" 2> "$stage/funnel-sans.log" <<'PY_SANS'
import ipaddress,json,re,sys
s=json.load(open(sys.argv[1]))['current']
assert isinstance(s['dns'],list) and isinstance(s['ips'],list)
for name in sorted(set(s['dns']+['porchlight.lan',sys.argv[2]])):
 assert len(name)<=253 and all(re.fullmatch(r'[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?',part) for part in name.split('.'))
 print('--san-dns');print(name)
for ip in sorted(set(s['ips'])):
 print('--san-ip');print(ipaddress.ip_address(ip))
PY_SANS
  then fail_install 'Invalid current SAN metadata; endpoint unchanged'; return 1; fi
  san_args=()
  while IFS= read -r kind && IFS= read -r host; do san_args+=("$kind" "$host"); done < "$stage/funnel-sans"
  if ! python3 -I -B - "$stage/current-sans.json" "$stackroot/.env" "$fqdn" <<'PY_UNCHANGED'
import json,pathlib,sys
s=json.load(open(sys.argv[1]))['current']
e=dict(line.split('=',1) for line in pathlib.Path(sys.argv[2]).read_text().splitlines() if '=' in line)
sys.exit(0 if {'porchlight.lan',sys.argv[3]}<=set(s['dns']) and e.get('PORCHLIGHT_ENDPOINTS')=='https://porchlight.lan:8443,https://'+sys.argv[3] else 1)
PY_UNCHANGED
  then
    rotate_server_cert
    set_stack_env PORCHLIGHT_ENDPOINTS "https://porchlight.lan:8443,https://$fqdn" > "$stage/endpoints.log" 2>&1
    endpoints_updated=1
    resume_api
  fi
  # The scheduler bind-mounts .env as a file; refresh its mount after atomic replacement
  # so nightly bundles retain Funnel configuration rather than the old LAN-only inode.
  if (( ! no_backup )); then
    dc up -d --no-deps --force-recreate --wait backup > "$stage/backup-recreate.log" 2>&1
  fi
  funnel_pending=0
  if ((cert_rotated)); then report cat "$stage/server-cert.out"; fi
  report printf 'PASS funnel %s\n' "$fqdn"
}

install_desktop() {
  local base=${XDG_DATA_HOME:-$HOME/.local/share}/porchlight
  mkdir -p "$base/releases"
  if ((rollback)); then
    [[ -L $base/previous ]] || { printf 'no previous desktop release\n' >&2; return 1; }
    local previous current
    previous=$(readlink "$base/previous"); current=$(readlink "$base/current")
    ln -s "$previous" "$base/.current.$$"; mv -Tf "$base/.current.$$" "$base/current"
    ln -s "$current" "$base/.previous.$$"; mv -Tf "$base/.previous.$$" "$base/previous"
    printf 'PASS desktop rollback\n'; return
  fi
  if [[ ! -L $base/current ]]; then
    printf 'Device key: Porchlight uses Secret Service (any provider: GNOME Keyring, KeePassXC, KWallet) when one runs in this user D-Bus session at enrollment; otherwise it keeps the key secret in an owner-only file (keep your disk encrypted). The choice is recorded; re-enroll to change it.\n'
  fi
  verify_source
  local same_version=0
  if [[ -f $base/current/release-manifest.json ]]; then
    local installed new_code old_code
    installed=$(ops_python manifest-version "$base/current/release-manifest.json")
    new_code=$(version_code "$version"); old_code=$(version_code "$installed")
    if ((new_code < old_code)); then printf 'refusing downgrade; use porchlight-update --rollback\n' >&2; exit 2; fi
    if ((new_code == old_code)); then
      cmp -s "$base/current/release-manifest.json" "$stage/release/manifest.json" || { printf 'desktop version already installed with different content\n' >&2; return 1; }
      same_version=1
    fi
  fi
  local datahome=${XDG_DATA_HOME:-$HOME/.local/share} confighome=${XDG_CONFIG_HOME:-$HOME/.config}
  local target=$base/releases/$version
  # Stage on the destination filesystem: the final rename can never become a partial copy.
  desktop_stage=$(mktemp -d "$base/releases/.install.XXXXXX")
  ops_python extract "$stage/release/porchlight-desktop-$version-linux-x86_64.tar.gz" "$desktop_stage/extracted" --desktop
  [[ -x $desktop_stage/extracted/porchlight/bin/porchlight ]] || { printf 'desktop app image launcher missing\n' >&2; return 1; }
  mv "$desktop_stage/extracted/porchlight" "$desktop_stage/app"
  rm -rf -- "$desktop_stage/extracted"
  cp "$stage/release/manifest.json" "$desktop_stage/release-manifest.json"
  cp "$stage/release/install.sh" "$desktop_stage/install.sh"
  cp -a "$stage/release/lib" "$desktop_stage/lib"
  if [[ -e $target ]]; then
    cmp -s "$target/release-manifest.json" "$stage/release/manifest.json" || { printf 'desktop version already installed with different content\n' >&2; return 1; }
    if ops_python same-tree "$desktop_stage" "$target" >/dev/null 2>&1; then
      rm -rf -- "$desktop_stage"; desktop_stage=''
      if ((same_version)); then printf 'PASS already installed %s\n' "$version"; return; fi
    else
      mv -- "$target" "$base/releases/.incomplete.$version.$$"
    fi
  fi
  if [[ -n $desktop_stage ]]; then mv -- "$desktop_stage" "$target"; desktop_stage=''; fi
  mkdir -p "$HOME/.local/bin" "$datahome/applications" "$confighome/autostart" "$datahome/icons/hicolor/scalable/apps"
  cp "$stage/release/porchlight.svg" "$datahome/icons/hicolor/scalable/apps/porchlight.svg"
  if [[ ! -L $base/current && ! -e $base/current ]]; then
    [[ ! -f $confighome/autostart/porchlight.desktop ]] || cp -p "$confighome/autostart/porchlight.desktop" "$stage/autostart-before"
    desktop_autostart_pending=$confighome/autostart/porchlight.desktop
  fi
  # Freedesktop Exec quoting (backslash, dollar, backtick and double quote), not shell interpolation.
  python3 -I -B - "$base" "$datahome" "$confighome" <<'PY'
import pathlib,sys
base,data,config=map(pathlib.Path,sys.argv[1:]);exe=str(base/'current/app/bin/porchlight')
exe=exe.replace('\\','\\\\').replace('"','\\"').replace('$','\\$').replace('`','\\`').replace('%','%%')
text='[Desktop Entry]\nType=Application\nName=Porchlight\nExec="'+exe+'"\nIcon=porchlight\nTerminal=false\nCategories=Office;\n'
(data/'applications/porchlight.desktop').write_text(text)
# The client owns XDG opt-in. Installer --niri is an explicit alternative, never a second mechanism.
PY
  cat > "$HOME/.local/bin/porchlight-update" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
base=${XDG_DATA_HOME:-$HOME/.local/share}/porchlight
if [[ ${1:-} == --rollback ]]; then exec bash "$base/current/install.sh" desktop --rollback; fi
exec bash "$base/current/install.sh" desktop --source "${1:?usage: porchlight-update source|--rollback}"
SH
  chmod 755 "$HOME/.local/bin/porchlight-update"
  if ((niri)); then
    if [[ -L $confighome/niri || -L $confighome/niri/config.kdl ]]; then
      printf 'Cannot edit niri config safely (symlink); use a regular config or omit --niri\n' >&2; return 1
    fi
    if ! mkdir -p "$confighome/niri"; then printf 'Cannot edit niri config; check the config directory\n' >&2; return 1; fi
    if [[ ! -L $base/current && ! -e $base/current ]]; then
      [[ ! -f $confighome/niri/config.kdl ]] || cp -p "$confighome/niri/config.kdl" "$stage/niri-before"
      niri_pending=$confighome/niri/config.kdl
    fi
    if ! python3 -I -B - "$confighome/niri/config.kdl" "$base/current/app/bin/porchlight" 2> "$stage/niri.log" <<'PY_NIRI'
import pathlib,sys,json,os,tempfile,shutil,re
p=pathlib.Path(sys.argv[1])
if p.is_symlink() or p.parent.is_symlink(): raise ValueError('symlink niri config')
text=p.read_text() if p.exists() else ''
line='spawn-at-startup '+json.dumps(sys.argv[2])+' "--tray"'
existing=[old for old in text.splitlines() if re.match(r'^\s*spawn-at-startup\s+', old) and re.search(r'/bin/[Pp]orchlight"(?:\s|$)', old)]
if any(json.dumps(sys.argv[2]) not in old for old in existing):
 print('Warning: niri spawn-at-startup names a different bin/porchlight path; update it manually (no duplicate added).',file=sys.stderr)
if not existing:
 backup=p.with_name(p.name+'.bak')
 if p.exists() and not backup.exists():
  fd=os.open(backup,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
  with os.fdopen(fd,'wb') as f: f.write(p.read_bytes());f.flush();os.fsync(f.fileno())
 fd,name=tempfile.mkstemp(prefix='.config.',dir=p.parent)
 try:
  with os.fdopen(fd,'w') as f:
   f.write(text+('' if not text or text.endswith('\n') else '\n')+line+'\n');f.flush();os.fsync(f.fileno())
  if p.exists(): shutil.copymode(p,name)
  os.replace(name,p)
 finally:
  if os.path.exists(name): os.unlink(name)
PY_NIRI
    then
      printf 'Cannot edit niri config safely (symlink or I/O error); use a regular config or omit --niri\n' >&2
      return 1
    fi
    # Successful Python execution emits only the fixed dedupe warning, never config text.
    if [[ -s $stage/niri.log ]]; then cat "$stage/niri.log" >&2; fi
  fi
  if [[ -L $base/current && $(readlink "$base/current") != "releases/$version" ]]; then
    ln -s "$(readlink "$base/current")" "$base/.previous.$$"; mv -Tf "$base/.previous.$$" "$base/previous"
  fi
  ln -s "releases/$version" "$base/.current.$$"; mv -Tf "$base/.current.$$" "$base/current"
  (( ! niri )) || rm -f -- "$confighome/autostart/porchlight.desktop"
  niri_pending='' desktop_autostart_pending=''
  printf 'PASS desktop installed %s\n' "$version"
}

case $mode in
  desktop) install_desktop ;;
  funnel) enable_funnel ;;
  enroll-host) change_enroll_host ;;
  server-cert)
    check_docker
    rotate_server_cert
    resume_api
    report cat "$stage/server-cert.out"
    report printf 'PASS server-cert\n'
    ;;
  server)
    check_docker
    if [[ ! -f $stackroot/.provisioning ]]; then
      [[ ! -e $stackroot/.env && ! -e $stackroot/data/pki ]] || { printf 'stack exists; use upgrade or restore\n' >&2; exit 2; }
      [[ -z $(docker ps -a -q --filter "label=com.docker.compose.project=$project") ]] || { printf 'Compose project already exists; choose a separate project/root\n' >&2; exit 2; }
    fi
    [[ $recipient == age1* ]] || { printf '--backup-recipient requires an age public recipient\n' >&2; exit 2; }
    verify_source
    [[ -n ${ENROLL_CIDRS:-} ]] || { printf 'set ENROLL_CIDRS to the actual LAN CIDR\n' >&2; exit 2; }
    if [[ -z $admin_name ]]; then read -r -p 'Super-admin display name: ' admin_name; fi
    if [[ -z $password_file ]]; then
      IFS= read -r -s -p 'Super-admin password: ' password; printf '\n'
      password_file=$stage/admin.pass; printf '%s\n' "$password" > "$password_file"; unset password
    fi
    [[ -s $password_file ]] || { printf 'password file required\n' >&2; exit 2; }
    mkdir -m 700 -p "$stackroot/data" "$stackroot/backups" "$stackroot/backup-work" "$run_dir"
    python3 -I -B - "$stage/initial.env" "$version" <<'PY'
import os,secrets,sys
fd=os.open(sys.argv[1],os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
with os.fdopen(fd,'w') as f:
 for key in ('POSTGRES_PASSWORD','PORCHLIGHT_MIGRATOR_PASSWORD','PORCHLIGHT_APP_PASSWORD','PORCHLIGHT_AGENT_PASSWORD','PORCHLIGHT_BACKUP_PASSWORD'): f.write(key+'='+secrets.token_hex(32)+'\n')
 for key,default in [('PORCHLIGHT_VERSION',sys.argv[2]),('PORCHLIGHT_UID',str(os.getuid())),('PORCHLIGHT_GID',str(os.getgid())),('PORCHLIGHT_MIN_CLIENT_VERSION','0.1.0'),('PG_PORT','5432'),('LAN_PORT','8443'),('ENROLL_PORT','8444'),('LAN_BIND','0.0.0.0'),('ENROLL_CIDRS',''),('ENROLL_HOST','porchlight.lan'),('PORCHLIGHT_ENDPOINTS','https://porchlight.lan:8443'),('BACKEND_SUBNET','172.29.250.0/24'),('INGRESS_SUBNET','172.29.251.0/24')]:
  value=os.environ.get(key,default)
  if any(c in value for c in '\r\n$#\"\''): raise ValueError('invalid configuration value')
  f.write(key+'='+value+'\n')
PY
    # Derive certificate names from operator endpoints unless explicit SANs were supplied.
    if ((${#san_args[@]} == 0)); then
      if ! python3 -I -B - > "$stage/sans" 2> "$stage/sans.log" <<'PY_SAN'
import ipaddress,os,urllib.parse
hosts={'porchlight.lan','localhost','127.0.0.1'}
for endpoint in os.environ.get('PORCHLIGHT_ENDPOINTS','https://porchlight.lan:8443').split(','):
 host=urllib.parse.urlsplit(endpoint).hostname
 if host: hosts.add(host)
host=urllib.parse.urlsplit('//'+os.environ.get('ENROLL_HOST','porchlight.lan')).hostname
if host: hosts.add(host)
for host in sorted(hosts):
 try: ipaddress.ip_address(host);kind='--san-ip'
 except ValueError: kind='--san-dns'
 print(kind);print(host)
PY_SAN
      then printf 'Cannot derive certificate SANs; check ENROLL_HOST/PORCHLIGHT_ENDPOINTS or supply explicit SANs\n' >&2; exit 2; fi
      while IFS= read -r kind && IFS= read -r host; do san_args+=("$kind" "$host"); done < "$stage/sans"
      ((${#san_args[@]})) || { printf 'No certificate SANs derived\n' >&2; exit 2; }
    fi
    ops_python validate-init "$password_file" "$admin_username" "$admin_name" "$household" "$zone" "$recipient" "${san_args[@]}" || { printf 'initial admin/password/household/timezone/recipient/network inputs rejected; correct inputs and retry server\n' >&2; exit 2; }
    if [[ -e $stackroot/.env ]]; then
      ops_python provision-env-check "$stage/initial.env" "$stackroot/.env" || { printf 'provisioning deployment inputs differ; retry the original settings\n' >&2; exit 2; }
    fi
    manifest_sha=$(sha256sum "$stage/release/manifest.json"); manifest_sha=${manifest_sha%% *}
    ops_python provision-marker "$stackroot/.provisioning" "$version" "$manifest_sha" "$project" "$run_dir" "$admin_username" "$admin_name" "$household" "$zone" "$recipient" "${san_args[@]}" || { printf 'provisioning inputs differ; retry the original server command/release\n' >&2; exit 2; }
    if [[ ! -e $stackroot/.env ]]; then ops_python atomic-copy "$stage/initial.env" "$stackroot/.env"; fi
    load_images; switch_release; upgrade_settings; migrate
    if [[ ! -f $stackroot/.provisioned-init ]]; then
      # Complete JSON may precede interrupted acknowledgement cleanup. The attempt lives
      # outside disposable staging so SIGKILL between output delivery and our copy is recoverable.
      delivered=''
      if [[ -f $stackroot/data/pki/ca.crt ]]; then
        for candidate in "$stackroot/init.pending.json" "$stackroot/init-attempt.json"; do
          if ops_python init-output-valid "$candidate" >/dev/null 2>&1; then delivered=$candidate; break; fi
        done
      fi
      if [[ -n $delivered ]]; then
        if [[ $delivered != "$stackroot/init.pending.json" ]]; then
          ops_python atomic-copy "$delivered" "$stackroot/init.pending.json"
        fi
        # Let the server reconcile publication and acknowledge its staged copy. Never
        # redirect this retry over our only delivery record: AlreadyInitialized emits none.
        init_status=0
        dc run --rm --no-deps -T api init --household "$household" --admin-username "$admin_username" \
          --admin-name "$admin_name" --tz "$zone" "${san_args[@]}" \
          --age-recipient "$recipient" --json < "$password_file" > /dev/null 2> "$stackroot/init.log" || init_status=$?
        if ((init_status == 2)); then
          # Exit 2 is the server's typed AlreadyInitialized result, including a crash
          # after staged-output removal. Verify committed PKI before accepting it.
          dc run --rm --no-deps -T api server-cert --show --json > "$stage/init-verified.json" 2> "$stackroot/init.log"
        elif ((init_status != 0)); then
          exit "$init_status"
        fi
        [[ ! -e $stackroot/data/.init-staging ]] || { printf 'Server initialization acknowledgement is incomplete; retry server\n' >&2; exit 1; }
      else
        # Never truncate a previous delivery record, even if this retry prints nothing.
        init_status=0
        dc run --rm --no-deps -T api init --household "$household" --admin-username "$admin_username" \
          --admin-name "$admin_name" --tz "$zone" "${san_args[@]}" \
          --age-recipient "$recipient" --json < "$password_file" > "$stackroot/init-attempt.json" 2> "$stackroot/init.log" || init_status=$?
        if ops_python init-output-valid "$stackroot/init-attempt.json" >/dev/null 2>&1; then
          ops_python atomic-copy "$stackroot/init-attempt.json" "$stackroot/init.pending.json"
        fi
        ((init_status == 0)) || exit "$init_status"
      fi
      ops_python init-output-valid "$stackroot/init.pending.json"
      ops_python atomic-copy "$stackroot/init.pending.json" "$stackroot/init.json"
      ops_python atomic-text "$stackroot/.provisioned-init" "$version"
    fi
    printf 'Initialization output (bootstrap token) saved privately: %s/init.json\n' "$stackroot"
    start
    rm -f -- "$stackroot/.provisioning" "$stackroot/.provisioned-init" "$stackroot/init.pending.json" "$stackroot/init-attempt.json"
    printf 'PASS server installed %s\n' "$version"
    ;;
  backup) check_docker; backup_now ;;
  upgrade)
    if [[ -n $release_request ]]; then upgrade_from_release; fi
    # Only our private download layout is eligible for automatic cleanup after handoff.
    incoming_parent=$(dirname "$(readlink -f -- "$source")")
    if [[ $(dirname "$incoming_parent") == "$(readlink -f -- "$stackroot")/incoming" && ${incoming_parent##*/} == install.* && ${source##*/} == release ]]; then
      incoming_dir=$incoming_parent
    fi
    verify_source; check_upgrade_version
    # Frozen bootstraps cannot gain this check; callers must still select the new trusted installer.
    cmp -s -- "${BASH_SOURCE[0]}" "$stage/release/install.sh" || {
      printf 'use the verified new release installer for upgrade --source; this bootstrap differs\n' >&2; exit 2;
    }
    check_docker
    rollback_dump=$stackroot/rollback/pre-upgrade-$old_version.dump
    # Only a failed rollback leaves this dump: it is the plain recovery point, so never stop services over it.
    [[ ! -e $rollback_dump ]] || { printf 'recovery dump from a failed rollback exists: %s; inspect it, then remove it\n' "$rollback_dump" >&2; exit 2; }
    cp -p -- "$stackroot/.env" "$stage/env-before-upgrade"
    if [[ -f $stackroot/porchlight-upgrade ]]; then cp -p -- "$stackroot/porchlight-upgrade" "$stage/upgrade-before"; fi
    # The new process must see the fence on its first request, before health polling.
    mkdir -m 700 -p "$run_dir"
    (set -o noclobber; : > "$run_dir/maintenance")
    maintenance_owned=1
    # Arm recovery before quiescing writers, including failures while making the backups.
    upgrade_pending=1
    dc stop backup api > "$stage/stop.log" 2>&1
    quiesce_agent > "$stage/agent-quiesce.log" 2>&1
    backup_now > "$stage/pre-upgrade.log" 2>&1
    rollback_bundle=$(python3 -I -B - "$stackroot/backups" "$stage/pre-upgrade.log" <<'PY'
import pathlib,re,sys
names=re.findall(r'^PASS backup (porchlight-[0-9]{8}T[0-9]{12}Z[.]tar[.]age)$',pathlib.Path(sys.argv[2]).read_text(),re.M)
assert len(names)==1
bundle=pathlib.Path(sys.argv[1])/names[0];assert bundle.is_file();print(bundle)
PY
)
    rollback_bundle=$(pin_rollback_bundle "$rollback_bundle")
    report printf 'PASS pre-upgrade backup pinned outside nightly retention\n'
    report_rollback
    [[ ! -L $stackroot/rollback ]] || { printf 'rollback directory must not be a symlink\n' >&2; exit 1; }
    mkdir -m 700 -p "$stackroot/rollback"
    chmod 700 "$stackroot/rollback"
    # Never overwrite an unexpected recovery point, even while holding the installer lock.
    (set -o noclobber; : > "$rollback_dump")
    dump_created=1
    chmod 600 "$rollback_dump"
    dc exec -T postgres pg_dump -U postgres -Fc porchlight > "$rollback_dump" 2> "$stage/dump.log"
    {
      load_images
      switch_release
      set_version
      upgrade_settings
      upgrade_migrating=1
      migrate
      start
    } > "$stage/upgrade.log" 2>&1
    upgrade_pending=0
    rm -f -- "$rollback_dump"
    if [[ -n $incoming_dir ]]; then rm -rf -- "$incoming_dir"; incoming_dir=''; fi
    report printf 'PASS upgrade %s\n' "$version"
    ;;
  restore)
    check_docker
    [[ -s $identity && -s $bundle ]] || { printf 'restore requires bundle and --identity\n' >&2; exit 2; }
    verify_source
    # Verify before stopping a stack or replacing any data; identity is consumed only by host age.
    age -d -i "$identity" -o "$stage/bundle.tar" "$bundle" 2> "$stage/age.log"
    ops_python extract "$stage/bundle.tar" "$stage/bundle"
    verify_hashes "$stage/bundle"
    for required in db.dump .env pki/ca.crt pki/ca.key secrets/idempotency.key backup-recipient.txt; do
      [[ -f $stage/bundle/$required ]] || { printf 'incomplete bundle\n' >&2; exit 2; }
    done
    bundle_version=$(python3 -I -B -c 'import json,sys;print(json.load(open(sys.argv[1]))["version"])' "$stage/bundle/manifest.json")
    [[ $bundle_version == "$version" ]] || { printf 'restore requires matching release; upgrade afterwards\n' >&2; exit 2; }
    # Deployment identity is target-local, never portable backup data. Owner-only paths stay private.
    target_uid=${PORCHLIGHT_UID:-$(id -u)} target_gid=${PORCHLIGHT_GID:-$(id -g)}
    if ! python3 -I -B - "$target_uid" "$target_gid" "$stackroot" "$run_dir" >/dev/null 2>&1 <<'PY_TARGET_ID'
import os,pathlib,sys
uid,gid=map(int,sys.argv[1:3]);root=pathlib.Path(sys.argv[3]);run=pathlib.Path(sys.argv[4])
assert uid==os.getuid() and gid in {os.getgid(),*os.getgroups()}
for path in (root,root/'data',root/'backups',root/'backup-work',run):
 assert not path.is_symlink()
 if path.exists(): assert path.is_dir() and path.stat().st_uid==uid and os.access(path,os.R_OK|os.W_OK|os.X_OK)
PY_TARGET_ID
    then printf 'target runtime identity cannot access private directories; run restore as the target owner, use its UID and a member GID, and repair directory ownership before retrying\n' >&2; exit 2; fi
    ops_python validate-restore-target "$stage/bundle/.env" "${restore_overrides[@]}" > "$stage/target-validation.log" 2>&1 || {
      printf 'invalid restore target network settings; correct overrides before retrying\n' >&2; exit 2;
    }
    running=$(docker ps -a -q --filter "label=com.docker.compose.project=$project")
    [[ ( -z $running && ! -e $stackroot/.env ) || $force == 1 ]] || { printf 'existing stack: use --force\n' >&2; exit 2; }
    if [[ -f $stackroot/.env && -L $stackroot/release-current ]]; then
      dc down
    elif [[ -n $running ]]; then
      printf 'existing project belongs to another/missing root; choose a separate project/root\n' >&2; exit 2
    fi
    mkdir -m 700 -p "$stackroot/data" "$stackroot/backups" "$stackroot/backup-work" "$run_dir"
    # Keep displaced state for forensic inspection; never silently merge old PKI/blob trees.
    if [[ -n $(ls -A "$stackroot/data") ]]; then mv "$stackroot/data" "$stackroot/data.before-restore.$(date -u +%Y%m%dT%H%M%S)"; mkdir -m 700 "$stackroot/data"; fi
    if [[ -e $stackroot/.env ]]; then cp -p "$stackroot/.env" "$stackroot/.env.before-restore.$(date -u +%Y%m%dT%H%M%S%N)"; fi
    cp "$stage/bundle/.env" "$stackroot/.env"; chmod 600 "$stackroot/.env"
    set_version # dev bundles may predate PORCHLIGHT_VERSION in their environment file
    set_stack_env PORCHLIGHT_UID "$target_uid" PORCHLIGHT_GID "$target_gid" "${restore_overrides[@]}"
    rm -f -- "$stackroot/.enroll-host-applied" "$stackroot/.provisioning" "$stackroot/.provisioned-init"
    # Only an enabled restored bundle warrants re-enable guidance, not the displaced stack.
    restore_funnel=0
    if funnel_enabled; then restore_funnel=1; fi
    # Backups do not carry node identity/config. Never let restored config opt into public ingress.
    set_stack_env FUNNEL_ENABLED 0 TS_AUTHKEY_SPEC ''
    if ((restore_funnel)); then
      python3 -I -B - "$stackroot" "$run_dir" "$project" > "$stage/reenable" 2> "$stage/reenable.log" <<'PY_REENABLE'
import pathlib,re,shlex,sys
root=pathlib.Path(sys.argv[1]);e=dict(line.split('=',1) for line in (root/'.env').read_text().splitlines() if '=' in line)
host=e.get('TS_HOSTNAME','');digest=e.get('TAILSCALE_IMAGE_SHA256','')
if not re.fullmatch(r'[a-z0-9-]{1,63}',host): host='<neutral-hostname>'
if not re.fullmatch(r'[0-9a-f]{64}',digest): digest='<verified-64-lowercase-hex>'
node=root/'tailscale/state/tailscaled.state'
reuse=not any(p.is_symlink() for p in (root/'tailscale',node.parent,node)) and node.is_file() and node.stat().st_size>0
args=['bash',str(root/'release-current/install.sh'),'funnel','--root',str(root),'--run-dir',sys.argv[2],'--project',sys.argv[3],
      '--ts-hostname',host,'--tailscale-image','tailscale/tailscale@sha256:'+digest]
args+=['--no-authkey'] if reuse else ['--ts-authkey-file','<new-owner-held-0600-auth-key-file>']
print(shlex.join(args))
PY_REENABLE
    fi
    for member in pki secrets backup-recipient.txt blobs; do cp -a "$stage/bundle/$member" "$stackroot/data/"; done
    load_images; switch_release
    restore_db "$stage/bundle/db.dump"
    migrate
    if ((no_start)); then
      dc exec -T postgres psql -X -q -U postgres -d porchlight -v ON_ERROR_STOP=1 \
        -c 'SELECT version, success FROM public._sqlx_migrations ORDER BY version;'
    else
      start
    fi
    report printf 'PASS restore %s\n' "$bundle_version"
    if ((restore_funnel)); then
      report printf 'Restore is LAN-only: FUNNEL_ENABLED=0; saved hostname, image and endpoints retained. Re-enable explicitly after checking the restored API:\n'
      report cat "$stage/reenable"
    fi
    ;;
  *) printf 'unknown install mode\n' >&2; exit 2 ;;
esac
