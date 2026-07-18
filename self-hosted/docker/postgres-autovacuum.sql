-- Idempotent table-level autovacuum profile for Convex persistence tables.
--
-- Low scale factors prevent the amount of tolerated churn from growing without
-- bound as the tables scale. Non-zero thresholds avoid constant maintenance on
-- tiny development databases. TOAST is tuned separately because Convex document
-- payloads can be much larger than their heap tuples.

DO $tuning$
BEGIN
  IF to_regclass('public.documents') IS NOT NULL THEN
    ALTER TABLE public.documents SET (
      autovacuum_vacuum_threshold = 1000,
      autovacuum_vacuum_scale_factor = 0.001,
      autovacuum_vacuum_insert_threshold = 1000,
      autovacuum_vacuum_insert_scale_factor = 0.001,
      autovacuum_analyze_threshold = 500,
      autovacuum_analyze_scale_factor = 0.0005,
      autovacuum_vacuum_cost_limit = 3000,
      autovacuum_vacuum_cost_delay = 1,
      toast.autovacuum_vacuum_threshold = 1000,
      toast.autovacuum_vacuum_scale_factor = 0.001,
      toast.autovacuum_vacuum_cost_limit = 3000,
      toast.autovacuum_vacuum_cost_delay = 1
    );
  END IF;

  IF to_regclass('public.indexes') IS NOT NULL THEN
    ALTER TABLE public.indexes SET (
      autovacuum_vacuum_threshold = 1000,
      autovacuum_vacuum_scale_factor = 0.001,
      autovacuum_vacuum_insert_threshold = 1000,
      autovacuum_vacuum_insert_scale_factor = 0.001,
      autovacuum_analyze_threshold = 500,
      autovacuum_analyze_scale_factor = 0.0005,
      autovacuum_vacuum_cost_limit = 3000,
      autovacuum_vacuum_cost_delay = 1,
      toast.autovacuum_vacuum_threshold = 1000,
      toast.autovacuum_vacuum_scale_factor = 0.001,
      toast.autovacuum_vacuum_cost_limit = 3000,
      toast.autovacuum_vacuum_cost_delay = 1
    );
  END IF;

  IF to_regclass('public.persistence_globals') IS NOT NULL THEN
    ALTER TABLE public.persistence_globals SET (
      autovacuum_vacuum_threshold = 10,
      autovacuum_vacuum_scale_factor = 0,
      autovacuum_analyze_threshold = 10,
      autovacuum_analyze_scale_factor = 0
    );
  END IF;

  IF to_regclass('public.leases') IS NOT NULL THEN
    ALTER TABLE public.leases SET (
      autovacuum_vacuum_threshold = 10,
      autovacuum_vacuum_scale_factor = 0,
      autovacuum_analyze_threshold = 10,
      autovacuum_analyze_scale_factor = 0
    );
  END IF;

  IF to_regclass('public.read_only') IS NOT NULL THEN
    ALTER TABLE public.read_only SET (
      autovacuum_vacuum_threshold = 10,
      autovacuum_vacuum_scale_factor = 0,
      autovacuum_analyze_threshold = 10,
      autovacuum_analyze_scale_factor = 0
    );
  END IF;
END
$tuning$;

ANALYZE public.documents;
ANALYZE public.indexes;
ANALYZE public.persistence_globals;
ANALYZE public.leases;
ANALYZE public.read_only;
