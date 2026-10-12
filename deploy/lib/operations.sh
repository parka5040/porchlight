#!/usr/bin/env bash
# Sourceable pure helpers. When bundled with install.sh this directory travels with the release.
_ops_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
version_code() {
  local version=$1 major minor patch code
  [[ $version =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || return 1
  major=${BASH_REMATCH[1]}; minor=${BASH_REMATCH[2]}; patch=${BASH_REMATCH[3]}
  # Bound lengths before arithmetic so untrusted input cannot overflow Bash's signed integer.
  [[ ${#major} -le 6 && ${#minor} -le 2 && ${#patch} -le 2 ]] || return 1
  code=$((major * 10000 + minor * 100 + patch))
  ((code > 0 && code <= 2100000000)) || return 1
  printf '%s\n' "$code"
}
source_kind() {
  local source=$1
  [[ -n $source && $source != -* && $source != *$'\n'* && $source != *$'\r'* ]] || return 1
  if [[ $source != *:* ]]; then printf 'local\n'; return; fi
  [[ $source =~ ^[a-zA-Z0-9_][a-zA-Z0-9_.-]*@[a-zA-Z0-9][a-zA-Z0-9_.-]*:/.*$ ]] || return 1
  printf 'ssh\n'
}
ops_python() { python3 -I -B "$_ops_dir/operations.py" "$@"; }
retention_selection() { ops_python retention-selection "$@"; }
pin_rollback_bundle() {
  local bundle=$1 filename=${1##*/} directory pinned
  [[ -f $bundle && ! -L $bundle && $filename =~ ^porchlight-[0-9]{8}T[0-9]{12}Z\.tar\.age$ ]] || return 1
  directory=$(dirname "$bundle")/rollback
  # shellcheck disable=SC2174 # The validated bundle's parent already exists; only rollback is created.
  mkdir -m 700 -p "$directory"
  pinned=$directory/$filename
  if [[ -e $pinned ]]; then cmp -s "$bundle" "$pinned" || return 1; else ln -- "$bundle" "$pinned"; fi
  # Retention scans only the top-level backups directory. This private recovery point needs operator cleanup.
  printf '%s\n' "$pinned"
}
verify_hashes() { ops_python verify "$1"; }
verify_manifest() {
  local dir=$1 key=$2
  [[ -n $key && $key != '@MINISIGN_PUBLIC_KEY@' ]] || { printf 'release key not embedded\n' >&2; return 1; }
  minisign -Vm "$dir/manifest.json" -x "$dir/manifest.json.minisig" -P "$key" >/dev/null 2>&1 || {
    printf 'release signature rejected\n' >&2; return 1;
  }
}
verify_release() {
  verify_manifest "$1" "$2" || return
  verify_hashes "$1"
}
verify_release_subset() {
  verify_manifest "$1" "$2" || return
  if [[ -e $1/release.json ]]; then ops_python verify "$1" --subset; else verify_hashes "$1"; fi
}
fetch_source() {
  local source=$1 destination=$2 kind
  kind=$(source_kind "$source") || return
  # shellcheck disable=SC2174 # Installer supplies an existing private mktemp stage as the parent.
  mkdir -m 700 -p "$destination"
  case $kind in
    local) cp -a -- "$source/." "$destination/" ;;
    ssh) rsync -a --no-links --protect-args -- "$source/" "$destination/" ;;
  esac
}
