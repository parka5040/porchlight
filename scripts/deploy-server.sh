#!/usr/bin/env bash
# Laptop controller for the fresh production stack on the home server. Never transfers laptop PKI or data.
set -euo pipefail
set +x
umask 077
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
# Machine-specific values come from the environment or the gitignored .env (see .env.example); none default here.
. "$repo/scripts/local-env.sh"
porchlight_local_env "$repo"
HOST=${PORCHLIGHT_DEPLOY_HOST:-}
DATA_MOUNT=${PORCHLIGHT_DEPLOY_DATA_MOUNT:-}
ROOT=$DATA_MOUNT/porchlight
RUN_DIR=/run/porchlight
PROJECT=porchlight
DIST_DIR=$DATA_MOUNT/porchlight-dist
DRY_RUN=${PORCHLIGHT_DEPLOY_DRY_RUN:-0}

usage() {
  printf 'usage: scripts/deploy-server.sh preflight|install <version>|upgrade <version>|status|enroll-host <host>|enroll-uri|backup|funnel|funnel-check [options]\n' >&2
  printf 'funnel: --ts-hostname <name> --tailscale-image tailscale/tailscale@sha256:<64 lowercase hex> [--ts-authkey-file <laptop 0600 file> | --no-authkey]\n' >&2
  printf 'funnel-check: [--fqdn <name>] (default var/m9/funnel-fqdn)\n' >&2
  printf 'install: [--admin-name <display name>] [--admin-password-file <laptop 0600 file>] [--extra-san-dns <fqdn>]...\n' >&2
  printf 'server host, data mount and LAN address: PORCHLIGHT_DEPLOY_* in .env (see .env.example)\n' >&2
  printf 'enroll-uri: --device <name> [--user <username>] [--out var/m9/enroll-uri.json] [--token-file <server 0600 path>]\n' >&2
  exit 2
}
die() { printf '%s\n' "$2" >&2; exit "$1"; }
# ssh concatenates argv into a shell command: quote every word, including whole bash programs.
quote_command() {
  local word escaped="'\\''"
  for word in "$@"; do printf "'%s' " "${word//\'/$escaped}"; done
}
remote() {
  local cmd; cmd=$(quote_command "$@")
  if [[ $DRY_RUN == 1 ]]; then printf 'DRY-RUN ssh -T -o BatchMode=yes %s %s\n' "$HOST" "$cmd"; return; fi
  ssh -T -o BatchMode=yes "$HOST" "$cmd"
}
remote_interactive() {
  local cmd; cmd=$(quote_command "$@")
  if [[ $DRY_RUN == 1 ]]; then printf 'DRY-RUN ssh -t %s %s\n' "$HOST" "$cmd"; return; fi
  ssh -t "$HOST" "$cmd"
}
[[ -n $HOST ]] || die 2 'deployment host needed: set PORCHLIGHT_DEPLOY_HOST in .env (see .env.example)'
[[ $HOST =~ ^[a-zA-Z0-9_][a-zA-Z0-9_.@-]*$ ]] || die 2 'invalid deployment host'
[[ -n $DATA_MOUNT ]] || die 2 'server data mount needed: set PORCHLIGHT_DEPLOY_DATA_MOUNT in .env (see .env.example)'
[[ $DATA_MOUNT =~ ^(/[a-zA-Z0-9_][a-zA-Z0-9_.-]*)+$ ]] || die 2 'PORCHLIGHT_DEPLOY_DATA_MOUNT must be an absolute path without trailing slash'
[[ $DRY_RUN == 0 || $DRY_RUN == 1 ]] || die 2 'PORCHLIGHT_DEPLOY_DRY_RUN must be 0 or 1'
command=${1:-}; [[ -n $command ]] || usage; shift
version='' enroll_host=''
case $command in
  install|upgrade)
    version=${1:-}; [[ $version =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]?)\.(0|[1-9][0-9]?)$ ]] || die 2 'version must be MAJOR.MINOR.PATCH (minor/patch 0-99)'
    shift ;;
  enroll-host)
    [[ $# == 1 && -n $1 && $1 != -* ]] || usage
    enroll_host=$1; shift ;;
  preflight|status|enroll-uri|backup|funnel|funnel-check) ;;
  *) usage ;;
