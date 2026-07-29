import { performance } from "node:perf_hooks";

import { ConvexHttpClient } from "convex/browser";

const [
  url,
  functionPath,
  functionType = "query",
  concurrencyArg = "80",
  durationArg = "30",
  warmupArg = "10",
  argumentMode = "empty",
] = process.argv.slice(2);

if (
  !url ||
  !functionPath ||
  !["query", "mutation", "action"].includes(functionType) ||
  !["empty", "cache-breaker"].includes(argumentMode)
) {
  console.error(
    "usage: node benchmark-http.mjs URL FUNCTION_PATH [query|mutation|action] [concurrency] [duration_seconds] [warmup_seconds] [empty|cache-breaker]",
  );
  process.exit(2);
}

const concurrency = Number.parseInt(concurrencyArg, 10);
const durationMs = Number.parseFloat(durationArg) * 1_000;
const warmupMs = Number.parseFloat(warmupArg) * 1_000;
if (
  !Number.isFinite(concurrency) ||
  concurrency < 1 ||
  !Number.isFinite(durationMs) ||
  durationMs <= 0 ||
  !Number.isFinite(warmupMs) ||
  warmupMs < 0
) {
  throw new Error("invalid numeric benchmark argument");
}

const clients = Array.from(
  { length: concurrency },
  () => new ConvexHttpClient(url),
);

async function invoke(client) {
  const args =
    argumentMode === "cache-breaker" ? { cacheBreaker: Math.random() } : {};
  return client[functionType](functionPath, args);
}

async function runPhase(duration, record) {
  const end = performance.now() + duration;
  const latencies = [];
  let completed = 0;
  let errors = 0;

  await Promise.all(
    clients.map(async (client) => {
      while (performance.now() < end) {
        const started = performance.now();
        try {
          await invoke(client);
          completed += 1;
          if (record) {
            latencies.push(performance.now() - started);
          }
        } catch (error) {
          errors += 1;
          if (errors <= 3) {
            console.error(error);
          }
        }
      }
    }),
  );
  return { completed, errors, latencies };
}

function percentile(sorted, fraction) {
  if (sorted.length === 0) {
    return null;
  }
  return sorted[
    Math.min(sorted.length - 1, Math.ceil(sorted.length * fraction) - 1)
  ];
}

if (warmupMs > 0) {
  await runPhase(warmupMs, false);
}
const started = performance.now();
const result = await runPhase(durationMs, true);
const elapsedSeconds = (performance.now() - started) / 1_000;
result.latencies.sort((a, b) => a - b);

console.log(
  JSON.stringify(
    {
      url,
      functionPath,
      functionType,
      argumentMode,
      concurrency,
      durationSeconds: elapsedSeconds,
      completed: result.completed,
      errors: result.errors,
      qps: result.completed / elapsedSeconds,
      p50Ms: percentile(result.latencies, 0.5),
      p95Ms: percentile(result.latencies, 0.95),
      p99Ms: percentile(result.latencies, 0.99),
      maxMs: percentile(result.latencies, 1),
    },
    null,
    2,
  ),
);
