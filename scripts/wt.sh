#!/usr/bin/env bash
# Workstream tree copies for parallel implementers (plain directories with their own databases, not git worktrees).
#
#   scripts/wt.sh create <name>        copy the tree to var/wt/<name> (+ pristine base var/wt/.base/<name>), give it
#                                      its own Postgres container, cargo target dir and a private copy of
#                                      var/gradle-home (sandboxed Gradle builds cannot coordinate cache locks across
#                                      network namespaces, and ci.sh --sandbox runs client JVM tests)
#   scripts/wt.sh diff <name> <out>    unified diff base -> tree copy (review package)
#   scripts/wt.sh merge <name>         three-way merge the copy's changes into this tree; conflicts are listed and
#                                      left as <file>.wt-conflict next to the untouched file
#   scripts/wt.sh rebase <name>        refresh the copy's base: merge this tree's changes since its base into it; on
#                                      conflicts the new base waits in var/wt/.base/<name>.pending until
#   scripts/wt.sh resolved <name>      (run after every *.wt-conflict in the copy is resolved) promotes it, so a later
#                                      merge diffs against what the copy was really rebased onto
#   scripts/wt.sh mark <name> <label>  remember the copy's current state (fix-round base) in var/wt/.marks/<name>-<label>
#   scripts/wt.sh diff-mark <name> <label> <out>   unified diff mark -> copy (scoped re-review package)
#   scripts/wt.sh drop <name>          delete the copy, its base, its database and its target dir
#
# Run with bash from the main tree. Each copy gets its OWN Postgres container (porchlight-wt-<name>, socket in the
# copy's var/pgsock): sqlx 0.9 names per-test databases after the test path alone, so two copies running the same
# test against one cluster would drop each other's databases.
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
[[ -f $root/scripts/wt.sh && $root != */var/wt/* ]] || { echo "run wt.sh from the main tree" >&2; exit 2; }
wt=$root/var/wt
excludes=(-x .git -x target -x build -x .gradle -x .kotlin -x var -x dist -x .idea -x __pycache__ -x '*.wt-conflict' -x .env)   # .env: machine profile, copied in by create, never diffed or merged

name=${2:-}
need_name() { [[ $name =~ ^[a-z0-9][a-z0-9-]{0,30}$ ]] || { echo "name must match [a-z0-9][a-z0-9-]{0,30}" >&2; exit 2; }; }
db() { echo "porchlight_${name//-/_}"; }

pg_image=$(sed -n 's/^ *image: \(postgres:18[^ ]*\)$/\1/p' "$root/deploy/compose.dev.yml" | head -n1)

pg() {  # superuser psql over a socket dir ($PGSOCK, default the copy's); the password stays in the environment
  ( set +x; . "$root/var/dev.env"
    PGHOST="${PGSOCK:-$wt/$name/var/pgsock}" PGUSER=postgres PGPASSWORD="$POSTGRES_PASSWORD" \
      psql -X -q -v ON_ERROR_STOP=1 "$@" )
}

pg_start() {  # the copy's own cluster: same image, bootstrap and passwords as the dev cluster; socket only, no TCP
  local sock=$wt/$name/var/pgsock i
  rm -f "$sock"; mkdir -p "$sock"; chmod 1777 "$sock"
  ( set +x; set -a; . "$root/var/dev.env"; set +a
    docker run -d --name "porchlight-wt-$name" --restart unless-stopped \
      -e POSTGRES_PASSWORD -e PORCHLIGHT_MIGRATOR_PASSWORD -e PORCHLIGHT_APP_PASSWORD -e PORCHLIGHT_AGENT_PASSWORD \
      -e PORCHLIGHT_BACKUP_PASSWORD \
      -v "porchlight-wt-$name:/var/lib/postgresql" \
      -v "$root/deploy/postgres/initdb:/docker-entrypoint-initdb.d:ro" \
      -v "$root/deploy/postgres/bootstrap-roles.sql:/porchlight/bootstrap-roles.sql:ro" \
      -v "$sock:/var/run/postgresql" \
      --network none "$pg_image" postgres -c max_connections=300 >/dev/null )
  for i in $(seq 1 120); do  # initdb restarts the server once; wait for the bootstrapped database
    if [[ -S $sock/.s.PGSQL.5432 ]] && pg -d porchlight -tAc 'select 1' >/dev/null 2>&1; then return 0; fi
    sleep 1
  done
  echo "postgres for $name did not come up" >&2; return 1
}

copy_tree() {  # $1 = source dir, $2 = destination dir
  mkdir -p "$2"
  rsync -a --delete --exclude-from="$root/scripts/snapshot-excludes.txt" "$1/" "$2/"
}

changed_files() {  # paths (relative) that differ between $1 and $2
  diff -rqN "${excludes[@]}" "$1" "$2" | sed -E "s#^Files $1/(.*) and $2/.* differ\$#\\1#"
}

merge3() {  # $1 = base dir, $2 = changed dir, $3 = target dir; prints conflicts, returns 1 if any
  local base=$1 src=$2 dst=$3 p b s d conflicts=0 tmp
  while IFS= read -r p; do
    b=$base/$p s=$src/$p d=$dst/$p
    if [[ ! -e $s ]]; then                          # deleted in src
      if [[ ! -e $d ]] || cmp -s "$d" "$b"; then rm -f "$d"
      else  # a marker file, so `find -name '*.wt-conflict'` and `wt.sh resolved` see this conflict too
        printf 'CONFLICT: %s was DELETED on the incoming side but CHANGED here.
Resolve by deleting the file (and this marker) or keeping it deliberately (delete this marker).
' "$p" > "$d.wt-conflict"
        echo "CONFLICT (deleted vs changed): $p"; conflicts=1
      fi
    elif [[ ! -e $b ]]; then                        # added in src
      if [[ ! -e $d ]]; then mkdir -p "$(dirname "$d")"; cp --preserve=mode "$s" "$d"
      elif ! cmp -s "$d" "$s"; then cp --preserve=mode "$s" "$d.wt-conflict"; echo "CONFLICT (added in both): $p"; conflicts=1; fi
    elif [[ ! -e $d ]]; then  # incoming version kept beside the missing file as the marker
      mkdir -p "$(dirname "$d")"; cp --preserve=mode "$s" "$d.wt-conflict"; echo "CONFLICT (changed vs deleted): $p"; conflicts=1
    elif cmp -s "$d" "$b"; then cp --preserve=mode "$s" "$d"   # target untouched since base (fresh mtime: cargo fingerprints by mtime)
    elif cmp -s "$d" "$s"; then :                   # same change on both sides
    else
      tmp=$(mktemp)
      # -E: a change made identically on both sides is merged, not bracketed (plain -m/-A flags it as a conflict)
      if diff3 -m -E -L target -L base -L "$name" "$d" "$b" "$s" > "$tmp" 2>/dev/null; then
        cat "$tmp" > "$d"
      else
        mv "$tmp" "$d.wt-conflict"; tmp=""; echo "CONFLICT (overlapping edits): $p"; conflicts=1
      fi
      [[ -z $tmp ]] || rm -f "$tmp"
    fi
  done < <(changed_files "$base" "$src")
  return $conflicts
}

create_db() {  # $1 = database name, created like bootstrap-roles.sql creates porchlight
  pg -d postgres <<SQL
SELECT format('DROP DATABASE IF EXISTS %I WITH (FORCE)', '$1') \gexec
CREATE DATABASE $1 OWNER porchlight_owner;
\connect $1
GRANT USAGE, CREATE ON SCHEMA public TO porchlight_migrator;
ALTER DATABASE $1 SET search_path = porchlight, public;
REVOKE TEMPORARY ON DATABASE $1 FROM PUBLIC;
SQL
}

case ${1:-} in
  own-pg)  # move an existing copy (created before per-copy clusters) onto its own cluster
    need_name
    [[ -d $wt/$name ]] || { echo "no copy $name" >&2; exit 1; }
    sock=$wt/$name/var/pgsock
    [[ -L $sock ]] && rm -f "$sock"
    pg_start
    create_db "$(db)"
    echo "$name now uses porchlight-wt-$name"
    ;;
  create)
    need_name
    tree=$wt/$name base=$wt/.base/$name
    [[ ! -e $tree ]] || { echo "$tree exists" >&2; exit 1; }
    copy_tree "$root" "$tree"
    copy_tree "$tree" "$base"   # from the copy, so a concurrent edit in this tree cannot slip in between the two
    mkdir -p "$tree/var"
    umask 077
    [[ ! -f $root/.env ]] || cp "$root/.env" "$tree/.env"   # the machine profile is excluded from copies like secrets
    gradle_home=$root/var/gradle-home android_home=$root/var/android-user-home
    cp -a "$root/var/gradle-home" "$tree/var/gradle-home"
    # AVDs (5.8 GB of Gradle Managed Device images) are shared, not copied: only the controller boots emulators
    rsync -a --exclude /avd "$root/var/android-user-home/" "$tree/var/android-user-home/"
    ln -s "$root/var/android-user-home/avd" "$tree/var/android-user-home/avd"
    rm -rf "$tree/var/gradle-home/daemon" "$tree/var/gradle-home/.tmp"
    gradle_home=$tree/var/gradle-home android_home=$tree/var/android-user-home
    { cat "$root/var/dev.env"
      printf '\n# wt.sh: this copy has its own database and cargo target dir\n'
      printf 'PORCHLIGHT_DB_NAME=%s\n' "$(db)"
      printf 'CARGO_TARGET_DIR=%s\n' "$tree/var/target"
      printf 'GRADLE_USER_HOME=%s\n' "$gradle_home"
      printf 'ANDROID_USER_HOME=%s\n' "$android_home"
      printf 'RUST_TEST_THREADS=4\n'
      printf 'CARGO_BUILD_JOBS=6\n'   # several copies build at once; the owner caps total CPU use
    } > "$tree/var/dev.env"
    pg_start
    create_db "$(db)"
    echo "$tree (own postgres porchlight-wt-$name, database $(db))"
    ;;
  diff)
    need_name
    out=${3:?usage: wt.sh diff <name> <out>}
    tree=$wt/$name base=$wt/.base/$name
    {
      echo "# Review package: workstream $name ($(date -u +%FT%TZ))"
      echo; echo "## Changed files"
      diff -rqN "${excludes[@]}" "$base" "$tree" | sed -e "s|$base/||g" -e "s|$tree/||g" || true
      echo; echo "## Diff"
      diff -ruN "${excludes[@]}" "$base" "$tree" | sed -e "s|$base/||g" -e "s|$tree/||g" || true
    } > "$out"
    echo "$out ($(wc -l < "$out") lines)"
    ;;
  merge)
    need_name
    [[ ! -e $wt/.base/$name.pending ]] || { echo "rebase of $name not resolved: run scripts/wt.sh resolved $name first" >&2; exit 1; }
    if merge3 "$wt/.base/$name" "$wt/$name" "$root"; then echo "merged $name cleanly"; else echo "merge of $name has conflicts (see *.wt-conflict)" >&2; exit 1; fi
    ;;
  rebase)
    need_name
    tree=$wt/$name base=$wt/.base/$name
    if [[ -e $base.pending ]]; then  # the copy already holds the pending base's changes: diff from it, never the old base
      left=$(find "$tree" -path "$tree/var" -prune -o -name '*.wt-conflict' -print | head -n1)
      [[ -z $left ]] || { echo "previous rebase of $name not resolved (e.g. ${left#"$tree/"}): resolve first" >&2; exit 1; }
      rm -rf "$base"; mv "$base.pending" "$base"; echo "promoted the resolved pending base of $name"
    fi
    fresh=$(mktemp -d "$wt/.rebase-$name.XXXXXX")
    copy_tree "$root" "$fresh"
    # main's changes since base go into the copy; the copy's own edits win nothing silently: conflicts are listed
    if merge3 "$base" "$fresh" "$tree"; then
      rm -rf "$base"; mv "$fresh" "$base"; echo "rebased $name"
    else
      rm -rf "$base.pending"; mv "$fresh" "$base.pending"
      echo "rebase of $name has conflicts (see *.wt-conflict in the copy); after resolving: scripts/wt.sh resolved $name" >&2
      exit 1
    fi
    ;;
  resolved)
    need_name
    base=$wt/.base/$name
    [[ -d $base.pending ]] || { echo "no pending rebase for $name" >&2; exit 1; }
    left=$(find "$wt/$name" -path "$wt/$name/var" -prune -o -name '*.wt-conflict' -print | head -n1)
    [[ -z $left ]] || { echo "unresolved conflict files remain, e.g. ${left#"$wt/$name/"}" >&2; exit 1; }
    rm -rf "$base"; mv "$base.pending" "$base"; echo "base of $name promoted to the resolved rebase"
    ;;
  mark)
    need_name
    label=${3:?usage: wt.sh mark <name> <label>}
    copy_tree "$wt/$name" "$wt/.marks/$name-$label"
    echo "$wt/.marks/$name-$label"
    ;;
  diff-mark)
    need_name
    label=${3:?usage: wt.sh diff-mark <name> <label> <out>}
    out=${4:?usage: wt.sh diff-mark <name> <label> <out>}
    mark=$wt/.marks/$name-$label tree=$wt/$name
    {
      echo "# Fix-round package: workstream $name since mark $label ($(date -u +%FT%TZ))"
      echo; echo "## Changed files"
      diff -rqN "${excludes[@]}" "$mark" "$tree" | sed -e "s|$mark/||g" -e "s|$tree/||g" || true
      echo; echo "## Diff"
      diff -ruN "${excludes[@]}" "$mark" "$tree" | sed -e "s|$mark/||g" -e "s|$tree/||g" || true
    } > "$out"
    echo "$out ($(wc -l < "$out") lines)"
    ;;
  drop)
    need_name
    # Gradle/Kotlin daemons started by the copy's builds outlive them and would re-create its gradle-home registry
    pkill -f -- "$wt/$name/var/gradle-home" 2>/dev/null || true
    docker rm -f "porchlight-wt-$name" >/dev/null 2>&1 || true
    if [[ -d $wt/$name/var/pgsock ]]; then  # the server's socket lock file belongs to the container's postgres user
      docker run --rm --network none -v "$wt/$name/var/pgsock:/s" "$pg_image" sh -c 'rm -f /s/.s.PGSQL.*' || true
    fi
    docker volume rm "porchlight-wt-$name" >/dev/null 2>&1 || true
    PGSOCK=$root/var/pgsock pg -d postgres -c "DROP DATABASE IF EXISTS $(db) WITH (FORCE)" 2>/dev/null || true
    rm -rf "${wt:?}/$name" "${wt:?}/.base/$name" "${wt:?}/.marks/$name"-*
    echo "dropped $name"
    ;;
  *) sed -n '2,13p' "$0" >&2; exit 2 ;;
esac
