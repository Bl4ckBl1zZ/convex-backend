import { randomUUID } from "node:crypto";
import { performance } from "node:perf_hooks";

import { ConvexHttpClient } from "convex/browser";

function usage() {
  console.error(`usage:
  node benchmark-workflows.mjs URL [concurrency] [duration_seconds] [warmup_seconds] [branching_factor] [depth] [payload_bytes] [cpu_work] [poll_ms] [timeout_seconds] [cleanup] [ramp_seconds]

defaults:
  concurrency=24 duration=60 warmup=15 branching_factor=4 depth=3
  payload_bytes=256 cpu_work=500 poll_ms=100 timeout_seconds=180 cleanup=true
  ramp_seconds=2`);
}

const [
  url,
  concurrencyArg = "24",
  durationArg = "60",
  warmupArg = "15",
  branchingFactorArg = "4",
  depthArg = "3",
  payloadBytesArg = "256",
  cpuWorkArg = "500",
  pollMsArg = "100",
  timeoutArg = "180",
  cleanupArg = "true",
  rampArg = "2",
] = process.argv.slice(2);

if (!url) {
  usage();
  process.exit(2);
}

const options = {
  concurrency: Number.parseInt(concurrencyArg, 10),
  durationMs: Number.parseFloat(durationArg) * 1_000,
  warmupMs: Number.parseFloat(warmupArg) * 1_000,
  branchingFactor: Number.parseInt(branchingFactorArg, 10),
  depth: Number.parseInt(depthArg, 10),
  payloadBytes: Number.parseInt(payloadBytesArg, 10),
  cpuWork: Number.parseInt(cpuWorkArg, 10),
  pollMs: Number.parseInt(pollMsArg, 10),
  timeoutMs: Number.parseFloat(timeoutArg) * 1_000,
  cleanup: cleanupArg === "true",
  rampMs: Number.parseFloat(rampArg) * 1_000,
};

for (const [name, value] of Object.entries(options)) {
  if (name === "cleanup") {
    continue;
  }
  if (!Number.isFinite(value) || value < 0) {
    throw new Error(`invalid ${name}: ${value}`);
  }
}
if (options.concurrency < 1 || options.durationMs <= 0 || options.pollMs < 10) {
  throw new Error("concurrency and duration must be positive; poll_ms >= 10");
}

