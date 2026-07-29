# Benchmarking

Check out our open-source benchmarking tool,
[LoadGenerator](../../crates/load_generator/README.md), for more information on
how to benchmark and load test your Convex instance.

## Durable workflow stress benchmark

`npm-packages/scenario-runner/benchmark-workflows.mjs` drives a closed-loop,
component-backed workload instead of repeatedly calling one hot function. Each
root uses `@convex-dev/workflow` to:

1. write a root node;
2. build a breadth-first fan-out tree over several levels;
3. alternate V8 action → indexed query → mutation, direct indexed read/write
   mutation, and Node action → indexed query → mutation levels;
4. store each level in one of three independently indexed application tables;
5. wait at a barrier after each level; and
6. fan in across all three tables, validate the node count, and materialize an
   aggregate.

For example, this command keeps 24 workflow roots in flight. Each root has
branching factor 4 and depth 3, so it creates 85 application nodes and executes
86 durable workflow steps:

```sh
cd npm-packages/scenario-runner
node benchmark-workflows.mjs \
  http://127.0.0.1:3210 \
  24 60 15 4 3 256 500 100 180 true 2
```

The positional parameters after the URL are:

```text
root concurrency
measurement seconds
warmup seconds
branching factor
depth
payload bytes per node
CPU-mix iterations per node
status poll interval in milliseconds
per-workflow timeout seconds
clean up completed workflow state
measured admission ramp seconds
```

The JSON result separates completed workflows per second from logical durable
steps per second. The application-function count excludes the Workflow and
Workpool components' internal scheduling functions. The result also reports
end-to-end workflow p50/p95/p99, fan-in latency, start-mutation latency,
admission retries, drain time, poll failures, workflow failures, timeouts, and
cleanup success. A sample is invalid if another compiler, container build,
backup, or benchmark is consuming the same CPU or storage.

Warm-up uses at most four roots, must complete without errors, and is cleaned
before measurement. Measured roots are ramped in rather than started as one
thundering herd. `TooManyConcurrentRequests` and `ExpiredInQueue` admission
responses use bounded exponential jitter and are counted separately instead of
being mislabeled as failed workflows.

`@convex-dev/workpool` 0.3.0 has its own component-level concurrency bounds: the
default is 10, values above the soft limit of 50 log a warning, and values above
the hard limit of 100 are rejected. The rejection currently says
`maxParallelism must be <= 50` even though 51 through 100 are accepted. This is
not a Rust backend limit. The benchmark uses 100 so a multi-root, 64-wide level
can keep the component queue populated while exercising the backend.

Workpool 0.3.0 also coordinates the entire component through singleton
`internalState` and `runStatus` documents. This deliberately serializes its main
scheduler and can dominate a high-fan-out benchmark before the Rust backend or
the local Node pool is saturated. See
[Workflow stress findings](workflow-stress-report.md) for the release-image A/B
and the implications.

## Fair A/B procedure

Use the same release build mode, CPU and memory limits, PostgreSQL 17
configuration, deployed component version, workload shape, warmup, database
contents, logging level, and cleanup policy for both sides. Drain all warmup
roots before timing begins and include the final root drain in measured wall
time.

Do not interpret start-request QPS as workflow throughput. A workflow start can
return while thousands of durable steps remain queued. The completion callback
and validated fan-in are the terminal condition.
