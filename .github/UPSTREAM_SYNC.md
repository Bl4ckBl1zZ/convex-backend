# Upstream sync playbook

`Bl4ckBl1zZ/convex-backend` is a long-lived fork of
[`get-convex/convex-backend`](https://github.com/get-convex/convex-backend). The
fork carries self-hosting, scaling, and benchmarking work that upstream does not
have, and upstream ships ~10 commits a day.
`.github/workflows/upstream-sync.yml` merges upstream into this fork every other
day and opens a pull request.

This document is the specification that merge follows. It is read by humans and
by the automated conflict resolver, so keep it accurate: if you add a change to
the fork that upstream could plausibly clobber, add it to the invariants below.

## Setup

Two repository secrets drive the automation.

### `SYNC_PAT` (required)

A fine-grained personal access token, **Only select repositories →
`Bl4ckBl1zZ/convex-backend`**, with these repository permissions:

| Permission    | Access         |
| ------------- | -------------- |
| Contents      | Read and write |
| Pull requests | Read and write |
| Workflows     | Read and write |

Create it at <https://github.com/settings/personal-access-tokens/new>, then:

```bash
gh secret set SYNC_PAT --repo Bl4ckBl1zZ/convex-backend
```

The default `GITHUB_TOKEN` cannot stand in for this. It is forbidden from
pushing changes under `.github/workflows/**`, which upstream syncs contain
regularly, and pull requests it opens do not trigger `pull_request` workflows —
so the CI that gates the automatic merge would never run.

Set a calendar reminder for the token's expiry. When it lapses the sync fails
loudly rather than silently, but it still stops.

### `ANTHROPIC_API_KEY` (optional but recommended)

Enables automatic conflict resolution. Without it, a conflicting sync stops and
reports the conflicted files instead of merging.

```bash
gh secret set ANTHROPIC_API_KEY --repo Bl4ckBl1zZ/convex-backend
```

### Merge policy

Clean merges are handed to GitHub auto-merge and land unattended once
`Build Convex Backend` and `Prettier` pass — those two are required checks on
`main` via the _main: require green CI via PR_ ruleset, which the repository
admin can bypass for direct pushes.

Conflict resolutions do **not** auto-merge by default: a resolution can compile,
pass the regression tests, and still be subtly wrong. Opt in with

```bash
gh variable set UPSTREAM_SYNC_AUTOMERGE_AI_RESOLVED --body true \
  --repo Bl4ckBl1zZ/convex-backend
```

### Running it by hand

```bash
gh workflow run "Upstream Sync" --repo Bl4ckBl1zZ/convex-backend
gh workflow run "Upstream Sync" -f automerge=false      # open the PR, do not merge
gh workflow run "Upstream Sync" -f upstream_ref=release # sync a different ref
```

## Merge strategy

**Always merge, never squash and never rebase.** The sync branch merges
`upstream/main` with a real merge commit, and the sync PR is merged into `main`
with a merge commit. Squashing or rebasing detaches the fork from upstream's
history, so `git merge-base` stops advancing and every later sync re-conflicts
over the entire backlog.

The sync uses one rolling branch, `upstream-sync/main`, and one open PR at a
time. If a sync PR is already open, the next run updates that branch in place
(merging both `origin/main` and `upstream/main` into it) rather than stacking a
second PR.

## Conflict resolution principle

Upstream owns _how the engine works_. The fork owns _how it scales and how it is
deployed_. When a conflict pits an upstream refactor against a fork feature, the
answer is almost never "pick a side" — it is **keep the fork's behavior,
expressed through upstream's new API**.

Concretely:

- Adopt upstream's renames, new arguments, changed trait bounds, and moved
  modules.
- Re-apply the fork's behavior on top of them.
- Never resolve a conflict by deleting a fork feature listed under
  [Invariants](#invariants).
- Never resolve a conflict by reverting an upstream bug fix.
- A conflict that cannot be resolved without dropping a fork invariant is a
  human decision. Stop and report it rather than guessing.

Textual conflicts are the easy half. Upstream API changes routinely break
fork-only code in files that git merged cleanly. Always compile after resolving.

## Invariants

These are the fork's reasons for existing. Preserve them.

### 1. Self-hosted PostgreSQL deployment profiles

`self-hosted/docker/**`, `self-hosted/docker-build/Dockerfile.backend`

Compose profiles for PostgreSQL 17, Dokploy, RestoreCord, vertical scaling, and
the horizontal Node pool, plus tuned `postgresql.*.conf` and autovacuum SQL.
Most of these files are fork-only. `docker-compose.yml` and `Dockerfile.backend`
are shared with upstream and do conflict — keep the fork's Postgres wiring,
environment knobs, and build changes while taking upstream's image/version
bumps.

Convex must receive a PostgreSQL **cluster** URL (no database name in the path).

### 2. Hardware-aware vertical scaling

`crates/common/src/knobs.rs`, `crates/local_backend/src/{lib,main}.rs`,
`crates/search/src/searcher/searchlight_knobs.rs`

Knob defaults are derived from detected CPU and memory instead of fixed
constants. Upstream regularly adds, removes, and renames knobs: take upstream's
knob set, then re-apply hardware derivation to the fork's scaled knobs. Do not
replace a hardware-derived default with an upstream constant.

Guarded by `cargo test -p common vertical_scaling --lib`.

### 3. Horizontally scalable Node executor pool

`crates/node_executor/src/{local,remote,metrics,lib}.rs`,
`crates/node_executor/src/bin/convex-node-executor.rs`,
`npm-packages/node-executor/src/local.ts`

The fork runs a pool of Node executor processes with least-loaded dispatch, and
`remote.rs` plus the `convex-node-executor` binary are fork-only. Upstream
changes to executor configuration (callback retry timing, timeouts, source
package fetching) must be threaded through to _every_ process in the pool, not
just a single executor.

### 4. Parallel independent pipelines

`crates/database/src/committer.rs`, `crates/storage/src/lib.rs`,
`crates/search_index_workers/src/{writer,search_flusher,search_compactor}.rs`,
`crates/search/src/fragmented_segment.rs`

Bounded concurrency for independent I/O: persistence writes, storage
upload/download, and search index work. Conflict validation, timestamp
assignment, and publication stay strictly ordered — only independent I/O
overlaps. Never resolve a conflict in a way that parallelizes commit ordering.

Guarded by `cargo test -p common commit_persistence --lib`.

### 5. Split isolate pools with a shared CPU budget

`crates/isolate/src/{client,concurrency_limiter}.rs`,
`crates/function_runner/src/server.rs`

Separate transaction and action isolate worker pools that share one
hardware-aware CPU limiter. Upstream's two-tier priority admission (nested UDF
callbacks outrank new external requests) must keep working across both pools;
when upstream adds an argument to the limiter, pass the correct priority rather
than dropping the fork's pool split.

Guarded by `cargo test -p isolate cloned_limiters_share_cpu_capacity --lib`.

### 6. State-only index metadata OCC filtering

`crates/indexing/src/index_registry.rs`,
`crates/database/src/bootstrap_model/system_metadata.rs`

State-only index metadata updates do not invalidate the virtual definition
index, so they do not spuriously abort concurrent transactions.

Guarded by
`cargo test -p indexing state_only_index_update_does_not_invalidate_virtual_definition_index --lib`.

### 7. Durable workflow benchmark harness

`npm-packages/scenario-runner/**`, `crates/load_generator/workloads/*.json`

Fork-only benchmark scenarios and workloads. The `scenario-runner` package has a
fork-added dependency that must survive lockfile regeneration.

### 8. CI that runs on GitHub's free hosted runners

`.github/workflows/build_local_backend.yml`,
`.github/workflows/publish_fork_backend.yml`,
`.github/actions/setup-rust/action.yml`

**This is the invariant upstream breaks most often.** Upstream targets Convex's
private `[self-hosted, aws, ...]` runners, which do not exist here — a job that
inherits those labels queues forever and never starts.

- Every workflow that must run on this fork uses a GitHub-hosted runner
  (`ubuntu-24.04` / `ubuntu-latest`).
- `setup-rust` falls back to local-disk sccache when Convex's private R2
  credentials are absent. Keep that fallback.
- `publish_fork_backend.yml` is fork-only (multi-arch GHCR publishing) and has
  no upstream counterpart.
- `precompile.yml`, `release_local_backend.yml`, and
  `release_local_dashboard.yml` are upstream-only release plumbing that never
  runs here. Take upstream's version wholesale; do not spend effort porting
  them.

## Lockfiles

Do not hand-merge lockfiles. Take upstream's copy, then regenerate so the fork's
extra dependencies are re-added:

```bash
# Cargo.lock
git checkout --theirs Cargo.lock
cargo metadata --format-version 1 >/dev/null   # re-adds fork crate deps
cargo check --locked -p storage -p search_index_workers

# npm-packages/pnpm-lock.yaml
git checkout --theirs npm-packages/pnpm-lock.yaml
just install-js
```

`npm-packages/pnpm-workspace.yaml` carries a fork `onlyBuiltDependencies`
allowlist for a pinned git dependency's prepare script. Keep those entries when
taking upstream's version of the file.

## Verification

Run what CI runs, in this order. The JS build must come first: `isolate` will
not compile without it.

```bash
npm ci --prefix scripts
just install-js
just turbo run build --filter=component-tests... --filter=convex... \
  --filter=system-udfs... --filter=udf-runtime... --filter=udf-tests...

cargo check --locked -p local_backend -p node_executor -p application \
  -p function_runner -p database -p indexing -p isolate \
  -p search_index_workers -p storage

cargo clippy --locked -p application -p database -p function_runner -p indexing \
  -p isolate -p search_index_workers -p storage --lib -- -D warnings

cargo test --locked -p common vertical_scaling --lib
cargo test --locked -p common commit_persistence --lib
cargo test --locked -p isolate cloned_limiters_share_cpu_capacity --lib
cargo test --locked -p indexing \
  state_only_index_update_does_not_invalidate_virtual_definition_index --lib
```

Formatting is checked repo-wide by the Prettier workflow (dprint config).

The four targeted tests are the fork's regression suite — one per behavioral
invariant above. If a sync makes one fail, the merge changed fork behavior, and
that is a conflict resolution bug rather than a flaky test.

## History

- `self-hosted/advanced/upstream-sync-2026-07-29.md` — the manual sync that
  established this baseline, including the conflict-by-conflict integration
  decisions that the invariants above generalize.
