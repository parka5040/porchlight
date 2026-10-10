#!/usr/bin/env bash
# Operator only: no sandbox run, never overwrite even a partial generation.
set -euo pipefail
umask 077
root=$(cd "$(dirname "$0")/.." && pwd)
. "$root/scripts/env.sh"
cd "$root" # Docker-backed minisign wrapper mounts the current tree.
dir=$root/var/release
mkdir -p "$root/var"
[[ ! -e $dir && ! -L $dir ]] || { printf 'refusing: var/release already exists (including partial generations)\n' >&2; exit 2; }
mkdir -m 700 "$dir"
# passwordless minisign is acceptable only for this laptop pass. Directory/file permissions protect it.
if ! "$root/scripts/bin/minisign" -G -W -s "$dir/minisign.key" -p "$dir/minisign.pub" > "$dir/key-generation.log" 2>&1; then
  printf 'key generation failed; partial output preserved in var/release\n' >&2; exit 1
fi
openssl rand -hex 32 > "$dir/keystore.pass"
if ! "$JAVA_HOME/bin/keytool" -genkeypair -alias porchlight -keyalg RSA -keysize 3072 -validity 10000 \
  -storetype JKS -keystore "$dir/porchlight-release.jks" \
  -storepass:file "$dir/keystore.pass" -keypass:file "$dir/keystore.pass" \
  -dname 'CN=Porchlight Release' > "$dir/android-key-generation.log" 2>&1; then
  printf 'APK key generation failed; partial output preserved in var/release\n' >&2; exit 1
fi
chmod 600 "$dir"/*
python3 - "$dir" <<'PY'
import base64,hashlib,pathlib,sys
p=pathlib.Path(sys.argv[1]);public=p.joinpath('minisign.pub').read_text().splitlines()[-1]
print('Public key SHA-256: '+hashlib.sha256(base64.b64decode(public,validate=True)).hexdigest())
for name in ('minisign.key','minisign.pub','porchlight-release.jks','keystore.pass'): print(p/name)
PY
