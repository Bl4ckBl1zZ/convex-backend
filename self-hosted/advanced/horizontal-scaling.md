# Horizontal scaling

This fork separates stateless Node.js action processing from the stateful Convex
database leader. It is the first safe horizontal scaling boundary for
self-hosted Convex: Node executor containers do not connect to PostgreSQL,
acquire the deployment lease, or start background maintenance workers.

## Supported topology

```text
clients
   |
   v
Convex backend leader ---- PostgreSQL
   |
   +---- Node executor 1
   +---- Node executor 2
   +---- Node executor N
```

There must be exactly one backend leader for a deployment. The leader handles
queries, mutations, subscriptions, V8 functions, scheduling, indexing, and
ordered commits. Node actions can execute on any configured remote executor.
Their database callbacks are authenticated and sent back to the leader.

Running multiple ordinary `convex-local-backend` processes against the same
database is not horizontal scaling. Each process attempts to acquire the same
PostgreSQL lease, so they preempt one another. Do not put several backend
containers behind a load balancer yet.

## Docker Compose

Generate a dedicated executor secret:

```sh
export NODE_EXECUTOR_SHARED_SECRET="$(openssl rand -hex 32)"
```

Start PostgreSQL, the backend, and two remote executors:

```sh
docker compose \
  -f self-hosted/docker/docker-compose.yml \
  -f self-hosted/docker/docker-compose.postgres.yml \
  -f self-hosted/docker/docker-compose.horizontal.yml \
  up --build
```

`NODE_EXECUTOR_URLS` is a comma-separated list of base URLs. The backend sends
each new request to the executor with the fewest in-flight requests and uses
round-robin ordering to break ties.

The shared secret is mandatory and is sent in the
`x-convex-node-executor-secret` header. Keep port 3002 on a private network.
Possession of this secret grants access to an arbitrary-code execution service,
so do not reuse the Convex instance secret or expose an executor directly to the
internet.

## Failure behavior

The backend does not retry an executor request after a transport failure. A Node
action may have made an external API call before the connection failed;
automatic retry on another executor could duplicate that side effect. The failed
worker enters a short exponential cooldown and becomes eligible again later so
transient failures recover without operator action. If every worker is cooling
down, requests fail fast instead of queuing behind known-bad endpoints.
`NODE_EXECUTOR_FAILURE_COOLDOWN_SECONDS` controls the base cooldown and defaults
to 5 seconds. Use container health checks and an orchestrator with
unhealthy-container replacement to restart persistently unhealthy workers.
Monitor `remote_node_executor_failure_total` by failure type alongside the
existing Node executor duration and function metrics.

## Capacity planning

Remote executors remove Node.js CPU and heap pressure from the database leader.
They improve Node action throughput, especially for CPU-bound actions and
deployments with large external packages. They do not increase mutation commit
throughput, V8 query throughput, or subscription capacity.

Size `APPLICATION_MAX_CONCURRENT_NODE_ACTIONS` on the leader to the total
executor capacity. Start with:

```text
executors * useful concurrent actions per executor
```

For CPU-bound work, useful concurrency is usually close to the CPU allocation
per executor. For I/O-heavy work it can be higher. Benchmark p95/p99 latency,
timeouts, leader CPU, executor CPU, and memory before raising the permit.

Each executor keeps an independent package cache. The Compose overlay gives each
one a persistent volume to avoid repeated package downloads after a restart.

## What remains for backend read replicas

Read replicas need more than read-only PostgreSQL credentials:

1. a follower persistence connection that never acquires the writer lease;
2. a continuously refreshed repeatable database snapshot and index metadata;
3. durable invalidation delivery for subscriptions (the existing write log is
   process-local);
4. request routing that pins mutations, deploys, scheduled work, HTTP actions,
   and action callbacks to the leader;
5. bounded-staleness and failover semantics visible to clients.

The repository already contains `FollowerRetentionManager` and
`DatabaseSnapshot`, but the self-hosted `Application` is still parameterized by
a writable `Database`. Serving replicas before that interface is split would
risk stale results and split-brain workers. The intended next milestone is a
read-only application implementation behind the existing `ApplicationApi` trait,
followed by a leader-aware gateway.
