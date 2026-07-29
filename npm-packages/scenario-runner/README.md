Run client-side scenarios driven by load-generator.

# Usage

Run [LoadGenerator](../../crates/load_generator/README.md). LoadGenerator will
provision a backend and start ScenarioRunner with the given parameters.

## Adding new scenarios

To add a new scenario,

1. Name the scenario and add it to `ScenarioName` and the `main` control flow in
   `index.ts`.
2. Write a class that implements the `IScenario` interface and extends the
   `Scenario` class and drop the class in the `scenarios` folder. Run this new
   scenario from the `main` control flow.
3. In LoadGenerator, add a new scenario to the `Scenario` struct.

## Standalone durable workflow stress test

Deploy this project, including its mounted Workflow and Workpool components, to
a disposable backend. Then run:

```sh
node benchmark-workflows.mjs \
  http://127.0.0.1:3210 \
  24 60 15 4 3 256 500 100 180 true 2
```

The workload keeps 24 roots in flight. Each root fans out over three levels (4 +
16 + 64 nodes), alternates V8 actions and mutations, uses indexed reads and
three application tables, sends the 64-wide final level through the local Node
executor pool, then performs a validated fan-in. The final `true` cleans up both
Workflow-component state and application records after metrics have been
captured; `2` ramps measured admission over two seconds.
