# Durable Workflow stress report

Date: 2026-07-29

## Outcome

The release-image Workflow benchmark found no meaningful end-to-end throughput
gain from the current backend parallelization patch on this workload. The
patched image completed 0.2545 workflows per second and the upstream control
completed 0.2559 workflows per second, a 0.55% difference that is smaller than
the expected noise of a single laptop run.

The important result is the bottleneck attribution. The workload reaches the
Workflow component's Workpool scheduler before additional backend or Node
parallelism can improve completed throughput:

- Workpool 0.3.0 has one `internalState` document and one `runStatus` document
  for the component.
- Its schema describes `internalState` as a singleton that ensures only one main
  scheduler is running.
- Enqueue, start, completion, cancelation, and recovery all coordinate through
  that scheduler and its pending tables.
- `maxParallelism` defaults to 10, warns above 50, and rejects values above 100.
  The rejection incorrectly says the maximum is 50 even though values through
  100 are accepted.
- Backend logs under load show optimistic-concurrency retries involving
  `internalState`, `runStatus`, `pendingStart`, `pendingCompletion`, and
  Workflow step documents.

This limit is in the external `@convex-dev/workpool` TypeScript component, not
the Rust backend repository. Removing Rust worker caps alone cannot horizontally
or vertically scale one mounted Workpool instance.

## Workload

Each root is one durable `@convex-dev/workflow` execution:

1. Seed one root record.
2. Run a four-wide V8 action level. Every action performs an indexed query and
   an idempotent mutation.
3. Run a 16-wide direct indexed read/write mutation level.
4. Run a 64-wide Node action level. Every Node action performs an indexed query
   and an idempotent mutation through the local Node executor.
5. Wait at a barrier after every level.
6. Fan in with three independent indexed table scans, validate all 85
   application nodes, calculate a checksum, and write an aggregate.
7. Record terminal state through the Workflow completion callback.

One root represents:

| Unit                                         | Per root | 24-root measured wave |
| -------------------------------------------- | -------: | --------------------: |
| Application nodes                            |       85 |                 2,040 |
| Durable Workflow steps                       |       86 |                 2,064 |
| V8 action steps                              |        4 |                    96 |
| Node action steps                            |       64 |                 1,536 |
| Direct mutation steps, including seed/fan-in |       18 |                   432 |
| Application function executions              |      222 |                 5,328 |

The application-function count includes the indexed queries and mutations nested
inside actions. It intentionally excludes the additional internal Workflow and
Workpool scheduling functions.

Writes use `(runToken, path)` idempotency so a durable action retry cannot
duplicate an application node. The driver considers only the completion callback
and validated fan-in terminal; a successful start request is not counted as
completed work.

## Method

The final A/B used:

- Apple M4 MacBook Air, 10 CPU cores and 24 GB host memory;
- Docker Desktop with 10 visible CPUs and 8.22 GB assigned memory;
- PostgreSQL 17.10 on a disposable 3 GB tmpfs;
- 1 GB PostgreSQL `shared_buffers`, 5 GB `effective_cache_size`, six autovacuum
  workers, and the repository's table-specific scale autovacuum settings;
- a fresh database and fresh module-storage volume for each image;
- identical deployed Workflow 0.3.3 and Workpool 0.3.0 component code;
- a four-root, 15-second admission warm-up that was fully drained, validated,
  and cleaned before measurement;
- 24 measured clients ramped over two seconds;
- 60 seconds of closed-loop admission followed by a full drain;
- branching factor 4, depth 3, 256-byte node payloads, 500 checksum-mix
  iterations per node, 100 ms status polling, and a 180-second root timeout;
- release images and warning-level backend logging; and
- zero workflow failures, admission retries, query poll failures, or cleanup
  failures in both final samples.

The control was image
`sha256:f0de0647e46c0ac830a6dda036ee78e4956cecc00144d951bf39b14cc570ad4d` at
revision `82d5e9f2e8298641e45e5227b9967aa4c1d10620`. It used 16 query permits,
16 mutation permits, 64 V8 action permits, 64 Node action permits, and one local
Node executor.

