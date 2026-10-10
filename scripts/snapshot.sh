#!/usr/bin/env bash
# Copy the source tree into a read-only, timestamped snapshot with a sha256 manifest.
# Snapshots are the local review baseline and rollback point alongside git (they also cover uncommitted work).
#
# Usage: scripts/snapshot.sh <label>
# Env:   PORCHLIGHT_SNAPSHOT_DIR, also read from .env (default: porchlight-snapshots next to this checkout)
set -euo pipefail

label=${1:?usage: scripts/snapshot.sh <label>}
[[ $label =~ ^[a-z0-9][a-z0-9.-]*$ ]] || { echo "label must match [a-z0-9][a-z0-9.-]*" >&2; exit 2; }

root=$(realpath -m "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)")
. "$root/scripts/local-env.sh"
porchlight_local_env "$root"
dest_root=$(realpath -m "${PORCHLIGHT_SNAPSHOT_DIR:-$(dirname "$root")/porchlight-snapshots}")
if [[ $dest_root == "$root" || $dest_root == "$root"/* ]]; then
  echo "snapshot dir $dest_root must not be the source root or inside it" >&2
  exit 2
fi
dest="$dest_root/$(date -u +%Y%m%dT%H%M%S%NZ)-$label"
[[ ! -e $dest ]] || { echo "snapshot destination already exists: $dest" >&2; exit 1; }

mkdir -p "$dest_root"
staging=$(mktemp -d "$dest_root/.staging.XXXXXX")
manifest=$(mktemp)
trap 'rm -f "$manifest"; [[ -d $staging ]] && { chmod -R u+w "$staging"; rm -rf "$staging"; }' EXIT

rsync -a --exclude-from="$root/scripts/snapshot-excludes.txt" "$root/" "$staging/"
chmod -R u+w "$staging"  # the source may itself be a read-only snapshot
(cd "$staging" && find . -type f ! -path ./.snapshot-manifest.sha256 -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum) > "$manifest"
mv "$manifest" "$staging/.snapshot-manifest.sha256"
chmod -R a-w "$staging"
mv -T "$staging" "$dest"

echo "$dest"
