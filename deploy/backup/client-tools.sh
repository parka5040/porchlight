#!/usr/bin/env bash
# Export only PG18.6 clients and their library/locale closure from the pinned official image.
set -euo pipefail
. /etc/os-release
[[ $VERSION_CODENAME == trixie ]] # Keep the runtime family/libc unchanged.
root=/client-root
bin=/usr/lib/postgresql/18/bin
mkdir -p "$root$bin" "$root/opt/pg/lib" "$root/usr/local/bin"
for tool in pg_dump psql pg_restore; do
  [[ $("$bin/$tool" --version) == "$tool (PostgreSQL) 18.6"* ]]
  cp "$bin/$tool" "$root$bin/"
  deps=$(ldd "$bin/$tool")
  [[ $deps != *'not found'* ]]
  # ldd reports the transitive ELF closure. Isolate it from apt's Python/OpenSSL libraries.
  while IFS= read -r library; do
    [[ -f $library ]]
    cp -L "$library" "$root/opt/pg/lib/"
    if [[ ${library##*/} == ld-linux* ]]; then cp -L "$library" "$root/opt/pg/lib/ld.so"; fi
  done < <(printf '%s\n' "$deps" | awk '$2 == "=>" {print $3} $1 ~ /^\// {print $1}')
  printf '#!/bin/sh\nexport GCONV_PATH=/opt/pg/gconv LOCPATH=/opt/pg/locale\nexec /opt/pg/lib/ld.so --library-path /opt/pg/lib %s/%s "$@"\n' "$bin" "$tool" > "$root/usr/local/bin/$tool"
  chmod 755 "$root/usr/local/bin/$tool"
done
[[ -x $root/opt/pg/lib/ld.so ]]
# Dynamically loaded encoding modules and message catalogs are not visible to ldd.
shopt -s nullglob
for directory in /usr/lib/*/gconv; do cp -aL "$directory" "$root/opt/pg/"; done
[[ -d $root/opt/pg/gconv ]]
cp -aL /usr/lib/locale "$root/opt/pg/locale"
for catalog in /usr/share/locale/*/LC_MESSAGES/{pg_dump,psql,pg_restore,libpq*}-18.mo; do
  cp --parents -L "$catalog" "$root/"
done
