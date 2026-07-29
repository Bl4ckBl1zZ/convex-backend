# Advanced Configuration and Tuning

There is a large number of detailed configuration options in
[knobs.rs](/crates/common/src/knobs.rs). These options are configurable via
environment variables. In order to tune your Convex instance at scale for your
workload, you may need to adjust these knobs. You will have to set these
environment variables by adding them to your `docker-compose.yml` file. Commonly
overridden knobs are listed in the `environment` section of the
[`docker-compose.yml`](../docker/docker-compose.yml)

See [Scaling a self-hosted backend](scaling.md) for the interaction between the
HTTP, function, Node executor, persistence, search, and storage limits.
See [Vertical scaling](vertical-scaling.md) for the hardware-aware defaults and
the separate transaction/action isolate pools.
See [Parallel pipelines](parallel-pipelines.md) for the table-scan, search
build, compaction, and per-index writer controls.

## `APPLICATION_MAX_CONCURRENT_*` knobs

You can increase the max concurrency on your self-hosted instance with these
environment variables. Note that increasing concurrency will increase load on
your system and after a certain threshold, performance will degrade. You will
have to tune parameters based on your own hardware and workload.

The Docker configuration enables hardware-aware vertical scaling by default.
Unset function limits are derived from the CPU count visible to the container.
Raising all four limits together is usually counterproductive because the
limits protect different CPU and persistence resources.

## Parallel background pipelines

The following knobs control independent background work:

- `IN_MEMORY_INDEX_LOAD_CONCURRENCY`
- `TABLE_SUMMARY_SNAPSHOT_CONCURRENCY`
- `ASYNC_JOIN_CONCURRENCY`
- `INDEX_RANGE_BATCH_CONCURRENCY`
- `SEARCH_INDEX_BUILD_CONCURRENCY`
- `SEARCH_INDEX_COMPACTION_CONCURRENCY`
- `SEARCH_INDEX_WRITER_THREADS`
- `SEARCH_INDEX_WRITER_QUEUE_SIZE`
- `SEARCH_GENERAL_POOL_MAX_CONCURRENCY`
- `SEARCH_GENERAL_POOL_QUEUE_SIZE`
- `MAX_CONCURRENT_SEGMENT_FETCHES`
- `MAX_CONCURRENT_VECTOR_SEARCHES`
- `MAX_CONCURRENT_TEXT_SEARCHES`
- `MAX_CONCURRENT_VECTOR_PREFETCHES`

Automatic values are conservative because these stages compete with foreground
queries and mutations for PostgreSQL connections, CPU, memory, and disk I/O.