function workloadSize(branchingFactor, depth) {
  let nodes = 1;
  let levelWidth = 1;
  let v8ActionSteps = 0;
  let nodeActionSteps = 0;
  let directMutationSteps = 2; // Root seed and final fan-in.
  for (let level = 1; level <= depth; level += 1) {
    levelWidth *= branchingFactor;
    nodes += levelWidth;
    if (level % 3 === 0) {
      nodeActionSteps += levelWidth;
    } else if (level % 2 === 1) {
      v8ActionSteps += levelWidth;
    } else {
      directMutationSteps += levelWidth;
    }
  }
  return {
    nodes,
    steps: nodes + 1,
    v8ActionSteps,
    nodeActionSteps,
    directMutationSteps,
    backendFunctionExecutions:
      nodes + 1 + 2 * (v8ActionSteps + nodeActionSteps),
  };
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function isTransientAdmissionError(error) {
  const message = error instanceof Error ? error.message : String(error);
  return (
    message.includes("TooManyConcurrentRequests") ||
    message.includes("ExpiredInQueue")
  );
}

function percentile(sorted, fraction) {
  if (sorted.length === 0) {
    return null;
  }
  return sorted[
    Math.min(sorted.length - 1, Math.ceil(sorted.length * fraction) - 1)
  ];
}

function summary(values) {
  values.sort((a, b) => a - b);
  return {
    p50Ms: percentile(values, 0.5),
    p95Ms: percentile(values, 0.95),
    p99Ms: percentile(values, 0.99),
    maxMs: percentile(values, 1),
  };
}

const clients = Array.from(
  { length: options.concurrency },
  () => new ConvexHttpClient(url),
);
const allTerminalTokens = [];

async function runWorkflow(client, phase) {
  const runToken = `${phase}-${randomUUID()}`;
  const startBegan = performance.now();
  let start;
  let admissionRetries = 0;
  let admissionBackoffMs = 25;
  while (!start) {
    try {
      start = await client.mutation("workflow_benchmark:start", {
        runToken,
        branchingFactor: options.branchingFactor,
        depth: options.depth,
        payloadBytes: options.payloadBytes,
        cpuWork: options.cpuWork,
      });
    } catch (error) {
      if (
        !isTransientAdmissionError(error) ||
        performance.now() - startBegan >= options.timeoutMs
      ) {
        return {
          status: "start_error",
          runToken,
          startLatencyMs: performance.now() - startBegan,
          admissionRetries,
          error: error instanceof Error ? error.message : String(error),
        };
      }
      admissionRetries += 1;
      const jitteredBackoff = admissionBackoffMs * (0.5 + Math.random());
      await sleep(jitteredBackoff);
      admissionBackoffMs = Math.min(1_000, admissionBackoffMs * 2);
    }
  }
  const startLatencyMs = performance.now() - startBegan;
  const deadline = performance.now() + options.timeoutMs;
  let polls = 0;
  let pollErrors = 0;

  while (performance.now() < deadline) {
    await sleep(options.pollMs);
    let status;
    try {
      status = await client.query("workflow_benchmark:status", { runToken });
      polls += 1;
    } catch (error) {
      pollErrors += 1;
      if (pollErrors >= 10) {
        return {
          status: "poll_error",
          runToken,
          startLatencyMs,
          polls,
          pollErrors,
          error: error instanceof Error ? error.message : String(error),
        };
      }
      continue;
    }
    if (!status || status.status === "running") {
      continue;
    }
    allTerminalTokens.push(runToken);
    return {
      status: status.status,
      runToken,
      startLatencyMs,
      admissionRetries,
      polls,
      pollErrors,
      workflowLatencyMs:
        status.workflowCompletedAt === undefined
          ? performance.now() - startBegan
          : status.workflowCompletedAt - status.createdAt,
      workloadLatencyMs:
        status.workloadCompletedAt === undefined
          ? null
          : status.workloadCompletedAt - status.createdAt,
      expectedSteps: status.expectedSteps,
      actualNodes: status.actualNodes,
      error: status.error,
    };
  }

  try {
    await client.mutation("workflow_benchmark:cancel", { runToken });
  } catch {
    // The timeout is already the primary error.
  }
  return {
    status: "timeout",
    runToken,
    startLatencyMs,
    admissionRetries,
    polls,
    pollErrors,
    error: `workflow exceeded ${options.timeoutMs}ms`,
  };
}

async function runPhase(name, durationMs, phaseClients, rampMs) {
  const phaseStarted = performance.now();
  const admissionDeadline = phaseStarted + durationMs;
  const results = [];
  await Promise.all(
    phaseClients.map(async (client, index) => {
      if (rampMs > 0 && phaseClients.length > 1) {
        await sleep((index * rampMs) / (phaseClients.length - 1));
      }
      while (performance.now() < admissionDeadline) {
        results.push(await runWorkflow(client, name));
      }
    }),
  );
  return {
    results,
    admissionSeconds: durationMs / 1_000,
    wallSeconds: (performance.now() - phaseStarted) / 1_000,
  };
}

async function cleanupTokens(tokens) {
  const cleanup = { attempted: tokens.length, completed: 0, seconds: 0 };
  if (!options.cleanup || tokens.length === 0) {
    return cleanup;
  }
  const cleanupStarted = performance.now();
  let next = 0;
  await Promise.all(
    clients.slice(0, Math.min(clients.length, 16)).map(async (client) => {
      while (next < tokens.length) {
        const token = tokens[next];
        next += 1;
        try {
          if (
            await client.mutation("workflow_benchmark:cleanup", {
              runToken: token,
            })
          ) {
            cleanup.completed += 1;
          }
        } catch {
          // Cleanup is reported independently from timed workload failures.
        }
      }
    }),
  );
  cleanup.seconds = (performance.now() - cleanupStarted) / 1_000;
  return cleanup;
}

let progressCompleted = 0;
const progress = setInterval(() => {
  process.stderr.write(
    `workflow stress: ${progressCompleted} measured roots completed\n`,
  );
}, 5_000);

let warmup = null;
let warmupCleanup = { attempted: 0, completed: 0, seconds: 0 };
if (options.warmupMs > 0) {
  const warmupClients = clients.slice(0, Math.min(clients.length, 4));
  warmup = await runPhase(
    "warmup",
    options.warmupMs,
    warmupClients,
    Math.min(options.rampMs, options.warmupMs / 2),
  );
  const warmupFailures = warmup.results.filter(
    (result) => result.status !== "completed",
  );
  const warmupTokens = allTerminalTokens.splice(0);
  warmupCleanup = await cleanupTokens(warmupTokens);
  if (
    warmupFailures.length > 0 ||
    warmupCleanup.completed !== warmupCleanup.attempted
  ) {
    clearInterval(progress);
    throw new Error(
      `warmup was not clean: ${warmupFailures.length} failures, ` +
        `${warmupCleanup.completed}/${warmupCleanup.attempted} cleaned`,
    );
  }
}
const measuredStarted = performance.now();
const measuredPromise = runPhase(
  "measured",
  options.durationMs,
  clients,
  options.rampMs,
);
const progressWatcher = setInterval(() => {
  // Avoid coupling the hot path to logging; this is a best-effort count.
  progressCompleted = allTerminalTokens.filter((token) =>
    token.startsWith("measured-"),
  ).length;
}, 1_000);
const measured = await measuredPromise;
clearInterval(progressWatcher);
clearInterval(progress);

const completed = measured.results.filter(
  (result) => result.status === "completed",
);
const failed = measured.results.filter(
  (result) => result.status !== "completed",
);
const elapsedSeconds = (performance.now() - measuredStarted) / 1_000;
const expected = workloadSize(options.branchingFactor, options.depth);
const workflowLatencies = completed.map((result) => result.workflowLatencyMs);
const workloadLatencies = completed
  .map((result) => result.workloadLatencyMs)
  .filter((value) => value !== null);
const startLatencies = measured.results
  .map((result) => result.startLatencyMs)
  .filter((value) => value !== undefined);

const cleanup = await cleanupTokens(allTerminalTokens);

console.log(
  JSON.stringify(
    {
      url,
      workload: {
        kind: "durable-component-multilevel-fanout-fanin",
        rootConcurrency: options.concurrency,
        branchingFactor: options.branchingFactor,
        depth: options.depth,
        expectedApplicationNodesPerWorkflow: expected.nodes,
        expectedDurableStepsPerWorkflow: expected.steps,
        v8ActionStepsPerWorkflow: expected.v8ActionSteps,
        nodeActionStepsPerWorkflow: expected.nodeActionSteps,
        directMutationStepsPerWorkflow: expected.directMutationSteps,
        backendFunctionExecutionsPerWorkflow:
          expected.backendFunctionExecutions,
        payloadBytesPerNode: options.payloadBytes,
        cpuMixIterationsPerNode: options.cpuWork,
        statusPollMs: options.pollMs,
        admissionRampSeconds: options.rampMs / 1_000,
      },
      timing: {
        warmupSeconds: options.warmupMs / 1_000,
        warmupRootConcurrency: Math.min(options.concurrency, 4),
        warmupRootsCompleted:
          warmup?.results.filter((result) => result.status === "completed")
            .length ?? 0,
        warmupWallSeconds: warmup?.wallSeconds ?? 0,
        warmupCleanup,
        admissionSeconds: measured.admissionSeconds,
        wallSecondsIncludingDrain: measured.wallSeconds,
        measuredElapsedSeconds: elapsedSeconds,
        drainSeconds: Math.max(
          0,
          measured.wallSeconds - measured.admissionSeconds,
        ),
      },
      results: {
        rootsAdmitted: measured.results.length,
        rootsCompleted: completed.length,
        rootsFailed: failed.length,
        completedWorkflowsPerSecond: completed.length / elapsedSeconds,
        logicalDurableStepsCompleted: completed.length * expected.steps,
        logicalDurableStepsPerSecond:
          (completed.length * expected.steps) / elapsedSeconds,
        backendFunctionExecutionsCompleted:
          completed.length * expected.backendFunctionExecutions,
        backendFunctionExecutionsPerSecond:
          (completed.length * expected.backendFunctionExecutions) /
          elapsedSeconds,
        applicationNodesMaterialized: completed.length * expected.nodes,
        workflowLatency: summary(workflowLatencies),
        workloadFanInLatency: summary(workloadLatencies),
        startMutationLatency: summary(startLatencies),
        statusPolls: measured.results.reduce(
          (sum, result) => sum + (result.polls ?? 0),
          0,
        ),
        statusPollErrors: measured.results.reduce(
          (sum, result) => sum + (result.pollErrors ?? 0),
          0,
        ),
        admissionRetries: measured.results.reduce(
          (sum, result) => sum + (result.admissionRetries ?? 0),
          0,
        ),
      },
      failures: failed.slice(0, 20).map((result) => ({
        status: result.status,
        runToken: result.runToken,
        error: result.error,
      })),
      cleanup,
    },
    null,
    2,
  ),
);
