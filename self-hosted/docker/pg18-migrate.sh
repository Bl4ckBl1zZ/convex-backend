#!/bin/sh
# One-shot copy of the Convex database from the PostgreSQL 17 service
# (`postgres`) to the PostgreSQL 18 service (`postgres18`). The backend starts
# only after this exits 0.
#
# - Idempotent: once the copy has finished, the `convex_pg18_migration`
#   database exists on the target and later runs exit immediately.
# - Before dumping, the source is made read-only and its sessions are
#   terminated, so an old backend still running during the deploy cannot
#   commit writes that the copy would miss.
# - The copy is checked by comparing row counts of every table; any mismatch
#   exits non-zero and keeps the backend down.
#
# Rollback: point the backend back at `postgres` and run
#   ALTER DATABASE convex_self_hosted RESET default_transaction_read_only;
# on the PostgreSQL 17 service.
set -eu

SRC=postgres
DST=postgres18
DB=convex_self_hosted
MARKER=convex_pg18_migration
DUMP=/tmp/convex-pg17.dump

export PGUSER="${PGUSER:-convex}"

log() { echo "[pg18-migrate] $(date -u +%H:%M:%S) $*"; }
src() { psql -h "$SRC" -d "$DB" -v ON_ERROR_STOP=1 -Atq "$@"; }
dst() { psql -h "$DST" -d "$DB" -v ON_ERROR_STOP=1 -Atq "$@"; }

if [ "$(psql -h "$DST" -d postgres -Atqc "SELECT 1 FROM pg_database WHERE datname = '$MARKER'")" = "1" ]; then
  log "already migrated; nothing to do"
  exit 0
fi

if [ "$(dst -c "SELECT count(*) FROM pg_tables WHERE schemaname = 'public'")" != "0" ]; then
  log "target $DST/$DB has tables but no completion marker: a previous copy failed part-way"
  log "drop and recreate $DB on $DST (or its volume) before retrying"
  exit 1
fi

log "source: $(src -c 'SELECT version()')"
log "target: $(dst -c 'SELECT version()')"

log "freezing source: read-only for new sessions, terminating existing ones"
src -c "ALTER DATABASE $DB SET default_transaction_read_only = on"
for _ in 1 2 3; do
  src -c "SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity WHERE datname = '$DB' AND pid <> pg_backend_pid()" >/dev/null
  sleep 1
done
log "source sessions reconnected since (read-only): $(src -c "SELECT count(*) FROM pg_stat_activity WHERE datname = '$DB' AND pid <> pg_backend_pid()")"

log "dumping"
rm -rf "$DUMP"
pg_dump -h "$SRC" -d "$DB" -Fd -j 4 -Z lz4 --no-owner -f "$DUMP"
log "dump size: $(du -sh "$DUMP" | cut -f1)"

log "restoring"
pg_restore -h "$DST" -d "$DB" -j 4 --no-owner --exit-on-error "$DUMP"

log "analyzing"
vacuumdb -h "$DST" -d "$DB" --analyze-only -j 4 --quiet

log "verifying row counts"
counts() {
  psql -h "$1" -d "$DB" -v ON_ERROR_STOP=1 -Atq -c "
    SELECT string_agg(format('%s=%s', t, (xpath('/row/c/text()',
      query_to_xml(format('SELECT count(*) AS c FROM public.%I', t), false, true, '')))[1]::text), ' ' ORDER BY t)
    FROM (SELECT tablename AS t FROM pg_tables WHERE schemaname = 'public') s"
}
before=$(counts "$SRC")
after=$(counts "$DST")
log "source: $before"
log "target: $after"
if [ "$before" != "$after" ] || [ -z "$after" ]; then
  log "row counts differ; leaving the backend down"
  exit 1
fi

psql -h "$DST" -d postgres -v ON_ERROR_STOP=1 -q -c "CREATE DATABASE $MARKER"
psql -h "$DST" -d postgres -v ON_ERROR_STOP=1 -q -c "COMMENT ON DATABASE $MARKER IS 'convex_self_hosted copied from PostgreSQL 17 at $(date -u +%FT%TZ): $after'"
rm -rf "$DUMP"
log "done"
