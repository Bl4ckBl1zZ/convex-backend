-- Idempotent table-level autovacuum profile for Convex persistence tables.
--
-- Low scale factors prevent the amount of tolerated churn from growing without
-- bound as the tables scale. Non-zero thresholds avoid constant maintenance on
-- tiny development databases. TOAST is tuned separately because Convex document
-- payloads can be much larger than their heap tuples.
--
-- The maintenance sidecar runs this file repeatedly. `ALTER TABLE ... SET` and
-- `ANALYZE` both take a lock that cancels a running autovacuum on the table, so
-- options are only written when they differ from what is already set, and a
-- table is only analyzed explicitly if it has never been analyzed.

CREATE FUNCTION pg_temp.apply_autovacuum_profile(
  rel text,
  heap_options text[],
  toast_options text[] DEFAULT '{}'
) RETURNS void
LANGUAGE plpgsql AS $fn$
DECLARE
  heap_current text[];
  toast_current text[];
  assignments text;
BEGIN
  IF to_regclass(rel) IS NULL THEN
    RETURN;
  END IF;

  SELECT coalesce(c.reloptions, '{}'), coalesce(t.reloptions, '{}')
  INTO heap_current, toast_current
  FROM pg_class c
  LEFT JOIN pg_class t ON t.oid = c.reltoastrelid
  WHERE c.oid = rel::regclass;

  IF heap_current @> heap_options AND toast_current @> toast_options THEN
    RETURN;
  END IF;

  SELECT string_agg(o, ', ')
  INTO assignments
  FROM (
    SELECT unnest(heap_options) AS o
    UNION ALL
    SELECT 'toast.' || unnest(toast_options)
  ) options;

  EXECUTE format('ALTER TABLE %s SET (%s)', rel, assignments);
END
$fn$;

SELECT pg_temp.apply_autovacuum_profile(
  'public.documents',
  ARRAY[
    'autovacuum_vacuum_threshold=1000',
    'autovacuum_vacuum_scale_factor=0.001',
    'autovacuum_vacuum_insert_threshold=1000',
    'autovacuum_vacuum_insert_scale_factor=0.001',
    'autovacuum_analyze_threshold=500',
    'autovacuum_analyze_scale_factor=0.0005',
    'autovacuum_vacuum_cost_limit=3000',
    'autovacuum_vacuum_cost_delay=1'
  ],
  ARRAY[
    'autovacuum_vacuum_threshold=1000',
    'autovacuum_vacuum_scale_factor=0.001',
    'autovacuum_vacuum_cost_limit=3000',
    'autovacuum_vacuum_cost_delay=1'
  ]
);

SELECT pg_temp.apply_autovacuum_profile(
  'public.indexes',
  ARRAY[
    'autovacuum_vacuum_threshold=1000',
    'autovacuum_vacuum_scale_factor=0.001',
    'autovacuum_vacuum_insert_threshold=1000',
    'autovacuum_vacuum_insert_scale_factor=0.001',
    'autovacuum_analyze_threshold=500',
    'autovacuum_analyze_scale_factor=0.0005',
    'autovacuum_vacuum_cost_limit=3000',
    'autovacuum_vacuum_cost_delay=1'
  ],
  ARRAY[
    'autovacuum_vacuum_threshold=1000',
    'autovacuum_vacuum_scale_factor=0.001',
    'autovacuum_vacuum_cost_limit=3000',
    'autovacuum_vacuum_cost_delay=1'
  ]
);

SELECT pg_temp.apply_autovacuum_profile(
  rel,
  ARRAY[
    'autovacuum_vacuum_threshold=10',
    'autovacuum_vacuum_scale_factor=0',
    'autovacuum_analyze_threshold=10',
    'autovacuum_analyze_scale_factor=0'
  ]
)
FROM unnest(ARRAY[
  'public.persistence_globals',
  'public.leases',
  'public.read_only'
]) AS rel;

DO $analyze$
DECLARE
  rel text;
BEGIN
  FOR rel IN
    SELECT format('%I.%I', schemaname, relname)
    FROM pg_stat_user_tables
    WHERE schemaname = 'public'
      AND relname IN ('documents', 'indexes', 'persistence_globals', 'leases', 'read_only')
      AND last_analyze IS NULL
      AND last_autoanalyze IS NULL
  LOOP
    EXECUTE 'ANALYZE ' || rel;
  END LOOP;
END
$analyze$;
