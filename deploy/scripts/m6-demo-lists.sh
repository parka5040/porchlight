#!/usr/bin/env bash
# Controller-only: real dev LAN mTLS + porch agent socket; no secrets in PASS/FAIL output.
set -Eeuo pipefail
umask 022
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
device=${1:-var/clients/desktop}
pat=${2:-var/bootstrap.pat}
socket=${PORCHLIGHT_SOCKET:-var/run/agent.sock}
porch=${PORCH_BIN:-porch}
base=${PORCHLIGHT_LAN_BASE:-https://localhost:8443}
mkdir -p var
scratch=$(mktemp -d "$root/var/m6-lists.XXXXXX")
chmod 700 "$scratch"
trap 'rm -rf "$scratch"' EXIT
trap 'printf "FAIL lists gate (line %s)\n" "$LINENO" >&2' ERR
for file in client.crt client.key ca.crt session.token; do test -f "$device/$file"; done
test -f "$pat"
python3 - "$device/session.token" "$scratch/headers" <<'PY'
import pathlib,sys
p=pathlib.Path(sys.argv[2]);p.write_text('Authorization: Bearer '+pathlib.Path(sys.argv[1]).read_text().strip()+'\nPorchlight-Client-Version: '+pathlib.Path('VERSION').read_text().strip()+'\n')
p.chmod(0o600)
PY
cli() { "$porch" --socket "$socket" --token-file "$pat" --json lists "$@"; }
field() { python3 - "$1" "$2" <<'PY'
import json,sys
print(json.load(open(sys.argv[1]))[sys.argv[2]])
PY
}
tls() {
  local method=$1 path=$2 outfile=$3 version=${4:-} body=${5:-}
  local args=(--silent --show-error --fail --cacert "$device/ca.crt" --cert "$device/client.crt" --key "$device/client.key" -H "@$scratch/headers" -X "$method" -o "$outfile")
  if [[ $method != GET ]]; then
    args+=(-H 'Content-Type: application/json' -H "Idempotency-Key: $(python3 -c 'import uuid;print(uuid.uuid4())')")
    [[ -n $version ]] && args+=(-H "If-Match: $version")
    [[ -n $body ]] && args+=(--data-binary "@$body")
  fi
  curl "${args[@]}" "$base$path"
}
cli add 'M6 demo groceries' > "$scratch/list.json"
list=$(field "$scratch/list.json" id)
cli add-entry "$list" Oats --qty 2 --unit bags > "$scratch/entry.json"
entry=$(field "$scratch/entry.json" id)
cli check "$entry" > "$scratch/check.json"
tls GET "/v1/lists/$list" "$scratch/shown.json"
python3 - "$scratch/shown.json" <<'PY'
import json,sys
r=json.load(open(sys.argv[1]));assert len(r['got_it'])==1 and not r['entries'];assert r['got_it'][0]['label']=='Oats'
PY
printf 'PASS persistent Got it over real mTLS + porch\n'
printf '{}' > "$scratch/body.json"
tls POST "/v1/list-entries/$entry/uncheck" "$scratch/uncheck.json" "$(field "$scratch/check.json" version)" "$scratch/body.json"
cli check "$entry" > "$scratch/check-again.json"
cli clear "$list" > "$scratch/clear.json"
python3 - "$scratch/clear.json" <<'PY'
import json,sys
r=json.load(open(sys.argv[1]));assert not r['entries'] and not r['got_it']
PY
printf 'PASS uncheck, clear and optimistic versions\n'
cli add-entry "$list" Bread > "$scratch/bread.json"
bread=$(field "$scratch/bread.json" id)
cli add-entry "$list" Milk --before-id "$bread" > "$scratch/milk.json"
tls GET "/v1/lists/$list" "$scratch/order.json"
python3 - "$scratch/order.json" <<'PY'
import json,sys
r=json.load(open(sys.argv[1]));assert [e['label'] for e in r['entries']]==['Milk','Bread']
assert r['entries'][0]['position'] < r['entries'][1]['position']
PY
printf 'PASS fractional neighbour ordering over real mTLS + porch\n'
cli rm "$list" > "$scratch/deleted.json"
# Two recurring instances, each with independent checks.
python3 - "$scratch/item-body.json" <<'PY'
import datetime,json,sys
now=datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0)
json.dump({'title':'M6 demo meds','kind':'deadline','time_mode':'fixed','due_at':now.isoformat(),'tzid':'UTC','rrule':'FREQ=DAILY;COUNT=2','recurrence_mode':'calendar'},open(sys.argv[1],'w'))
PY
tls POST /v1/items "$scratch/item.json" '' "$scratch/item-body.json"
item=$(field "$scratch/item.json" id)
tls GET "/v1/items/$item/occurrences" "$scratch/occurrences.json"
read -r first second < <(python3 - "$scratch/occurrences.json" <<'PY'
import json,sys
r=json.load(open(sys.argv[1]));assert len(r['items'])==2;print(r['items'][0]['id'],r['items'][1]['id'])
PY
)
cli add 'M6 demo refills' --item-id "$item" --mode per-occurrence > "$scratch/per.json"
per=$(field "$scratch/per.json" id)
cli add-entry "$per" Refill > "$scratch/per-entry.json"
per_entry=$(field "$scratch/per-entry.json" id)
cli check "$per_entry" --occurrence-id "$first" > "$scratch/per-check.json"
tls GET "/v1/lists/$per?occurrence_id=$second" "$scratch/fresh.json"
python3 - "$scratch/fresh.json" <<'PY'
import json,sys
r=json.load(open(sys.argv[1]));assert r['entries'][0]['checked_at'] is None
PY
printf 'PASS per-occurrence checks start fresh\n'
cli rm "$per" > "$scratch/per-delete.json"
tls DELETE "/v1/items/$item" "$scratch/item-delete.json" "$(field "$scratch/item.json" version)" "$scratch/body.json"
printf 'PASS lists gate\n'
