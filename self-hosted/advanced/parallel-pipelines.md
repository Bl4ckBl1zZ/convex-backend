# Parallel pipelines

This fork separates work that merely happened to run serially from ordering that
is part of Convex's correctness model. Independent preparation and I/O use
bounded parallel stages. State transitions are still published in deterministic
order.

The parallel-pipeline defaults are enabled with the hardware-aware vertical
scaling plan:

```text
VERTICAL_SCALING_ENABLED=true
```

Every stage has its own override. Setting vertical scaling to `false` restores
the compatibility concurrency for these stages.

## Changes in this branch

### Search indexes

Search flushers and compactors previously iterated over every eligible index
with an awaited `for` loop. The search engine had bounded execution pools, but
the worker submitted only one job at a time.

The workers now submit independent index jobs concurrently:

```text
eligible indexes
      |
      +--> bounded build/compaction jobs
                    |
                    +--> per-index metadata lock
                              |
                              +--> commit index metadata
```

The old metadata writer had one mutex per search type. A slow merge for one
index blocked every other text or vector index. It now uses a lock per index
metadata document. A flusher and compactor touching the same index remain
serialized, while unrelated indexes commit independently.

The transaction layer also used to add a whole-`_index`/`_tables` OCC dependency
to these state checkpoints. A dedicated state-only update now uses the
document's point dependency after verifying that the index name and
specification did not change. Schema pushes and all index-definition changes
keep the full registry dependency.

User-table writes also take a dependency on the virtual `_index.by_table_id`
index so an index definition cannot change underneath a mutation. The write log
now omits that virtual invalidation when an `_index` update preserves the name
and specification. Search and backfill checkpoints therefore no longer force
unrelated mutations on that table to retry, while adding, deleting, or changing
an index still invalidates them.

The blocking writer pool now has multiple hardware-aware threads and a bounded
queue. Searcher's own compaction pool remains a second global safety bound.

### Table summaries

A from-scratch table-summary snapshot previously scanned tables one after
another. All scans use the same repeatable timestamp and update independent
summary objects, so the scans now execute concurrently and merge only after they
complete.

Document revisions inside one table and transaction-log catch-up remain ordered.
Applying those revisions out of timestamp order can produce incorrect counts and
inferred shapes.

### In-memory database indexes

Loading enabled indexes for a set of tables previously scanned one table at a
time. Each table now builds its index maps independently against the same
`PersistenceSnapshot`; the completed maps are merged into
`BackendInMemoryIndexes` afterward.

Indexes for one table still share one scan. This avoids reading the same table
once per index and preserves the existing memory-efficient construction path.

### Bootstrap metadata

The four independent persistence-global reads used to discover `_tables` and
`_index` metadata now run concurrently. The `_tables` and `_index` snapshot
scans also run concurrently. Parsing and registry construction occur only after
both results are available.

### Foreground fan-out and function logs

The shared async join helper and batched index-range fetcher previously had
fixed concurrency of 20. They now retain 20 in compatibility mode and scale with
application CPUs. Results that require input order still use an ordered buffer;
only execution overlaps. Component source packages used by a deployment push are
also downloaded through this bounded fan-out instead of one at a time.
Function-runner schema registries likewise load each component namespace's
independent `_schemas` index concurrently on a cache miss.

Fragmented vector downloads previously had a separate hardcoded limit of 4,
leaving the searcher's global fetch pool underused. They now use the same
bounded global segment-fetch limit. Fragments within one prefetch request also
fan out through that pool instead of being fetched serially. Text, vector,
prefetch, general search, and queue defaults follow the vertical CPU plan.

Function completion logging previously held one global mutex while it updated
metrics, formatted sink records, enqueued external logs, assigned a cursor, and
updated the stream ring. Metrics and stream state now have independent locks,
and record formatting happens outside both. The nonblocking sink enqueue, cursor
assignment, and ring mutation share one short critical section so sink and
stream consumers retain the same total event order.

## Hardware-aware defaults

Let:

```text
application_cpus = cpu_count - reserved_cpu_count
```

| Setting                        | Compatibility |                                Vertical default |
| ------------------------------ | ------------: | ----------------------------------------------: |
| In-memory table index loads    |             1 |                `clamp(application_cpus, 1, 16)` |
| Table-summary table scans      |             1 |                `clamp(application_cpus, 1, 16)` |
| Lightweight async joins        |            20 |          `clamp(application_cpus * 4, 20, 128)` |
| Batched index-range fetches    |            20 |          `clamp(application_cpus * 4, 20, 128)` |
| Search index builds            |             1 |       `clamp(ceil(application_cpus / 4), 1, 4)` |
| Search index compactions       |             1 |       `clamp(ceil(application_cpus / 4), 1, 8)` |
| Search metadata writer threads |             1 |         `clamp(max(builds, compactions), 1, 8)` |
| Search metadata writer queue   |             2 |                            `writer_threads * 4` |
| General search tasks           |            50 | `max(50, clamp(application_cpus * 4, 16, 128))` |
| Search segment fetches         |             8 |                `clamp(application_cpus, 8, 16)` |
| Vector searches                |            20 |   `max(20, clamp(application_cpus * 2, 8, 64))` |
| Text searches                  |            20 | `max(20, clamp(application_cpus * 4, 16, 128))` |
| Vector prefetches              |             2 |       `clamp(ceil(application_cpus / 4), 2, 8)` |

The corresponding environment variables are:

