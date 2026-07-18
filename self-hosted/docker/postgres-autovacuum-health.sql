-- Run with:
-- docker compose -f docker-compose.yml -f docker-compose.postgres.yml \
--   exec postgres psql -U convex -d convex_self_hosted \
--   -f /opt/convex/postgres-autovacuum-health.sql

SELECT
  relname,
  n_live_tup,
  n_dead_tup,
  round(100.0 * n_dead_tup / greatest(n_live_tup, 1), 3) AS dead_pct,
  last_autovacuum,
  autovacuum_count,
  last_autoanalyze,
  autoanalyze_count
FROM pg_stat_user_tables
ORDER BY n_dead_tup DESC, relname;

SELECT
  pid,
  datname,
  relid::regclass AS relation,
  phase,
  heap_blks_scanned,
  heap_blks_total,
  heap_blks_vacuumed,
  pg_size_pretty(dead_tuple_bytes) AS dead_tuple_bytes,
  pg_size_pretty(max_dead_tuple_bytes) AS max_dead_tuple_bytes,
  num_dead_item_ids,
  indexes_processed,
  indexes_total
FROM pg_stat_progress_vacuum;

SELECT
  relname,
  age(relfrozenxid) AS xid_age,
  pg_size_pretty(pg_total_relation_size(oid)) AS total_size,
  reloptions
FROM pg_class
WHERE relname IN ('documents', 'indexes', 'persistence_globals', 'leases', 'read_only')
ORDER BY pg_total_relation_size(oid) DESC;