esac
admin_name='' password_file='' device='' username=admin out=var/m9/enroll-uri.json token_file=''
extra_sans=()
ts_hostname='' tailscale_image='' ts_authkey_file=var/tailscale-authkey no_authkey=0 explicit_authkey=0 fqdn=''
while (($#)); do
  if [[ $command:$1 == funnel:--no-authkey ]]; then no_authkey=1; shift; continue; fi
  [[ $# -ge 2 && -n $2 ]] || usage
  case $command:$1 in
    install:--admin-name) admin_name=$2 ;;
    install:--admin-password-file) password_file=$2 ;;
    install:--extra-san-dns)
      [[ $2 =~ ^[a-zA-Z0-9][a-zA-Z0-9.-]*[a-zA-Z0-9]$ && $2 != *..* ]] || die 2 'invalid extra DNS SAN'
      extra_sans+=(--san-dns "$2") ;;
    funnel:--ts-hostname) ts_hostname=$2 ;;
    funnel:--tailscale-image) tailscale_image=$2 ;;
    funnel:--ts-authkey-file) ts_authkey_file=$2; explicit_authkey=1 ;;
    funnel-check:--fqdn) fqdn=$2 ;;
    enroll-uri:--device) device=$2 ;;
    enroll-uri:--user) username=$2 ;;
    enroll-uri:--out) out=$2 ;;
    enroll-uri:--token-file) token_file=$2; [[ $token_file == /* ]] || die 2 'server token file must be absolute' ;;
    *) usage ;;
  esac
  shift 2
done

require_release() {
  local file
  for file in install.sh manifest.json manifest.json.minisig; do
    [[ -f $repo/dist/$version/$file && ! -L $repo/dist/$version/$file ]] || die 2 "required release file missing: $repo/dist/$version/$file"
  done
}
push_release() {
  local version=$1 destination=$DIST_DIR/$1 hashes local_hash remote_hash file
  require_release
  remote mkdir -p -m 0750 -- "$DIST_DIR" "$destination" < /dev/null
  if [[ $DRY_RUN == 1 ]]; then
    printf 'DRY-RUN rsync -a --protect-args --delete -e "ssh -o BatchMode=yes" %s/ %s:%s/\n' "$repo/dist/$version" "$HOST" "$destination"
    remote bash -c "# release-hashes
cd $(quote_command "$destination") && sha256sum install.sh manifest.json" < /dev/null
    return
  fi
  rsync -a --protect-args --delete -e 'ssh -o BatchMode=yes' "$repo/dist/$version/" "$HOST:$destination/" < /dev/null
  hashes=$(remote bash -c "# release-hashes
cd $(quote_command "$destination") && sha256sum install.sh manifest.json" < /dev/null) || die 1 'remote release hash check failed'
  for file in install.sh manifest.json; do
    local_hash=$(sha256sum "$repo/dist/$version/$file"); local_hash=${local_hash%% *}
    remote_hash=$(printf '%s\n' "$hashes" | python3 -I -c 'import sys; rows=[s.split() for s in sys.stdin]; matches=[r[0] for r in rows if len(r)==2 and r[1]==sys.argv[1]]; print(matches[0] if len(matches)==1 else "invalid")' "$file")
    [[ $local_hash == "$remote_hash" ]] || die 1 "remote release hash mismatch: $file"
  done
}

preflight() {
  # One remote read-only program, configured paths, no stack .env or inspection of container secrets.
  # The SSH login user runs the stack, so ownership is checked against it rather than a configured name.
  local script
  script="# preflight
$(quote_command declare "mount=$DATA_MOUNT" "root=$ROOT" "dist_dir=$DIST_DIR")
"
  script+=$(cat <<'REMOTE'
set -uo pipefail
failed=0
check() { local name=$1; shift; if "$@"; then printf 'PASS %s\n' "$name"; else printf 'FAIL %s\n' "$name"; failed=1; fi; }
member() { getent group porchlight >/dev/null && [[ " $(id -nG) " == *' porchlight '* ]]; }
private_dir() { [[ -d $1 && $(stat -c '%U:%G:%a' "$1") == "$(id -un):porchlight:750" ]]; }
python_ok() { command -v python3 >/dev/null && python3 -I -c 'import sys;sys.exit(sys.version_info < (3,12))'; }
mount_order() {
  local mounts
  mounts=$(systemctl show docker -p RequiresMountsFor) || return 1
  [[ " ${mounts#RequiresMountsFor=} " == *" $mount "* ]]
}
check nixos test -e /etc/NIXOS
check group member
check run-dir private_dir /run/porchlight
check root-dir private_dir "$root"
dist_ready() {
  local dist=$dist_dir
  if [[ -e $dist || -L $dist ]]; then
    [[ -d $dist && -w $dist && $(stat -c %u "$dist") == "$(id -u)" ]]
  else
    [[ -d ${dist%/*} && -w ${dist%/*} ]]
  fi
}
check dist-dir dist_ready
mounted() { findmnt -n "$mount" >/dev/null; }
check data-mount mounted
has_tool() { command -v "$1" >/dev/null; }
for tool in minisign age rsync; do check "$tool" has_tool "$tool"; done
check python3 python_ok
check docker bash -c 'docker info >/dev/null 2>&1 && docker compose version >/dev/null 2>&1'
check docker-mount mount_order
# Inspect only project labels/ports, never Config.Env (contains database credentials).
ids=$(docker ps -a -q --filter label=com.docker.compose.project=porchlight 2>/dev/null) || ids=invalid
containers=''
if [[ $ids == invalid ]]; then
  containers=invalid
elif [[ -n $ids ]]; then
  # Docker IDs are daemon-produced hexadecimal words, not user input.
  containers=$(docker inspect --format '{"labels":{{json .Config.Labels}},"ports":{{json .NetworkSettings.Ports}},"running":{{json .State.Running}}}' $ids 2>/dev/null) || containers=invalid
fi
projects=$(docker compose ls -a --format json 2>/dev/null) || projects=invalid
project_ok() {
  python3 -I -c '
import json,pathlib,sys
root=pathlib.Path(sys.argv[3])
def within(s):
 p=pathlib.Path(s)
 return p.is_absolute() and (p==root or root in p.parents) and ".." not in p.parts
for c in map(json.loads,filter(None,sys.argv[1].splitlines())):
 labels=c["labels"] or {}
 assert labels.get("com.docker.compose.project")=="porchlight"
 assert within(labels.get("com.docker.compose.project.working_dir",""))
 configs=labels.get("com.docker.compose.project.config_files","").split(",")
 assert configs and all(within(s) for s in configs)
for p in json.loads(sys.argv[2]):
 if p["Name"]=="porchlight": assert all(within(s) for s in p["ConfigFiles"].split(","))
' "$containers" "$projects" "$root" >/dev/null 2>&1
}
check compose-root project_ok
listeners=$(ss -tlnH 2>/dev/null) || listeners=invalid
port_ok() {
  python3 -I -c '
import json,sys
port=sys.argv[1]; listeners=sys.argv[2]; assert listeners!="invalid"
used=[row.split()[3].rsplit(":",1)[0].strip("[]") for row in listeners.splitlines() if len(row.split())>=4 and row.split()[3].rsplit(":",1)[-1]==port]
if used:
 bound={b.get("HostIp","") for c in map(json.loads,filter(None,sys.argv[3].splitlines())) if c["running"] for bindings in (c["ports"] or {}).values() for b in (bindings or []) if b.get("HostPort")==port}
 assert all(host in bound or (host=="*" and bound.intersection({"::","0.0.0.0"})) for host in used)
' "$1" "$listeners" "$containers" >/dev/null 2>&1
}
check port-8443 port_ok 8443
check port-8444 port_ok 8444
exit "$failed"
REMOTE
)
  remote bash -c "$script" < /dev/null
}

status() {
  remote bash -c "# status
set -euo pipefail
export PORCHLIGHT_ROOT=$(quote_command "$ROOT") PORCHLIGHT_RUN_DIR=$(quote_command "$RUN_DIR")
release=\$(readlink -f -- $(quote_command "$ROOT/release-current"))
printf 'Active release: '; readlink -- $(quote_command "$ROOT/release-current")
docker compose ls -a --filter name=porchlight
docker compose -p $(quote_command "$PROJECT") --env-file $(quote_command "$ROOT/.env") -f \"\$release/compose.yml\" ps
docker compose -p $(quote_command "$PROJECT") --env-file $(quote_command "$ROOT/.env") -f \"\$release/compose.yml\" exec -T api /usr/local/bin/porchlight healthcheck" < /dev/null
}

# Public DNS names are still untrusted input: no shell metacharacters or invalid labels.
valid_fqdn() {
  python3 -I -c 'import re,sys; s=sys.argv[1]; sys.exit(not (len(s)<=253 and re.fullmatch(r"[a-z0-9-]+(?:\.[a-z0-9-]+)*\.ts\.net",s) and all(re.fullmatch(r"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?",p) for p in s.split("."))))' "$1"
}
private_local_dir() {
  [[ ! -L $repo/var && ! -L $1 ]] || die 2 'local state directory must not be a symlink'
  mkdir -m 700 -p -- "$1"; chmod 700 -- "$1"
}
# The server's LAN address is machine-specific and stays out of the repo: it sets the enrollment CIDR and IP SAN.
server_lan() {
  local value=${PORCHLIGHT_DEPLOY_LAN:-} resolved
  [[ -n $value ]] || die 2 'server LAN address needed: set PORCHLIGHT_DEPLOY_LAN in .env (e.g. 192.168.1.10/24)'
  resolved=$(python3 -I -c 'import ipaddress,sys; s=sys.argv[1]; i=ipaddress.IPv4Interface(s); assert "/" in s and i.network.prefixlen<=30; print(i.ip,i.network)' "$value" 2>/dev/null) ||
    die 2 'invalid server LAN address: want IPv4 address/prefix, e.g. 192.168.1.10/24'
  read -r lan_ip lan_cidr <<< "$resolved"
}
funnel() {
  # All input validation precedes SSH, even dry runs.
  [[ $ts_hostname =~ ^[a-z0-9-]{1,63}$ ]] || die 2 'invalid Tailscale hostname (1-63 lowercase letters, digits or hyphens)'
  [[ $tailscale_image =~ ^tailscale/tailscale@sha256:[a-f0-9]{64}$ ]] || die 2 'Tailscale image must be tailscale/tailscale@sha256:<64 lowercase hex>'
  [[ $no_authkey == 0 || $explicit_authkey == 0 ]] || die 2 '--no-authkey conflicts with --ts-authkey-file'
  local key=$ts_authkey_file transport=$ROOT/tmp/m9-ts-authkey script result pending identity size=0 digest=dry-run
  if [[ $no_authkey == 0 ]]; then
    [[ $key == /* ]] || key=$repo/$key
    [[ -f $key && ! -L $key && -s $key && $(stat -c '%a:%u' "$key") == "600:$(id -u)" ]] || die 2 'Tailscale auth key must be a nonempty owner-owned regular 0600 file (no symlinks)'
  fi
  if [[ $no_authkey == 0 && $DRY_RUN != 1 ]]; then
    identity=$(python3 -I -c 'import hashlib,pathlib,sys; data=pathlib.Path(sys.argv[1]).read_bytes(); print(len(data),hashlib.sha256(data).hexdigest())' "$key") || die 1 'FAIL auth-key identity'
    read -r size digest <<< "$identity"
  fi
  local args=(bash "$ROOT/release-current/install.sh" funnel --root "$ROOT" --run-dir "$RUN_DIR" --project "$PROJECT" --ts-hostname "$ts_hostname" --tailscale-image "$tailscale_image")
  # Omit BOTH installer key flags: reuse part A's staged key if present, otherwise node state.
  [[ $no_authkey == 1 ]] || args+=(--ts-authkey-file "$transport")
  script="# funnel-activate
set -euo pipefail
umask 077
[[ ! -L $(quote_command "$ROOT/tmp") ]] || { printf 'FAIL funnel staging directory symlink\\n' >&2; exit 1; }
$(quote_command declare "transport=$transport")
mkdir -m 700 -p $(quote_command "$ROOT/tmp")
chmod 700 $(quote_command "$ROOT/tmp")
[[ ! -L $(quote_command "$ROOT/tmp/.m9-funnel.lock") ]] || exit 1
exec 8>>$(quote_command "$ROOT/tmp/.m9-funnel.lock")
flock -n 8 || { printf 'FAIL concurrent Funnel controller\\n' >&2; exit 1; }
# Only sweep our private, owned regular stages older than a day, while holding the transfer lock.
python3 -I - $(quote_command "$ROOT/tmp") <<'PY_SWEEP'
import os,pathlib,stat,sys,time
root=pathlib.Path(sys.argv[1])
for path in root.glob('.m9-ts-authkey.*'):
 info=path.lstat()
 if stat.S_ISREG(info.st_mode) and info.st_uid==os.getuid() and stat.S_IMODE(info.st_mode)==0o600 and time.time()-info.st_mtime>86400:
  path.unlink()
PY_SWEEP
stage='' log=''
cleanup() { rm -f -- \"\$transport\"; [[ -z \$stage ]] || rm -f -- \"\$stage\"; [[ -z \$log ]] || rm -f -- \"\$log\"; }
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
mkdir -m 700 -p $(quote_command "$ROOT/tmp")
chmod 700 $(quote_command "$ROOT/tmp")
"
  if [[ $no_authkey == 0 ]]; then
    script+="stage=\$(mktemp $(quote_command "$ROOT/tmp/.m9-ts-authkey.XXXXXX"))
cat > \"\$stage\"
python3 -I -c 'import hashlib,pathlib,sys; p=pathlib.Path(sys.argv[1]); data=p.read_bytes(); sys.exit(not (len(data)==int(sys.argv[2]) and hashlib.sha256(data).hexdigest()==sys.argv[3]))' \"\$stage\" $(quote_command "$size" "$digest") || { printf 'FAIL auth-key transfer integrity\\n' >&2; exit 1; }
mv -Tf -- \"\$stage\" \"\$transport\"
"
  fi
  script+="log=\$(mktemp $(quote_command "$ROOT/tmp/funnel.XXXXXX"))
$(quote_command "${args[@]}") | tee \"\$log\"
python3 -I -c 'import pathlib,re,sys; lines=re.findall(r\"^PASS funnel ([a-z0-9.-]+)$\",pathlib.Path(sys.argv[1]).read_text(),re.M); assert len(lines)==1; s=lines[0]; assert len(s)<=253 and re.fullmatch(r\"[a-z0-9-]+(?:\\.[a-z0-9-]+)*\\.ts\\.net\",s) and all(re.fullmatch(r\"[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\",p) for p in s.split(\".\"))' \"\$log\" 2>/dev/null || { printf 'FAIL funnel missing valid PASS line\\n' >&2; exit 1; }
"
  if [[ $DRY_RUN == 1 ]]; then remote bash -c "$script" < /dev/null; return; fi
  # Preserve recovery guidance, but only this controller may publish final success.
  local remote_rc=0
  if [[ $no_authkey == 1 ]]; then
    result=$(remote bash -c "$script" < /dev/null) || remote_rc=$?
  else
    result=$(remote bash -c "$script" < "$key") || remote_rc=$?
  fi
  if ((remote_rc)); then
    printf '%s\n' "$result" | python3 -I -c 'import sys; sys.stdout.writelines(line for line in sys.stdin if not line.startswith("PASS funnel "))'
    die 1 'FAIL funnel (no FQDN published; laptop key retained)'
  fi
  fqdn=$(printf '%s\n' "$result" | python3 -I -c 'import re,sys; rows=re.findall(r"^PASS funnel ([a-z0-9.-]+)$",sys.stdin.read(),re.M); assert len(rows)==1; print(rows[0])') || die 1 'FAIL funnel response'
  valid_fqdn "$fqdn" || die 1 'FAIL funnel FQDN'
  private_local_dir "$repo/var/m9"
  pending=$(mktemp "$repo/var/m9/.funnel-fqdn.XXXXXX")
  trap "$(quote_command rm -f -- "$pending")" EXIT
  printf '%s\n' "$fqdn" > "$pending"
  python3 -I - "$pending" "$repo/var/m9/funnel-fqdn" <<'PY_SAVE'
import os,pathlib,sys
pending,target=map(pathlib.Path,sys.argv[1:])
with pending.open('rb') as stream: os.fsync(stream.fileno())
os.replace(pending,target)
fd=os.open(target.parent,os.O_RDONLY|os.O_DIRECTORY)
try: os.fsync(fd)
finally: os.close(fd)
PY_SAVE
  printf '%s\n' "$result"
  [[ $no_authkey == 1 ]] || printf 'Reminder: delete the laptop auth key after successful activation (not deleted automatically).\n'
}
funnel_check() {
  if [[ -z $fqdn ]]; then
    [[ -f $repo/var/m9/funnel-fqdn ]] || die 2 'funnel-check needs --fqdn or var/m9/funnel-fqdn'
    fqdn=$(< "$repo/var/m9/funnel-fqdn")
  fi
  valid_fqdn "$fqdn" || die 2 'invalid Funnel FQDN'
  server_lan
  local script failed=0 directory=$repo/var/m9-funnel pending probe_rc=0
  script="$(quote_command export "PORCHLIGHT_ROOT=$ROOT" "PORCHLIGHT_RUN_DIR=$RUN_DIR")
$(quote_command declare "root=$ROOT" "run=$RUN_DIR" "project=$PROJECT" "fqdn=$fqdn" "lan_ip=$lan_ip")
"
  script+=$(cat <<'REMOTE'
# funnel-status
set -uo pipefail
failed=0
check() { local name=$1; shift; if "$@" >/dev/null 2>&1; then printf 'PASS %s\n' "$name"; else printf 'FAIL %s\n' "$name"; failed=1; fi; }
dc() { docker compose -p "$project" --env-file "$root/.env" -f "$root/release-current/compose.yml" -f "$root/release-current/compose.funnel.yml" --profile funnel "$@"; }
running() {
  dc exec -T tailscale tailscale status --json | python3 -I -c 'import json,sys; s=json.load(sys.stdin); assert s["BackendState"]=="Running" and s["Self"]["DNSName"].removesuffix(".")==sys.argv[1]' "$fqdn"
}
serve() {
  dc exec -T tailscale tailscale serve status --json | python3 -I -c 'import json,sys; s=json.load(sys.stdin); t=s["TCP"]["443"]; assert t["TCPForward"]=="api:9443" and t["ProxyProtocol"]==2; assert s["AllowFunnel"][sys.argv[1]+":443"] is True' "$fqdn"
}
private_state() {
  python3 -I - "$root" "$fqdn" <<'PY'
import pathlib,stat,sys
root=pathlib.Path(sys.argv[1]); fqdn=sys.argv[2]; ts=root/'tailscale'
for path,mode in [(ts,0o700),(ts/'state',0o700),(ts/'serve.json',0o600),(root/'.env',0o600)]:
 assert not path.is_symlink() and stat.S_IMODE(path.stat().st_mode)==mode
e=dict(line.split('=',1) for line in (root/'.env').read_text().splitlines() if '=' in line)
assert e['FUNNEL_ENABLED']=='1' and e['TS_AUTHKEY_SPEC']==''
assert e['PORCHLIGHT_ENDPOINTS']=='https://porchlight.lan:8443,https://'+fqdn
PY
}
sans() {
  dc run --rm --no-deps -T api server-cert --show --json | python3 -I -c 'import json,sys; s=json.load(sys.stdin)["current"]; assert {"porchlight.lan","localhost",sys.argv[1]}<=set(s["dns"]); assert {"127.0.0.1",sys.argv[2]}<=set(s["ips"])' "$fqdn" "$lan_ip"
}
check compose-valid dc config -q
check api-health dc exec -T api /usr/local/bin/porchlight healthcheck
check tailscale-running running
check authkey-cleared dc exec -T tailscale sh -c 'test -z "$TS_AUTHKEY" && test ! -e /config/authkey && test ! -L /config/authkey'
check raw-tcp-proxy-v2 serve
leftover_authkey() {
  [[ ! -e $root/tmp/m9-ts-authkey && ! -L $root/tmp/m9-ts-authkey && ! -e $root/tailscale/authkey && ! -L $root/tailscale/authkey ]]
}
check leftover-authkey leftover_authkey
check private-state private_state
check production-sans sans
exit "$failed"
REMOTE
)
  local probe
  probe=$(cat <<'PY_PROBE'
# funnel-public-tls
import ipaddress,socket,ssl,sys,time
fqdn,cafile=sys.argv[1:]
budget=20
deadline=time.monotonic()+budget
def fail(name, message):
 print('FAIL '+name, flush=True)
 print(message, file=sys.stderr, flush=True)
 raise SystemExit(1)
try:
 addresses=socket.getaddrinfo(fqdn,443,type=socket.SOCK_STREAM)
 assert addresses
 tailnet=[ipaddress.ip_network('100.64.0.0/10'),ipaddress.ip_network('fd7a:115c:a1e0::/48')]
 if any(any(ipaddress.ip_address(a[4][0].split('%')[0]) in net for net in tailnet) for a in addresses):
  fail('probe-path','this machine is on the tailnet; run funnel-check off-tailnet')
except (OSError,ValueError,AssertionError):
 fail('probe-path','Funnel DNS resolution failed; public path not probed')
print('PASS probe-path', flush=True)
try:
 ctx=ssl.create_default_context(cafile=cafile)
 ctx.minimum_version=ssl.TLSVersion.TLSv1_3
 ctx.check_hostname=True
 ctx.hostname_checks_common_name=False
 # Retry only transient post-handshake resets, never weaken the certificate-required proof.
 for attempt in range(3):
  raw=None
  for family,kind,proto,_,address in addresses:
   candidate=socket.socket(family,kind,proto)
   try:
    remaining=deadline-time.monotonic()
    if remaining<=0: raise TimeoutError()
    candidate.settimeout(remaining)
    candidate.connect(address)
    raw=candidate
    break
   except OSError:
    candidate.close()
  if raw is None: fail('public-tls','TLS connection failed or timed out')
  with raw:
   remaining=deadline-time.monotonic()
   if remaining<=0: raise TimeoutError()
   raw.settimeout(remaining)
   with ctx.wrap_socket(raw,server_hostname=fqdn) as tls:
    if tls.version()!='TLSv1.3': fail('public-tls','TLS 1.3 required')
    remaining=deadline-time.monotonic()
    if remaining<=0: raise TimeoutError()
    tls.settimeout(remaining)
    # TLS 1.3 can complete on the client before the server rejects the empty client chain.
    try:
     tls.recv(1)
    except ssl.SSLError as error:
     if getattr(error,'reason',None) in ('TLSV13_ALERT_CERTIFICATE_REQUIRED','CERTIFICATE_REQUIRED'):
      print('PASS public-tls', flush=True)
      raise SystemExit(0)
     fail('public-tls','Unexpected TLS alert; mandatory client certificate not proven')
    except (ConnectionResetError,BrokenPipeError):
     if attempt<2: continue
     fail('public-tls','Repeated post-handshake reset; mandatory mTLS not proven')
    fail('public-tls','Data or EOF without certificate-required alert; mandatory mTLS not proven')
except ssl.SSLCertVerificationError:
 fail('public-tls','Household-CA chain or FQDN SAN verification failed')
except (TimeoutError,OSError,ValueError):
 fail('public-tls','TLS handshake/read failed or timed out')
PY_PROBE
)
  if [[ $DRY_RUN == 1 ]]; then
    remote bash -c "$script" < /dev/null
    remote bash -c "# funnel-ca
cat $(quote_command "$ROOT/data/pki/ca.crt")" < /dev/null
    printf 'DRY-RUN python3 -I household-CA TLS 1.3 probe %s:443 (20 s; off-tailnet DNS gate; no client certificate or HTTP request)\n' "$fqdn"
    return
  fi
  local status_rc=0
  remote bash -c "$script" < /dev/null || status_rc=$?
  case $status_rc in
    0) printf 'PASS remote-status\n' ;;
    255) printf 'FAIL status-transport\n'; failed=1 ;;
    *) printf 'FAIL remote-status\n'; failed=1 ;;
  esac
  private_local_dir "$directory"
  pending=$(mktemp "$directory/.ca.XXXXXX")
  trap "$(quote_command rm -f -- "$pending")" EXIT
  if remote bash -c "# funnel-ca
cat $(quote_command "$ROOT/data/pki/ca.crt")" < /dev/null > "$pending" && [[ -s $pending ]]; then
    mv -Tf -- "$pending" "$directory/ca.crt"; printf 'PASS household-ca\n'
  else
    printf 'FAIL household-ca\nFAIL public-tls\n'; return 1
  fi
  # M9-R5: ssl verifies identity, then requires the specific empty-client-chain TLS alert.
  pending=$(mktemp "$directory/.tls.XXXXXX")
  trap "$(quote_command rm -f -- "$pending")" EXIT
  timeout --signal=KILL 22 python3 -I -c "$probe" "$fqdn" "$directory/ca.crt" \
    < /dev/null > "$pending" 2> "$directory/tls-check.log" || probe_rc=$?
  cat "$pending"
  cat "$directory/tls-check.log" >&2
  if ((probe_rc != 0)); then
    # Distinguish resolver stalls from TLS stalls even when the external watchdog fires.
    if ((probe_rc == 137 || probe_rc == 124)); then
      if grep -q '^PASS probe-path$' "$pending"; then
        printf 'Funnel TLS watchdog expired\n' >&2
      else
        printf 'FAIL probe-path\n'; printf 'Funnel DNS watchdog expired\n' >&2
      fi
    fi
    if ! grep -q '^FAIL probe-path$\|^FAIL public-tls$' "$pending"; then printf 'FAIL public-tls\n'; fi
    failed=1
  fi
  rm -f -- "$pending"
  return "$failed"
}

case $command in
  funnel) funnel ;;
  funnel-check) funnel_check ;;
  preflight) preflight ;;
  install)
    require_release
    server_lan
    if [[ -z $password_file && $DRY_RUN != 1 ]]; then
      [[ -t 0 && -t 1 ]] || die 2 'interactive install needs a local terminal; use --admin-password-file'
    fi
    if [[ -n $password_file ]]; then
      [[ -f $password_file && ! -L $password_file && -s $password_file && $(stat -c '%a:%u' "$password_file") == "600:$(id -u)" ]] || die 2 'admin password file must be a nonempty owner-owned regular 0600 file'
    fi
    # Resolve the public recipient before any remote access; the wrapper mounts cwd only.
    if [[ $DRY_RUN == 1 ]]; then recipient='age1[REDACTED-public-recipient]'; else
      [[ -f $repo/var/backup-recovery.key ]] || die 2 "age recovery identity missing: $repo/var/backup-recovery.key"
      keygen=$(command -v age-keygen || true)
      [[ -n $keygen ]] || keygen=$repo/scripts/bin/age-keygen
      [[ -x $keygen ]] || die 1 'cannot derive backup recipient: age-keygen unavailable (native or scripts/bin wrapper)'
      # Keep the invoked name: scripts/bin symlinks dispatch via basename($0).
      keygen=$(realpath -s -- "$keygen")
      recipient=$(cd "$repo" && "$keygen" -y var/backup-recovery.key < /dev/null 2>/dev/null) || die 1 'cannot derive backup recipient: age-keygen failed (native or scripts/bin wrapper)'
      [[ $recipient =~ ^age1[a-z0-9]+$ ]] || die 1 'invalid age public recipient'
    fi
    preflight
    stack_status=0
    remote bash -c "# stack-absent
[[ ! -e $(quote_command "$ROOT/.env") || -f $(quote_command "$ROOT/.provisioning") ]]" < /dev/null || stack_status=$?
    case $stack_status in
      0) ;;
      1) die 1 'production stack .env exists; use upgrade' ;;
      *) die 1 'cannot check remote stack state' ;;
    esac
    if [[ -z $admin_name ]]; then
      if [[ $DRY_RUN == 1 ]]; then admin_name='[prompt display name]'; else
        read -r -p 'Super-admin display name: ' admin_name || die 2 'admin name required (or use --admin-name)'
      fi
    fi
    [[ -n $admin_name ]] || die 2 'admin name required'
    if [[ $DRY_RUN == 1 ]]; then gid='[porchlight-gid]'; else
      gid=$(remote bash -c '# porchlight-gid
getent group porchlight | cut -d: -f3' < /dev/null) || die 1 'cannot resolve porchlight gid'
      [[ $gid =~ ^[0-9]+$ ]] || die 1 'cannot resolve porchlight gid'
    fi
    push_release "$version"
    args=(env "ENROLL_CIDRS=$lan_cidr" ENROLL_HOST=porchlight.lan:8444 PORCHLIGHT_ENDPOINTS=https://porchlight.lan:8443 "PORCHLIGHT_GID=$gid"
      bash "$DIST_DIR/$version/install.sh" server --source "$DIST_DIR/$version" --root "$ROOT" --run-dir "$RUN_DIR" --project "$PROJECT"
      --no-funnel --backup-recipient "$recipient" --admin-name "$admin_name" --san-dns porchlight.lan --san-dns localhost --san-ip 127.0.0.1 --san-ip "$lan_ip" "${extra_sans[@]}")
    if [[ -n $password_file ]]; then
      # No pseudo-terminal: tty line discipline would corrupt/echo the password stream.
      script="set -euo pipefail
umask 077
mkdir -m 700 -p $(quote_command "$ROOT/tmp")
password_file=\$(mktemp $(quote_command "$ROOT/tmp/admin.XXXXXX"))
trap 'rm -f -- \"\$password_file\"' EXIT
chmod 600 \"\$password_file\"
cat > \"\$password_file\"
$(quote_command "${args[@]}") --admin-password-file \"\$password_file\""
      remote bash -c "$script" < "$password_file"
    else
      remote_interactive "${args[@]}"
    fi
    status ;;
  upgrade)
    push_release "$version"
    remote bash "$DIST_DIR/$version/install.sh" upgrade --source "$DIST_DIR/$version" --root "$ROOT" --run-dir "$RUN_DIR" --project "$PROJECT" < /dev/null
    status ;;
  status) status ;;
  enroll-host)
    remote bash "$ROOT/release-current/install.sh" enroll-host --host "$enroll_host" --root "$ROOT" --run-dir "$RUN_DIR" --project "$PROJECT" < /dev/null ;;
  enroll-uri)
    [[ -n $device ]] || die 2 'enroll-uri requires --device'
    output=$out; [[ $out == /* ]] || output=$repo/$out
    output=$(realpath -m -- "$output")
    [[ $output == "$repo/var/"* && $output != "$repo/var/" ]] || die 2 'enrollment output must live under this checkout var/'
    script="# enroll-uri
set -euo pipefail
umask 077
mkdir -m 700 -p $(quote_command "$ROOT/tmp")
tmp=\$(mktemp -d $(quote_command "$ROOT/tmp/enroll.XXXXXX"))
trap 'rm -rf -- \"\$tmp\"' EXIT
chmod 700 \"\$tmp\"
"
    if [[ -n $token_file ]]; then
      script+="[[ -f $(quote_command "$token_file") && ! -L $(quote_command "$token_file") && -r $(quote_command "$token_file") && -s $(quote_command "$token_file") && \$(stat -c %a $(quote_command "$token_file")) == 600 ]] || { printf 'FAIL enroll: token file must be a readable nonempty regular 0600 file (no symlinks)\\n' >&2; exit 1; }
cp -- $(quote_command "$token_file") \"\$tmp/token\"
"
    else
      script+="python3 -I -c 'import json,pathlib,sys; t=json.load(open(sys.argv[1]))[\"bootstrap_token\"]; pathlib.Path(sys.argv[2]).write_text(t)' $(quote_command "$ROOT/init.json") \"\$tmp/token\"
"
    fi
    script+="chmod 600 \"\$tmp/token\"
active=\$(readlink -- $(quote_command "$ROOT/release-current"))
version=\${active##*/}
release=\$(readlink -f -- $(quote_command "$ROOT/release-current"))
cli=\"\$release/porch-\$version-x86_64-linux-musl\"
[[ -x \$cli ]] || { printf 'FAIL enroll: release CLI missing\\n' >&2; exit 1; }
\"\$cli\" --socket $(quote_command "$RUN_DIR/agent.sock") --token-file \"\$tmp/token\" --json enroll --user $(quote_command "$username") --device $(quote_command "$device")"
    if [[ $DRY_RUN == 1 ]]; then remote bash -c "$script"; else
      directory=$(dirname "$output")
      mkdir -m 700 -p -- "$directory"; chmod 700 -- "$directory"
      pending=$(mktemp "$directory/.enroll.XXXXXX")
      trap 'rm -f -- "$pending"' EXIT
      if ! remote bash -c "$script" < /dev/null > "$pending"; then
        # porch --json writes problems to stdout, alongside success URIs. Never
        # echo raw output or extensions; reject URI-bearing failure responses.
        python3 -I -c '
import json,pathlib,sys,unicodedata
try:
 p=pathlib.Path(sys.argv[1])
 if p.stat().st_size > 65536: sys.exit(0)
 j=json.loads(p.read_text())
 if not isinstance(j,dict) or "uri" in j: sys.exit(0)
 fields=[]
 for key in ("type","title","status","detail","code"):
  value=j.get(key)
  if type(value) not in (str,int): continue
  clean="".join(c for c in str(value) if not unicodedata.category(c).startswith("C"))[:300]
  if clean and "porchlight://" not in clean.lower(): fields.append(key+": "+clean)
 if fields: print("FAIL enroll: "+"; ".join(fields))
except (OSError,ValueError,RecursionError):
 pass
' "$pending" >&2 || true
        die 1 'enrollment failed; no URI published'
      fi
      python3 -I -c 'import json,sys; j=json.load(open(sys.argv[1])); assert isinstance(j.get("uri"),str) and j["uri"]' "$pending" 2>/dev/null || die 1 'invalid enrollment response; no URI published'
      chmod 600 "$pending"; mv -Tf -- "$pending" "$output"
      printf 'PASS enrollment URI saved to %s\n' "$out"
    fi ;;
  backup)
    remote bash -c "# backup
set -euo pipefail
release=\$(readlink -f -- $(quote_command "$ROOT/release-current"))
umask 077
mkdir -m 700 -p $(quote_command "$ROOT/tmp")
log=\$(mktemp $(quote_command "$ROOT/tmp/backup.XXXXXX"))
trap 'rm -f -- \"\$log\"' EXIT
bash \"\$release/install.sh\" backup --root $(quote_command "$ROOT") --run-dir $(quote_command "$RUN_DIR") --project $(quote_command "$PROJECT") > \"\$log\" || { printf 'FAIL backup\n' >&2; exit 1; }
python3 -I -c 'import pathlib,re,sys; lines=re.findall(r\"^PASS backup porchlight-[0-9]{8}T[0-9]{12}Z[.]tar[.]age$\",pathlib.Path(sys.argv[1]).read_text(),re.M); assert len(lines)==1; print(lines[0])' \"\$log\"" < /dev/null ;;
esac