The patched image was
`sha256:f8b1757dd43e04d96c35d96602566ce3fbd10ec2320ada32d242a6a9ca769563` at
revision `ceac588d8d502df0ebc6ade24d1d66127c05ce60`. Hardware-aware vertical
sizing resolved the 10-CPU container to 36 query permits, 18 mutation permits,
72 V8 action permits, 72 Node action permits, 18 runnable V8 permits, 36 bounded
persistence writes, and three local Node executors. Both sides used a
64-connection Convex PostgreSQL pool.

## Final A/B

| Metric                            | Upstream control | Patched vertical |  Change |
| --------------------------------- | ---------------: | ---------------: | ------: |
| Completed roots                   |            24/24 |            24/24 |       — |
| Completed workflows/s             |           0.2559 |           0.2545 |  -0.55% |
| Durable steps/s                   |            22.01 |            21.89 |  -0.55% |
| Application function executions/s |            56.82 |            56.51 |  -0.55% |
| End-to-end p50                    |         86.763 s |         91.791 s |  +5.80% |
| End-to-end p95                    |         92.053 s |         92.608 s |  +0.60% |
| End-to-end p99                    |         92.141 s |         92.700 s |  +0.61% |
| Fan-in p50                        |         74.421 s |         85.028 s | +14.25% |
| Start-mutation p50                |         23.80 ms |         18.35 ms | -22.90% |
| Drain after admission             |         33.773 s |         34.289 s |  +1.53% |
| Admission retries                 |                0 |                0 |       — |
| Workflow/poll failures            |                0 |                0 |       — |

The patched image accepts the root-start mutation faster, but that improvement
does not propagate through the singleton component scheduler. End-to-end
throughput is effectively tied, and the latency differences need repeated runs
on an otherwise idle dedicated host before they should be treated as a
regression rather than noise. During development the macOS FileProvider,
CloudKit, Spotlight, media-analysis, and Storage Management services were
periodically CPU-active, so these are directional capacity results rather than
production claims.

## Stress-only findings

An earlier synchronized 24-client start, intentionally excluded from the final
table, exposed overload behavior:

- new starts can receive `TooManyConcurrentRequests` when all mutation permits
  are occupied;
- queued starts can receive `ExpiredInQueue`;
- treating those responses as terminal failures creates a retry storm; and
- increasing backend concurrency can amplify Workpool document conflicts instead
  of increasing useful completions.

The benchmark now ramps admission, applies bounded exponential jitter to those
two transient responses, reports admission retries, validates every warm-up
root, and cleans warm-up state before measurement.

After a large run, deleting application and Workflow records did not make the
PostgreSQL persistence tables empty. One observed control run retained roughly
141,580 `documents` rows and 558,082 `indexes` rows while PostgreSQL reported
zero dead tuples after `VACUUM (ANALYZE)`. These are Convex history and
component bookkeeping rows, not PostgreSQL heap bloat. PostgreSQL autovacuum
cannot remove logically live Convex history; Convex document retention must
advance before those rows become reclaimable.

## Engineering implications

1. Keep the backend parallel-pipeline changes: this Workflow workload does not
   exercise the multi-index bootstrap, search build/compaction, component
   package download, and multi-table scan paths they target, and it found no
   correctness failures.
2. Do not claim that automatic vertical sizing universally improves throughput.
   Conflict-heavy Workflow traffic must tune mutation, persistence-write,
   Workpool, and Node concurrency together.
3. Default production Workpool parallelism should remain at or below its soft
   limit and should normally be no higher than the backend mutation capacity.
   The value 100 is useful for a stress test, not a safe general default.
4. Scaling one Workflow component requires redesigning or sharding Workpool's
   singleton scheduler state. Candidate partitions are stable pool/shard IDs
   with independent `internalState`, `runStatus`, running sets, and pending
   indexes, plus a client-side hash or explicit routing key.
5. Any scheduler redesign must preserve per-work-item exactly-once state
   transitions, retry/cancel ordering, recovery generation fencing, fairness,
   and aggregate concurrency limits. It needs failure-injection tests before
   throughput benchmarking.
6. Repeat the final A/B at least five times on a dedicated host and report
   medians and dispersion. Add a long-duration retention/storage test because
   short cleanups do not represent physical persistence-table reclamation.
