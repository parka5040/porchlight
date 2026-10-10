#!/usr/bin/env bash
# Isolated laptop restore: decrypted state/diagnostics never leave var/restore-drill.
set -euo pipefail
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
. scripts/env.sh
unset POSTGRES_PASSWORD PORCHLIGHT_MIGRATOR_PASSWORD PORCHLIGHT_APP_PASSWORD PORCHLIGHT_AGENT_PASSWORD PORCHLIGHT_BACKUP_PASSWORD PORCHLIGHT_VERSION
keep=0 bundle='' bundle_supplied=0 receipt='' source='' identity=$root/var/backup-recovery.key
while (($#)); do
  case $1 in
    --keep) keep=1; shift ;;
    --source) source=${2:?}; shift 2 ;;
    --identity) identity=${2:?}; shift 2 ;;
    --receipt) receipt=${2:?}; shift 2 ;;
    *)
      (( ! bundle_supplied )) || exit 2
      bundle_supplied=1; bundle=$1
      [[ -n $bundle ]] || { printf 'FAIL explicit bundle path is empty\n'; exit 2; }
      shift ;;
  esac
done
if (( ! bundle_supplied )); then
  bundle=$(python3 -I - "$root/var/backups" <<'PY'
import pathlib,sys
files=sorted(pathlib.Path(sys.argv[1]).glob('porchlight-*.tar.age'));print(files[-1] if files else '')
PY
)
fi
[[ -s $bundle && -s $identity ]] || { printf 'FAIL drill bundle or identity missing\n'; exit 1; }
# age on this laptop sees only the repository mount. Cold desktop copies must be staged and reverified first.
if [[ -n $receipt ]]; then bundle=$(python3 -I deploy/scripts/stage-pulled-bundle.py "$bundle" "$receipt"); fi
bundle=$(realpath "$bundle")
[[ $bundle == "$root"/var/* ]] || { printf 'FAIL external pulled bundle requires --receipt for private var staging\n'; exit 1; }
drill=$root/var/restore-drill
[[ ! -e $drill ]] || { printf 'FAIL drill directory exists; inspect/remove it explicitly\n'; exit 2; }
mkdir -m 700 "$drill"
export PORCHLIGHT_ROOT=$drill/stack PORCHLIGHT_RUN_DIR=$drill/run PORCHLIGHT_PROJECT=porchlight-drill
# Isolate from both dev and production; all ports loopback for this drill.
export PG_PORT=${DRILL_PG_PORT:-55432} LAN_PORT=${DRILL_LAN_PORT:-18443} ENROLL_PORT=${DRILL_ENROLL_PORT:-18444}
export LAN_BIND=127.0.0.1 ENROLL_HOST=localhost:$ENROLL_PORT
export PORCHLIGHT_ENDPOINTS=https://localhost:$LAN_PORT
export BACKEND_SUBNET=${DRILL_BACKEND_SUBNET:-172.28.250.0/24} INGRESS_SUBNET=${DRILL_INGRESS_SUBNET:-172.28.251.0/24}
# Docker's loopback port forward may present a bridge gateway as the source.
# Match the M8 gate: only scratch gateways (/32), never an entire subnet.
ENROLL_CIDRS=$(python3 -I - "$BACKEND_SUBNET" "$INGRESS_SUBNET" <<'PY'
import ipaddress,sys
print('127.0.0.1/32,'+','.join(str(ipaddress.ip_network(net)[1])+'/32' for net in sys.argv[1:]))
PY
)
export ENROLL_CIDRS  # assigned first so a helper failure is not masked by export's status
dc() { docker compose -p porchlight-drill --env-file "$PORCHLIGHT_ROOT/.env" -f "$PORCHLIGHT_ROOT/release-current/compose.yml" "$@"; }
cleanup() {
  local code=$?
  trap - EXIT
  if (( ! keep )); then
    if [[ -f $PORCHLIGHT_ROOT/.env && -L $PORCHLIGHT_ROOT/release-current ]]; then dc down --volumes > "$drill/teardown.log" 2>&1 || code=1; fi
    # pgdata can be owned by container postgres: tear it down through the existing backup tools image.
    if [[ -f $drill/manifest.json ]]; then
      local version
      version=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["version"])' "$drill/manifest.json")
      docker run --rm --user 0 --entrypoint /bin/sh -v "$drill:$drill" "porchlight-backup:$version" -c 'rm -rf -- "$1/stack/pgdata"' sh "$drill" > "$drill/cleanup.log" 2>&1 || code=1
    fi
    rm -rf "$drill"
  fi
  if ((code)); then printf 'FAIL restore drill\n'; fi
  exit "$code"
}
trap cleanup EXIT
if ! age -d -i "$identity" -o "$drill/bundle.tar" "$bundle" > "$drill/decrypt.log" 2>&1; then exit 1; fi
if ! python3 -I deploy/lib/operations.py extract "$drill/bundle.tar" "$drill/bundle" > "$drill/verify.log" 2>&1; then exit 1; fi
if ! python3 -I deploy/lib/operations.py verify "$drill/bundle" >> "$drill/verify.log" 2>&1; then exit 1; fi
cp "$drill/bundle/manifest.json" "$drill/manifest.json"
printf 'PASS bundle manifest hashes\n'
version=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["version"])' "$drill/manifest.json")
source=${source:-$root/dist/$version}
if ! bash "$source/install.sh" restore "$bundle" --source "$source" --identity "$identity" \
  --root "$PORCHLIGHT_ROOT" --run-dir "$PORCHLIGHT_RUN_DIR" --project porchlight-drill --no-funnel --no-backup --no-start > "$drill/restore.log" 2>&1; then exit 1; fi
printf 'PASS isolated restore and migrate\n'
# Compare before any API/projector/derivation or backup writer starts.
if ! python3 -I - "$drill" 2> "$drill/counts.log" <<'PY'
import hashlib,json,pathlib,re,subprocess,sys
p=pathlib.Path(sys.argv[1]);m=json.loads((p/'manifest.json').read_text())
base=['docker','compose','-p','porchlight-drill','--env-file',str(p/'stack/.env'),'-f',str(p/'stack/release-current/compose.yml'),'exec','-T','postgres','psql','-X','-qAt','-U','postgres','-d','porchlight','-v','ON_ERROR_STOP=1']
def query(sql): return subprocess.check_output(base,input=sql,text=True,stderr=subprocess.DEVNULL).strip()
for table,count in m['table_row_counts'].items():
 assert re.fullmatch('[a-z_][a-z0-9_]*',table)
 assert int(query('SELECT count(*) FROM porchlight."'+table+'";'))==count
assert int(query('SELECT max(version) FROM public._sqlx_migrations WHERE success;'))==m['migration_version']
assert query('SELECT count(*) FROM public._sqlx_migrations WHERE NOT success;')=='0'
pem=(p/'stack/data/pki/ca.crt').read_text();import ssl
fingerprint=hashlib.sha256(ssl.PEM_cert_to_DER_cert(pem)).hexdigest()
assert query("SELECT encode(ca_fingerprint,'hex') FROM porchlight.install_state WHERE id;")==fingerprint
assert (p/'stack/data/pki/ca.crt').read_bytes()==(p/'bundle/pki/ca.crt').read_bytes()
# Trusted local helper; never import from the decrypted bundle or caller's cwd.
import importlib.util
spec=importlib.util.spec_from_file_location('blob_integrity',pathlib.Path.cwd()/'deploy/lib/blob_integrity.py')
blobs=importlib.util.module_from_spec(spec);spec.loader.exec_module(blobs)
blobs.verify_restored(query,p/'stack/data/blobs')
print('PASS restored main and thumbnail blob identities')
print('PASS table row counts and migration status')
print('PASS household CA fingerprint')
PY
then exit 1; fi
if ! dc up -d --wait api > "$drill/start.log" 2>&1; then exit 1; fi
if ! dc exec -T api /usr/local/bin/porchlight healthcheck > "$drill/health.log" 2>&1; then exit 1; fi
if ! python3 -I deploy/lib/tls-readiness.py --port "$ENROLL_PORT" --version "$version" \
  --ca "$PORCHLIGHT_ROOT/data/pki/ca.crt" --server-cert "$PORCHLIGHT_ROOT/data/pki/server.crt" > "$drill/tls-health.log" 2>&1; then exit 1; fi
printf 'PASS API healthy\nPASS TLS/application readiness\nPASS restore drill\n'
