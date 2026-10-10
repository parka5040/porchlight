#!/usr/bin/env bash
# S7b controller gate. Run against the laptop TLS stack with an existing visible item.
# Credentials are private files under var/; never enable xtrace or print request headers.
set -euo pipefail
umask 022
root=$(cd "$(dirname "$0")/../.." && pwd)
: "${PORCHLIGHT_COLLAB_ITEM:?set an existing item UUID}"
: "${PORCHLIGHT_COLLAB_DEVICE_DIR:?set the enrolled device directory under var}"
: "${PORCHLIGHT_COLLAB_ORIGIN:?set the laptop TLS origin}"
: "${PORCHLIGHT_AGENT_SOCKET:?set the agent socket path}"
: "${PORCHLIGHT_AGENT_TOKEN_FILE:?set a private PAT file}"
porch=${PORCHLIGHT_PORCH_BIN:-$root/server/target/debug/porch}
work=$(mktemp -d "$root/var/m6-collab.XXXXXXXX")
trap 'rm -rf "$work"' EXIT
chmod 700 "$work"
device=$PORCHLIGHT_COLLAB_DEVICE_DIR
case $(realpath "$device") in "$root"/var/*) ;; *) printf 'FAIL credentials must be under var\n'; exit 1;; esac
# curl reads the sensitive bearer header from a private file, not ps arguments.
printf 'Authorization: Bearer %s\n' "$(< "$device/session.token")" > "$work/auth.header"
chmod 600 "$work/auth.header"
request() {
 local method=$1 path=$2 out=$3; shift 3
 curl --silent --show-error --fail-with-body --cacert "$device/ca.crt" --cert "$device/client.crt" --key "$device/client.key" \
  -H "@$work/auth.header" -H 'Porchlight-Client-Version: 0.1.0' -H "Idempotency-Key: $(uuidgen)" \
  -X "$method" "$PORCHLIGHT_COLLAB_ORIGIN$path" "$@" > "$out" 2> "$work/curl.err"
}
fail() { printf 'FAIL %s\n' "$1"; exit 1; }
request POST "/v1/items/$PORCHLIGHT_COLLAB_ITEM/comments" "$work/comment.json" -H 'Content-Type: application/json' --data '{"body":"M6 TLS collaboration demo"}' || fail 'TLS comment create'
request GET "/v1/items/$PORCHLIGHT_COLLAB_ITEM/comments" "$work/list.json" || fail 'TLS comment list'
python3 - "$work/comment.json" "$work/list.json" 2> "$work/assert.err" <<'PY' || fail 'TLS comment list contents'
import json,sys
comment=json.load(open(sys.argv[1]));page=json.load(open(sys.argv[2]));assert any(row['id']==comment['id'] for row in page['items'])
PY
printf 'PASS TLS comments\n'
printf 'M6 safe text attachment\n' > "$work/notes.txt"
request POST "/v1/items/$PORCHLIGHT_COLLAB_ITEM/attachments" "$work/attachment.json" -F "file=@$work/notes.txt;type=text/plain" || fail 'TLS attachment upload'
id=$(python3 - "$work/attachment.json" 2> "$work/assert.err" <<'PY'
import json,sys
print(json.load(open(sys.argv[1]))['id'])
PY
) || fail 'TLS attachment metadata'
request GET "/v1/attachments/$id" "$work/download.txt" || fail 'TLS attachment download'
cmp "$work/notes.txt" "$work/download.txt" || fail 'attachment bytes'
printf 'PASS TLS attachment bytes\n'
"$porch" --socket "$PORCHLIGHT_AGENT_SOCKET" --token-file "$PORCHLIGHT_AGENT_TOKEN_FILE" --json comments add "$PORCHLIGHT_COLLAB_ITEM" 'M6 porch demo' > "$work/cli-comment.json" 2> "$work/cli.err" || fail 'porch comments add'
"$porch" --socket "$PORCHLIGHT_AGENT_SOCKET" --token-file "$PORCHLIGHT_AGENT_TOKEN_FILE" --json comments list "$PORCHLIGHT_COLLAB_ITEM" > "$work/cli-list.json" 2> "$work/cli.err" || fail 'porch comments list'
"$porch" --socket "$PORCHLIGHT_AGENT_SOCKET" --token-file "$PORCHLIGHT_AGENT_TOKEN_FILE" --json attach "$PORCHLIGHT_COLLAB_ITEM" "$work/notes.txt" > "$work/cli-attachment.json" 2> "$work/cli.err" || fail 'porch attach'
printf 'PASS porch comments and attach\n'
printf 'm6-demo-collab: PASS\n'
