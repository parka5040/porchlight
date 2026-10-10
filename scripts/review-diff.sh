#!/usr/bin/env bash
# Review support for tree copies and snapshots (plain directory diffs).
#   scripts/review-diff.sh base <dir>             copy the current tree (snapshot excludes apply) to <dir>
#   scripts/review-diff.sh diff <base-dir> <out>  write a stat summary and unified diff of <base-dir> -> tree to <out>
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
excludes=(-x .git -x target -x build -x .gradle -x .kotlin -x var -x dist -x '*.jar' -x '*.lockfile' -x Cargo.lock -x .sqlx -x .idea)

case ${1:-} in
  base)
    dir=${2:?usage: review-diff.sh base <dir>}
    rm -rf "$dir"
    mkdir -p "$dir"
    rsync -a --exclude-from="$root/scripts/snapshot-excludes.txt" "$root/" "$dir/"
    echo "$dir"
    ;;
  diff)
    base=${2:?usage: review-diff.sh diff <base-dir> <out>}
    out=${3:?usage: review-diff.sh diff <base-dir> <out>}
    {
      echo "# Review package: $base -> $root ($(date -u +%FT%TZ))"
      echo
      echo "## Changed files"
      diff -rqN "${excludes[@]}" "$base" "$root" | sed -e "s|$base/||" -e "s|$root/||" || true
      echo
      echo "## Diff"
      diff -ruN "${excludes[@]}" "$base" "$root" | sed -e "s|$base/||" -e "s|$root/||" || true
    } > "$out"
    echo "$out ($(wc -l < "$out") lines)"
    ;;
  *) echo "usage: review-diff.sh base <dir> | diff <base-dir> <out>" >&2; exit 2 ;;
esac
