# Upstream sync report: 2026-07-29

## Scope

This fork merged the canonical `get-convex/convex-backend` `main` branch from
the common base `2c2e0f662b0ef02779028c5a8fca2322ae1b4cce` through upstream
commit `e835bede653955d718eda32bf6ff3dc503082cbe`.

The range contains 173 upstream commits across 505 files. The merge preserves
the fork's PostgreSQL, vertical-scaling, Node-pool, parallel-pipeline, and
Workflow benchmark changes rather than replacing them with upstream versions.

## Upstream changes incorporated

### Runtime scheduling and overload behavior

- Added a two-tier V8 concurrency limiter that prioritizes nested UDF callbacks
  over new external requests.
- Acquires concurrency capacity before assigning an isolate worker, drops
  canceled requests sooner, and unifies queue and permit timeouts.
- Improved CoDel overload handling and rejection sampling.
- Avoids releasing execution capacity during synchronous function initialization
  unless the future actually blocks.
- Removed obsolete heap and function-context compatibility knobs.

These changes complement the fork's split transaction/action isolate pools. Both
pools still share the same CPU limiter, and nested callbacks retain upstream's
high-priority admission path.

### Commit, database, and cache paths

- Fixed committer trace-parent handling and removed unnecessary committer
  clones.
- Replaced the WriteLog timestamp index with a binary heap and reduced lock hold
  time while iterating writes.
- Tolerates a missing index snapshot during page reads.
- Keeps preloaded documents packed and lifts repeated component-path lookups.
- Optimized interval insertion and asynchronous LRU cache hits/metrics.
- Allows a pushed index definition to revert to an already-active index.

The fork's bounded concurrent persistence writes remain in place. Conflict
validation, timestamp assignment, and publication remain ordered; only
independent persistence I/O overlaps.

### Storage and network reliability

- Fetches source packages with their known exact size.
- Fixed range-prefetch overflow.
- Added HTTP/2 keepalive for proxied fetch clients.
- Retries network-level failures in Node action callbacks.
- Bounds gRPC error messages to 4 KiB and adds a webhook-sink request timeout.

The fork's configurable storage upload/download concurrency is retained around
these upstream correctness fixes.

### Values, validators, SDK, and scheduling

- Added `commitTs` placeholder support across values, validators, queries,
  patches, subtransactions, cron logs, and size calculation.
- Aligned frontend/backend value comparison behavior.
- Added stable TypeScript 7 discovery and documentation.
- Splays cron schedules to avoid synchronized load spikes and permits schedules
  that omit the minute field.

### Build and repository tooling

- Replaced Rush with pnpm workspaces and Turborepo.
- Updated pnpm, Turbo, Node engine pins, Rust and JavaScript dependencies, and
  CI workflows.
- Added the Rust style guide and moved `usage_limits` into its own crate.

The fork's Workflow scenario dependency was migrated into the new pnpm lockfile.
The old ignored Rush cache is no longer required.

### Security, dashboard, and operations

- Added WebCrypto HKDF support.
- Added WorkOS MFA/profile support and MFA-gated primary-email changes.
- Reworked the dashboard command palette, responsive data editors, profile
  security UI, usage labels, and deployment-status reporting.
- Added usage-limit CLI commands and expanded streaming-export documentation.
- Reduced high-volume routine worker logs and made `convexActor` a dynamic log
  variable.

## Merge adaptations

Seven textual conflicts and three API-level compatibility issues required manual
integration:

| Area                        | Integration decision                                                                                |
| --------------------------- | --------------------------------------------------------------------------------------------------- |
| Component source downloads  | Retained bounded parallel downloads and adapted them to upstream's exact-size `SourcePackage` API.  |
| Runtime knobs               | Retained hardware-derived scaling controls and added upstream HTTP/2 keepalive knobs.               |
| Committer                   | Retained bounded persistence I/O and adopted upstream trace-parent construction.                    |
| Function runner             | Retained separate transaction/action isolate worker pools and adapted to upstream's analyze API.    |
| Isolate limiter             | Retained a shared-capacity regression test and supplied upstream's new low/high-priority argument.  |
| Index registry              | Retained state-only metadata OCC filtering and adapted packed documents to upstream's borrowed API. |
| Transaction metadata writes | Restored the `ConvexObject` import required by the fork's narrow state-update method.               |
| JavaScript workspace        | Regenerated the pnpm lockfile with the durable Workflow benchmark dependency.                       |

## Mutation concurrency after the sync

The fork continues to remove the single-host admission bottleneck for
independent mutations:

- `APPLICATION_MAX_CONCURRENT_MUTATIONS` controls concurrent mutation UDF
  admission.
- Transaction isolates share a hardware-aware CPU budget.
- `COMMITTER_MAX_CONCURRENT_PERSISTENCE_WRITES` overlaps independent PostgreSQL
  writes while leaving pool capacity for reads.

This does not make conflicting transactions commutative. Mutations that touch
the same documents or overlapping index ranges can still retry under optimistic
concurrency control. Commit validation and publication remain ordered so
timestamps, subscriptions, and client-visible mutation order stay correct.

## Validation

Completed locally:

- Rust formatting check for every fork-modified Rust file.
- `cargo check` for `local_backend`, `node_executor`, `application`,
  `function_runner`, `database`, `indexing`, `isolate`, `search_index_workers`,
  and `storage`.
- Strict Clippy (`-D warnings`) for the fork-modified library crates.
- Three hardware-aware scaling-default tests.
- PostgreSQL connection-headroom test for concurrent persistence writes.
- Shared isolate CPU-capacity test.
- State-only index metadata OCC regression test.
- pnpm lockfile regeneration.
- Turborepo build of `scenario-runner` and its Convex dependency.
- Docker Compose rendering for the base PostgreSQL, vertical, Node-pool,
  ModularBots Dokploy, and Restorecord Dokploy profiles.

The full local Docker image build could not start because Docker Desktop's
engine API socket stopped responding. Both the CLI and the supported Desktop
restart action failed to recover the local daemon. This is an environment
failure after source/Compose validation, so the integration branch's GitHub CI
is the image-build gate.
