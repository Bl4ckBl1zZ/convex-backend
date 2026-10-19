# This fork's production deployment

`Bl4ckBl1zZ/convex-backend` runs one production deployment: the Dokploy profile
in [`docker-compose.dokploy.yml`](../docker/docker-compose.dokploy.yml) on a
shared 16-core / 32 GiB host, with PostgreSQL 18.6. Apart from that profile, the
fork differs from upstream in two places:

- **Local Node executor pool.** `LOCAL_NODE_EXECUTOR_POOL_SIZE` (default 1,
  production 4) runs that many Node processes and sends each Node action to the
  least-loaded one. Each process has its own heap: budget about 150-200 MiB per
  process under load.
- **HTTP request limit.** The local backend honours upstream's
  `HTTP_SERVER_MAX_CONCURRENT_REQUESTS` knob (production 1024) instead of a
  hardcoded 128.

Everything else is upstream. The production profile also runs a maintenance
sidecar that applies per-table autovacuum settings and reports vacuum health
every six hours; see the comments in
[`postgres-autovacuum.sql`](../docker/postgres-autovacuum.sql).

The image is built by `.github/workflows/publish_fork_backend.yml`; see
[ghcr-image.md](./ghcr-image.md).

## 2026-10-18 production review

The fork used to carry much more: hardware-derived ("vertical scaling") knob
defaults, split transaction/action isolate pools, a commit persistence-write
limiter, parallel search and bootstrap pipelines, a narrowed index OCC check, a
function-log lock split, a remote Node executor pool, a persistent search cache
path and a workflow benchmark harness. Each was tested against upstream, and
only what measurably helped the production deployment was kept.

### Method

A local copy of the production profile: PostgreSQL 18.6 with
`postgresql.dokploy.conf` (memory settings scaled down), the backend with the
exact environment production pins, and a test app exercising every path the fork
touched. The backend and PostgreSQL were pinned to 6 CPUs and the load generator
to 2 others. Images compared:

| Name       | Image                                                                                    |
| ---------- | ---------------------------------------------------------------------------------------- |
| upstream   | `ghcr.io/convex/convex-backend:a4ad3530c` (one docs commit before the fork's merge base) |
| fork (old) | `ghcr.io/bl4ckbl1zz/convex-backend:sha-c487bee40`, the image production ran              |
| fork (new) | this branch                                                                              |

The test host was shared with other workloads. Runs during which the host's I/O
pressure exceeded 15% were discarded; results are relative, not absolute.

### Functional suite

16 checks: inserts, index range queries, pagination, updates and deletes, 64
concurrent mutations on one document, text search, vector search, V8 actions,
Node actions, 32 concurrent Node actions, 100 scheduled functions, a cron, file
upload/download/delete, storage from an action, HTTP actions and a live
subscription.

| upstream | fork (old), production settings | fork (old), vertical scaling on | fork (new) |
| -------- | ------------------------------- | ------------------------------- | ---------- |
| 16/16    | 16/16                           | 16/16                           | 16/16      |

### Throughput

Operations per second, closed loop, 20-40 s per workload after warm-up.

| Workload                                  | upstream | fork (old) | fork (old), no commit limiter |
| ----------------------------------------- | -------: | ---------: | ----------------------------: |
| Index range query, 64 clients             |    5,375 |      5,168 |                         5,364 |
| Text search, 32 clients                   |   11,351 |     11,269 |                        11,260 |
| Insert, 128 clients                       |    1,390 |      1,451 |                         1,370 |
| Contended counter, 32 clients             |      384 |        385 |                           375 |
| Mixed (70% query, 20% insert, 10% action) |    1,186 |      1,138 |                             — |

All within 5% of upstream: none of the engine changes paid for itself. Vertical
scaling on 6 CPUs was no better (queries -5%, inserts +4%, mixed -10%) and
rejected 477 V8 actions with `ExpiredInQueue` at 32 concurrent clients that
upstream served.

Node actions are the exception. CPU-bound Node actions (16 clients) ran at about
420/s on upstream's single Node process and 1,167-1,289/s on the 4-process pool.
Node actions that mostly call back into the database are bounded by the backend,
not the pool: 349/s on the new image against 340-375/s on upstream.

### Memory

Peak non-reclaimable memory of the backend container across the same workload
sequence, at a 3.5 GiB limit.

|                                  |  upstream |                      fork (old) | fork (old), pool of 1 |                                   fork (new) |
| -------------------------------- | --------: | ------------------------------: | --------------------: | -------------------------------------------: |
| After mixed load                 | 3,041 MiB |                       3,411 MiB |             3,356 MiB |                                    3,116 MiB |
| After V8 actions                 | 3,072 MiB | 3,435 MiB, stalled at 0.5 ops/s |             3,356 MiB |                                    3,161 MiB |
| Isolate threads after mixed load |        70 |                              90 |                    90 |                                           72 |
| Survived the Node workloads      |       yes |                      OOM-killed |     yes, at the limit | OOM-killed when the 4 Node processes started |

The split isolate pools kept two sets of warm isolates where upstream reuses
one, costing 300-370 MiB with no throughput gain. The new image is back to
upstream's footprint plus the Node pool, which is what no longer fits in 3.5
GiB. At a 4.2 GiB limit the new image completed every workload with a peak of
3,468 MiB against upstream's 3,370 MiB; its four Node processes held 710 MiB at
the end, against 172 MiB for upstream's one.

### Reproduced problems

- **Persistent search cache leaked disk.** With `IN_PROCESS_SEARCH_CACHE_PATH`
  on the data volume, the cache grew from 319 entries (32 MiB) to 379 (56 MiB)
  over three clean restarts and two crashes; nothing was ever reused or removed.
  Production now uses upstream's temporary cache with
  `MAX_ARCHIVE_CACHE_SIZE_BYTES` set to the same 1 GiB.
- **The maintenance sidecar cancelled autovacuum.** On a 6M-row `documents`
  table mid-vacuum, the old SQL rewrote `pg_class` and logged
  `canceling autovacuum task` on every six-hour run. The new SQL leaves
  `pg_class` untouched and logs no cancellation.
- **Wedged Node worker.** While one Node action blocked its event loop for 45 s,
  upstream completed 4 other Node actions in 30 s, each waiting 43.5 s. The
  4-process pool completed 77,832 (median 1 ms); the few requests routed to the
  blocked process still waited for it. A blocked process is not replaced until
  its action returns or times out.

### What production needs to know

- Merging to `main` redeploys the profile, but the backend image is pinned by
  `CONVEX_BACKEND_IMAGE` in the Dokploy environment; see "Merge strategy" in
  `.github/UPSTREAM_SYNC.md` for rolling out a new backend. Production has run
  `sha-3860098a0` since 2026-10-19.
- The new image uses less memory than the one production ran, but the 4-process
  Node pool still adds roughly 0.5-0.75 GiB over upstream. Check the backend's
  steady-state usage with `docker stats` against its 12 GiB limit before raising
  `LOCAL_NODE_EXECUTOR_POOL_SIZE`.
- The old `/convex/data/search-cache` directory was deleted from production's
  `convex_data` volume on 2026-10-19.
