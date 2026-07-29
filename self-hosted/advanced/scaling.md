# Scaling a self-hosted backend

Convex has several independent concurrency limits. Increasing only the function
permits can move the bottleneck into PostgreSQL, the HTTP server, search,
storage, or the Node.js event loop. Change one group at a time and keep latency,
timeout count, database connections, CPU, memory, and disk I/O in the same
benchmark report.

The scaling controls that are new in this fork require the fork's backend
binary. From the repository root, build and start the PostgreSQL profile used
for the benchmarks with:

```sh
docker compose \
  -f self-hosted/docker/docker-compose.yml \
  -f self-hosted/docker/docker-compose.postgres.yml \
  up --build
```

Set `CONVEX_BACKEND_IMAGE` when using a prebuilt image of this branch. An
upstream `ghcr.io/get-convex/convex-backend` image does not contain the Node
pool or the new HTTP, search, and storage wiring. The bundled PostgreSQL
credentials are for local development; replace them or use a managed database
for a production deployment.

For a large single host, start with the hardware-aware capacity plan in
[vertical-scaling.md](./vertical-scaling.md). It separates transactional and
action isolate pools, gives them a shared CPU budget, and bounds parallel commit
I/O independently from PostgreSQL read capacity.

The [parallel pipeline architecture](./parallel-pipelines.md) removes
unnecessary one-at-a-time table and search-index work while documenting the
ordered stages that preserve transaction, cursor, and derived-state correctness.

## PostgreSQL first

Use PostgreSQL rather than SQLite for sustained concurrent writes. SQLite uses a
single connection protected by a mutex in the self-hosted backend.

`POSTGRES_MAX_CONNECTIONS` controls Convex's lazy connection pool. The Rust
default is 128, which is greater than the 100 connections accepted by a stock
PostgreSQL server. The Docker configuration therefore defaults to 64. Keep:

```text
POSTGRES_MAX_CONNECTIONS
  <= PostgreSQL max_connections
     - superuser_reserved_connections
     - connections used by administrators, monitoring, and other services
```

Exhausting PostgreSQL connections can make the Convex committer shut down, so
this is a correctness boundary, not just a throughput setting.

## HTTP and function permits

`HTTP_SERVER_MAX_CONCURRENT_REQUESTS` now controls the self-hosted backend HTTP
service. Previously, that service ignored the knob and used a hardcoded limit
of 128. Docker defaults to 1024 concurrent requests, matching the Rust knob.

Function permits are separate:

- `APPLICATION_MAX_CONCURRENT_QUERIES`
- `APPLICATION_MAX_CONCURRENT_MUTATIONS`
- `APPLICATION_MAX_CONCURRENT_V8_ACTIONS`
- `APPLICATION_MAX_CONCURRENT_NODE_ACTIONS`

The compatibility defaults are 16 queries, 16 mutations, 64 V8 actions, and 64
Node actions. With vertical scaling enabled, Docker derives these values from
visible CPUs instead of forcing those defaults. More permits can reduce
throughput and increase tail latency once CPU, search indexing, or the database
is saturated.

## Local Node.js process pool

`LOCAL_NODE_EXECUTOR_POOL_SIZE` controls the number of lazy local Node.js
executor processes. Requests are sent to the least-busy process, with
round-robin tie breaking. Vertical mode derives one process per four visible
CPUs, rounded up and capped at 16. Compatibility mode defaults to one.

Cold concurrent requests are deduplicated by source-package key while each
process downloads and links a deployment. This avoids multiple first requests
removing and recreating the same package directory.

Increase this for CPU-bound Node actions only. Each process has its own V8 heap
and module state, so it consumes additional memory. The pool is local to one
backend and does not implement multi-backend routing or horizontal scaling.

For horizontal Node action processing, use the authenticated remote executor
pool described in [horizontal-scaling.md](./horizontal-scaling.md). It keeps a
single database leader while moving Node CPU and heap usage to independently
scalable containers.

## Write budget

`MAX_BYTES_WRITTEN_PER_SECOND` has a conservative Rust and base-Docker default
of 4 MiB/s. The PostgreSQL Compose override uses 16 MiB/s. Keep 4 MiB/s for
SQLite or small disks, and retain 16 MiB/s only when a large-write benchmark
shows WAL, CPU, and disk-latency headroom. Higher limits do not fix transaction
conflicts, search-index maintenance, or CPU saturation.

## Search cache and pools

The in-process search cache is configurable with:

- `IN_PROCESS_SEARCH_CACHE_PATH` (empty means a temporary directory)
- `IN_PROCESS_SEARCH_CACHE_SIZE_BYTES` (500 MiB Rust default; 2 GiB in Docker)
- `SEARCH_GENERAL_POOL_MAX_CONCURRENCY` (50 by default)
- `SEARCH_GENERAL_POOL_QUEUE_SIZE` (1000 by default)

Docker places the cache under `/convex/data/search-cache`, so it survives a
backend restart in the existing data volume. Size the cache and search pools
against available disk, memory, and CPU; a larger pool can cause congestion
collapse when segment fetches or storage are already saturated.

## Storage parallelism

`STORAGE_MAXIMUM_PARALLEL_UPLOADS` (default 8) and
`STORAGE_MAX_CONCURRENT_CHUNK_DOWNLOADS` (default 16) replace fixed compile-time
limits. Values are per upload or download, not global. Increase them only for
high-bandwidth object storage with enough connection and memory headroom.

## Benchmark workflow

Deploy `npm-packages/scenario-runner` to a disposable backend, then run the
official load generator against that existing instance. Useful workloads are:

- `benchmark_query.json`
- `benchmark_insert.json`
- `benchmark_query_and_insert.json`
- `benchmark_large_insert.json`
- `benchmark_node_cpu.json`

Use the same database contents, duration, client count, and machine state for
each comparison. Treat timeout counts and backend restarts as failures even if
the reported average throughput is high.