```text
IN_MEMORY_INDEX_LOAD_CONCURRENCY
TABLE_SUMMARY_SNAPSHOT_CONCURRENCY
ASYNC_JOIN_CONCURRENCY
INDEX_RANGE_BATCH_CONCURRENCY
SEARCH_INDEX_BUILD_CONCURRENCY
SEARCH_INDEX_COMPACTION_CONCURRENCY
SEARCH_INDEX_WRITER_THREADS
SEARCH_INDEX_WRITER_QUEUE_SIZE
SEARCH_GENERAL_POOL_MAX_CONCURRENCY
SEARCH_GENERAL_POOL_QUEUE_SIZE
MAX_CONCURRENT_SEGMENT_FETCHES
MAX_CONCURRENT_VECTOR_SEARCHES
MAX_CONCURRENT_TEXT_SEARCHES
MAX_CONCURRENT_VECTOR_PREFETCHES
```

`MAX_CONCURRENT_SEGMENT_COMPACTIONS` remains an optional explicit override for
the searcher's global compaction execution pool. When unset, it follows
`SEARCH_INDEX_COMPACTION_CONCURRENCY`.

For a 10-CPU container with one reserved CPU, the automatic plan uses 9
concurrent table scans, 36 lightweight joins/range fetches, 3 search builds, 3
search compactions, 3 search writer threads, a writer queue of 12, 20 vector
searches, 36 text searches, and a general search pool of 50. The vector and
general pools retain their compatibility floors on that host.

## Serialization audit

| Pipeline                                            | Status                        | Reason                                                                                      |
| --------------------------------------------------- | ----------------------------- | ------------------------------------------------------------------------------------------- |
| Commit persistence I/O                              | Parallel and bounded          | Writes are independent after validation; results publish in order.                          |
| Commit timestamp assignment and conflict validation | Ordered                       | Each transaction must observe prior accepted writes.                                        |
| Commit publication and snapshot advancement         | Ordered                       | Readers and subscriptions require monotonically published timestamps.                       |
| Maximum-repeatable-timestamp publication            | Ordered                       | An older completion must never regress the global timestamp.                                |
| WebSocket mutations from one client                 | Ordered                       | Convex guarantees client mutation order and optimistic-update consistency.                  |
| WebSocket actions                                   | Parallel                      | Actions already use `FuturesUnordered`.                                                     |
| Query-set reruns                                    | Parallel and bounded          | Queries run at the same timestamp and merge by query ID.                                    |
| Batched index ranges                                | Parallel and bounded          | Independent reads overlap; output slots preserve input order.                               |
| Search builds across indexes                        | Parallel and bounded          | Indexes have independent segment and metadata state.                                        |
| Search metadata updates for the same index          | Ordered by keyed lock         | Flusher/compactor reconciliation depends on the current segment set.                        |
| Search checkpoints versus user mutations            | Independent                   | Unchanged index definitions no longer invalidate the virtual per-table registry dependency. |
| Search bootstrap revision replay                    | Ordered                       | Deletes and replacements must be applied chronologically.                                   |
| Table-summary scans across tables                   | Parallel and bounded          | Tables are independent at a repeatable snapshot.                                            |
| Table-summary revision replay                       | Ordered                       | Count and shape deltas are not commutative across create/delete history.                    |
| In-memory index loading across tables               | Parallel and bounded          | Each table produces disjoint index maps.                                                    |
| Index-backfill tables and write chunks              | Parallel and bounded          | Existing worker and persistence controls are retained.                                      |
| Index-backfill discovery and state commits          | Parallel and point-conflicted | Per-index reads fan out; unrelated state transitions no longer share a metadata mutex.      |
| PostgreSQL previous-revision queries                | Already pipelined             | `PIPELINE_QUERIES` bounds one-connection query pipelining.                                  |
| Storage uploads/downloads                           | Already parallel and bounded  | Existing storage transfer knobs are retained.                                               |
| Function log formatting and metrics                 | Parallel domains              | Sink formatting, metrics, and ordered stream maintenance no longer share a mutex.           |
| Function log cursor/ring publication                | Ordered                       | Streaming consumers require a stable total cursor order.                                    |
| Ordered index scans                                 | Demand-driven                 | Page order and backpressure are part of cursor semantics.                                   |

The ordered rows in this table are not performance accidents. Removing their
ordering without a replacement protocol would weaken serializability, return
client mutations out of order, regress timestamps, or corrupt derived index and
summary state.

### Component-level limits outside the backend

The official `@convex-dev/workpool` 0.3.0 package has a default parallelism of
10, warns above 50, and rejects values above 100. That bound is implemented in
the deployed TypeScript component, not in this Rust repository, so increasing
backend isolate or application admission limits cannot raise it. Workloads that
use Workflow/Workpool must include this separate queue when interpreting backend
utilization.

The current Workpool validation error for values above 100 says the value must
be at most 50, although values from 51 through 100 are accepted with a warning.
Treat 100 as the actual hard ceiling for that package version.

## Checkpoint conflict validation

A sustained indexed-mutation run exposed the original virtual-index conflict:
when the text search writer published a checkpoint, in-flight mutations on the
same user table retried because they depended on `_index.by_table_id`. After the
state-only write-log filter was added, the same 80-client workload completed a
60-second sample without that OCC signature.

The regression test also checks both sides of the safety boundary: changing only
index state omits the virtual definition invalidation, while changing the
indexed fields still emits it. This is a conflict-rate improvement, not a
license to make index-definition changes concurrent with user writes.

## Tuning

Parallel table scans consume PostgreSQL read connections and memory. Keep their
combined peak below the connections left after
`COMMITTER_MAX_CONCURRENT_PERSISTENCE_WRITES`.

Search builds and compactions can each consume several CPU cores, large
temporary files, and significant memory. Reduce their concurrency before
increasing timeouts when the host shows memory pressure, disk queue growth, or
search latency regression.

The backend startup capacity log reports all resolved parallel-pipeline limits.
Benchmark initial bootstrap, multiple simultaneous search indexes, steady
queries and mutations, and PostgreSQL pool wait together before changing the
defaults.
