# Vertical scaling

This fork can derive a coherent single-host capacity plan from the CPU
parallelism visible to the backend process. The plan is enabled by default in
the self-hosted Docker configuration:

```text
VERTICAL_SCALING_ENABLED=true
VERTICAL_SCALING_CPU_COUNT=0
VERTICAL_SCALING_RESERVED_CPU_COUNT=0
```

Zero means automatic. CPU detection follows the parallelism visible inside the
container. Set `VERTICAL_SCALING_CPU_COUNT` explicitly if the container runtime
reports the host's CPUs instead of its CPU quota.

Every derived setting remains individually overridable. Setting
`VERTICAL_SCALING_ENABLED=false` restores the compatibility defaults.

Start the vertically sized PostgreSQL profile with:

```sh
docker compose \
  -f self-hosted/docker/docker-compose.yml \
  -f self-hosted/docker/docker-compose.postgres.yml \
  -f self-hosted/docker/docker-compose.vertical.yml \
  up --build
```

The vertical overlay defaults to a PostgreSQL container with roughly 12-16 GiB
available. Override its `POSTGRES_*` memory variables when the database
container is smaller. PostgreSQL can terminate connections or be killed by the
container runtime if `shared_buffers`, aggregate `work_mem`, maintenance memory,
and autovacuum memory exceed its limit.

## Execution architecture

The backend uses a staged pipeline rather than one global request count:

```text
HTTP admission
   |
   +-- query/mutation limit --> transaction isolate pool --+
   |                                                       |
   +-- V8/HTTP action limit --> action isolate pool -------+--> shared V8 CPU permits
   |
   +-- Node action limit --> local or remote Node workers

mutations --> ordered validation --> bounded parallel PostgreSQL writes
          --> ordered publication
```

Queries, mutations, and deployment analysis have a dedicated isolate-worker
pool. V8 and HTTP actions have a separate pool. Long-lived actions can no
longer occupy all workers needed by latency-sensitive transactional functions.

Both V8 pools share one runnable-isolate limiter. A pool may use the full
budget when the other is idle, but mixed traffic cannot exceed the combined
budget.
V8 functions release a CPU permit while awaiting supported asynchronous
operations, allowing useful I/O overlap without allowing unbounded JavaScript
execution.

Mutation validation and publication remain ordered for transaction correctness.
Persistence writes between those stages run concurrently, bounded by
`COMMITTER_MAX_CONCURRENT_PERSISTENCE_WRITES`. This prevents a write burst from
using every PostgreSQL connection and starving query reads.

## Derived defaults

Let:

```text
application_cpus = cpu_count - reserved_cpu_count
```

At least one application CPU is always retained. Defaults are bounded:

| Setting | Derived default |
| --- | ---: |
| Runnable V8 isolate permits | `application_cpus * 2` |
| Concurrent queries | `clamp(application_cpus * 4, 16, 512)` |
| Concurrent mutations | `clamp(application_cpus * 2, 16, 256)` |
| Concurrent V8 actions | `clamp(application_cpus * 8, 64, 1024)` |
| Concurrent Node actions | `clamp(application_cpus * 8, 64, 1024)` |
| Transaction isolate workers | `clamp(application_cpus * 4, 32, 256)` |
| Action isolate workers | `clamp(application_cpus * 8, 64, 512)` |
| Parallel persistence writes | `min(clamp(application_cpus * 4, 8, 64), max(PostgreSQL pool - 16, 1), 40)` |
| Local Node processes | `clamp(ceil(cpu_count / 4), 1, 16)` |

Automatic reserved CPU count is one eighth of visible CPUs, with at least one
reserved core on machines with more than one CPU. Reserved capacity is used by
the Tokio runtime, ordered commits, PostgreSQL communication, subscription
processing, and background workers.

For example, a 10-CPU container automatically resolves to 9 application CPUs,
18 runnable V8 permits, 36 query slots, 18 mutation slots, 72 action slots, 36
transaction workers, 72 action workers, 36 parallel persistence writes, and 3
local Node processes.

## Memory and PostgreSQL constraints

CPU-derived concurrency is only a starting point. V8 workers are created lazily
but each active isolate has heap and native-memory overhead. Reduce
`MAX_TRANSACTION_ISOLATE_WORKERS` and `MAX_ACTION_ISOLATE_WORKERS` when memory
usage or garbage-collection latency grows.

Keep:

```text
COMMITTER_MAX_CONCURRENT_PERSISTENCE_WRITES
  < POSTGRES_MAX_CONNECTIONS
```

Leave enough PostgreSQL connections for query reads, subscriptions, index
workers, autovacuum monitoring, and administration. The automatic value
reserves at least 16 connections and caps persistence concurrency at 40. An
explicit override can exceed that safety bound, so benchmark the whole workload
before doing so.

