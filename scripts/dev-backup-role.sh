#!/usr/bin/env bash
# Upgrade an existing dev cluster's backup role only. Never rotates any other role credentials.
set -euo pipefail
umask 077
root=$(cd "$(dirname "$0")/.." && pwd)
. "$root/scripts/dev-db.sh"
{
  printf '\\getenv backup_password PORCHLIGHT_BACKUP_PASSWORD\n'
  printf "SELECT 'CREATE ROLE porchlight_backup LOGIN' WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname='porchlight_backup') \\gexec\n"
  printf "ALTER ROLE porchlight_backup PASSWORD :'backup_password';\nGRANT pg_read_all_data TO porchlight_backup;\n"
} | PGPASSWORD="$POSTGRES_PASSWORD" dc exec -T -e PGPASSWORD -e PORCHLIGHT_BACKUP_PASSWORD postgres \
  psql -X -h 127.0.0.1 -U postgres -d postgres -v ON_ERROR_STOP=1 -f - > "$root/var/backup-role.log" 2>&1
printf 'PASS dev backup role bootstrapped\n'
