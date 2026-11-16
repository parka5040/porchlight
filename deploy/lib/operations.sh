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
# The same fd survives verified installer exec handoffs and nested retention calls.
installer_lock() {
  local root=$1
  # shellcheck disable=SC2174 # Installer umask also protects newly created parents.
  mkdir -m 700 -p "$root" || return
  [[ ! -L $root/.installer.lock ]] || { printf 'installer lock must not be a symlink\n' >&2; return 2; }
  if [[ ${PORCHLIGHT_INSTALL_LOCK_ROOT:-} != "$root" || ! /proc/$$/fd/9 -ef $root/.installer.lock ]]; then
    exec 9>>"$root/.installer.lock"
  fi
  flock -n 9 || { printf 'installer already running for this stack\n' >&2; return 2; }
  export PORCHLIGHT_INSTALL_LOCK_ROOT=$root
}
# Desktop-only retention. Hold the transaction lock from link reads through the last deletion.
desktop_prune_images() (
  installer_lock "$1" || return
  python3 -I -B - "$1" <<'PY_DESKTOP_PRUNE'
import json,os,pathlib,re,shutil,sys
base=pathlib.Path(sys.argv[1]); releases=base/'releases'
if releases.is_symlink() or not releases.is_dir(): raise ValueError('unsafe desktop releases folder')
releases=releases.resolve(); keep=set()
if not (base/'current').is_symlink(): raise ValueError('desktop activation missing')
for name in ('current','previous'):
 link=base/name
 if link.is_symlink():
  target=link.resolve(strict=True)
  if target.parent!=releases: raise ValueError('unsafe desktop activation link')
  keep.add(target.name)
# Native jpackage processes and their bundled JVM live below the resolved image, not current.
# Non-dumpable own processes (keyring daemons, sandboxes) refuse exe; their world-readable argv[0] still
# names the launcher. Only if neither can be read is pruning skipped rather than risking a running image.
for process in pathlib.Path('/proc').iterdir():
 if not process.name.isdigit(): continue
 try:
  if process.stat().st_uid!=os.getuid(): continue
  image=pathlib.Path(os.readlink(process/'exe'))
 except (FileNotFoundError,ProcessLookupError): continue
 except PermissionError:
  try: argv0=(process/'cmdline').read_bytes().split(b'\0',1)[0]
  except (FileNotFoundError,ProcessLookupError): continue
  except PermissionError: sys.exit(0)
  if not argv0.startswith(b'/'): continue
  image=pathlib.Path(os.fsdecode(argv0)).resolve()
 try: relative=image.relative_to(releases)
 except ValueError: continue
 if relative.parts: keep.add(relative.parts[0])
for image in releases.iterdir():
 if image.name in keep or not re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)',image.name): continue
 if image.is_symlink() or not image.is_dir(): continue
 manifest=image/'release-manifest.json'
 if manifest.is_symlink() or not manifest.is_file() or not (image/'install.sh').is_file() or not (image/'app').is_dir(): continue
 try: version=json.loads(manifest.read_text())['version']
 except (OSError,ValueError,KeyError): continue
 if version==image.name: shutil.rmtree(image)
PY_DESKTOP_PRUNE
)