Local Node processes compete with V8 and PostgreSQL on the same machine. Use
the remote Node executor pool for sustained CPU-heavy Node workloads, or lower
`LOCAL_NODE_EXECUTOR_POOL_SIZE` when V8 traffic is the priority.

## Deliberate remaining limits

Vertical scaling does not mean removing every queue or concurrency bound:

- Commit validation and publication use one ordered stream. This preserves
  serializable transaction ordering; only the persistence I/O between those
  stages is parallel.
- `COMMITTER_QUEUE_SIZE` and `ISOLATE_QUEUE_SIZE` remain overload-protection
  queues. Increasing them absorbs a longer burst but also consumes memory and
  usually worsens overload latency.
- `POSTGRES_MAX_CONNECTIONS` remains a hard database-resource boundary. More
  connections can reduce performance through memory use and lock contention.
- Search, storage transfer, deployment analysis, and index-backfill limits
  remain separately overridable. They are I/O- and memory-sensitive and should
  not automatically track CPU count.
- A local Node pool cannot share the in-process V8 semaphore because its
  workers are separate operating-system processes. For mixed CPU-heavy V8 and
  Node traffic, explicitly partition the host with
  `FUNRUN_ISOLATE_ACTIVE_THREADS` and `LOCAL_NODE_EXECUTOR_POOL_SIZE`.

The startup capacity log reports the resolved execution limits, but it cannot
infer memory bandwidth, storage IOPS, workload conflict rate, or PostgreSQL
running on a separate machine. Those remain benchmark inputs.

See [Parallel pipelines](parallel-pipelines.md) for the hardware-aware table
scan and search-index stages, and for the audit of ordering that remains
required for correctness.

## Tuning procedure

1. Pin a CPU and memory limit on the backend container.
2. Start with automatic values and record the resolved capacity log line.
3. Benchmark query-only, mutation-only, action-only, and mixed workloads.
4. Track p95/p99 latency, rejected requests, V8 permit wait, isolate workers,
   commit queue time, `database_commit_persistence_permit_seconds`, PostgreSQL
   pool wait, CPU pressure, memory, WAL, and disk latency.
5. Change only the saturated stage. Raising downstream concurrency cannot fix
   an ordered commit or storage bottleneck.

The single ordered committer is intentional. Increasing mutation execution
parallelism improves preparation and persistence overlap, but conflicting
mutations still retry and all successful commits publish in timestamp order.

## Local A/B benchmark

The final development benchmark used the same debug backend binary, PostgreSQL
17 on a disposable tmpfs data directory, 10 visible CPUs, 80 concurrent
`ConvexHttpClient` loops, a 10-second warmup, and no per-request info logging.
Every measured request completed without a client error.

Compatibility mode disables automatic sizing but retains this fork's
architecture and correctness fixes. The table therefore compares capacity
plans, not upstream against the fork.

| Workload | Capacity plan | QPS | p50 | p95 | p99 | Errors |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Cache-bypassing indexed query | Compatibility, 16 query slots | 774.1 | 98.4 ms | 168.7 ms | 210.3 ms | 0 |
| Cache-bypassing indexed query | Vertical, 36 query slots, 18 V8 permits | 669.2 | 113.4 ms | 206.7 ms | 266.1 ms | 0 |
| Indexed insert mutation | Compatibility, 16 mutation slots | 475.6 | 159.1 ms | 259.7 ms | 357.7 ms | 0 |
| Indexed insert mutation | Rejected one-per-CPU V8 policy | 422.9 | 179.0 ms | 300.4 ms | 384.7 ms | 0 |
| Indexed insert mutation | Vertical, 18 mutation slots, 18 V8 permits | 452.2 | 168.7 ms | 275.2 ms | 357.9 ms | 0 |

The one-per-application-CPU V8 policy underutilized the host, so the automatic
default now allows two runnable isolates per application CPU. This recovered
6.9% mutation throughput in the controlled policy comparison. The simple
query and single-hot-table mutation samples still favor the lower compatibility
admission limits. Extra capacity is useful for mixed and independent work, but
it is not a universal throughput win when every request competes for the same
CPU or commit stream.

The parallel-pipeline work targets multi-table bootstrap, multiple independent
search indexes, deployment fan-out, and mixed query/action/mutation workloads.
The single-table foreground benchmarks do not exercise most of those stages.
They are included to prevent the defaults from being presented as automatically
faster for every workload.

These are directional debug-build results, not production capacity claims.
Benchmark a release image, persistent production-class storage, fixed container
CPU/memory limits, a steady database size, and the real traffic mix before
selecting production values. If p95/p99 rises without a QPS gain, lower
`APPLICATION_MAX_CONCURRENT_QUERIES` or
`APPLICATION_MAX_CONCURRENT_MUTATIONS`.
