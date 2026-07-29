import {
  WorkflowManager,
  type WorkflowId,
  vWorkflowId,
} from "@convex-dev/workflow";
import { v } from "convex/values";
import { components, internal } from "./_generated/api";
import {
  action,
  internalAction,
  internalMutation,
  internalQuery,
  mutation,
  query,
  type DatabaseReader,
  type DatabaseWriter,
} from "./_generated/server";
import { mixChecksum } from "./workflow_benchmark_shared";

const workflow = new WorkflowManager(components.workflow, {
  workpoolOptions: {
    // @convex-dev/workpool 0.3.0 warns above its soft limit of 50 and rejects
    // values above its hard limit of 100. Use the true ceiling so concurrent
    // roots and 64-wide levels create a sustained queue behind that bound.
    maxParallelism: 100,
  },
});

const nodeArgs = {
  runToken: v.string(),
  path: v.string(),
  parentPath: v.string(),
  parentLevel: v.number(),
  level: v.number(),
  shard: v.number(),
  ordinal: v.number(),
  payloadBytes: v.number(),
  cpuWork: v.number(),
};

const nodeResult = v.object({
  path: v.string(),
  level: v.number(),
  checksum: v.number(),
});

type NodeResult = {
  path: string;
  level: number;
  checksum: number;
};

function workloadSize(branchingFactor: number, depth: number) {
  let nodes = 1;
  let levelWidth = 1;
  for (let level = 1; level <= depth; level += 1) {
    levelWidth *= branchingFactor;
    nodes += levelWidth;
  }
  return {
    nodes,
    // One durable step seeds the root, every non-root node is one step, and
    // one final step performs the fan-in.
    steps: nodes + 1,
  };
}

function validateShape(
  branchingFactor: number,
  depth: number,
  payloadBytes: number,
  cpuWork: number,
) {
  if (
    !Number.isInteger(branchingFactor) ||
    branchingFactor < 2 ||
    branchingFactor > 8
  ) {
    throw new Error("branchingFactor must be an integer from 2 through 8");
  }
  if (!Number.isInteger(depth) || depth < 2 || depth > 5) {
    throw new Error("depth must be an integer from 2 through 5");
  }
  if (
    !Number.isInteger(payloadBytes) ||
    payloadBytes < 0 ||
    payloadBytes > 4096
  ) {
    throw new Error("payloadBytes must be an integer from 0 through 4096");
  }
  if (!Number.isInteger(cpuWork) || cpuWork < 0 || cpuWork > 100_000) {
    throw new Error("cpuWork must be an integer from 0 through 100000");
  }
  const size = workloadSize(branchingFactor, depth);
  if (size.nodes > 600) {
    throw new Error(
      `workflow shape has ${size.nodes} nodes; the benchmark limit is 600`,
    );
  }
  return size;
}

async function readNode(
  db: DatabaseReader,
  runToken: string,
  path: string,
  level: number,
) {
  switch (level % 3) {
    case 0:
      return await db
        .query("workflow_benchmark_ingest")
        .withIndex("by_run_path", (q) =>
          q.eq("runToken", runToken).eq("path", path),
        )
        .unique();
    case 1:
      return await db
        .query("workflow_benchmark_enriched")
        .withIndex("by_run_path", (q) =>
          q.eq("runToken", runToken).eq("path", path),
        )
        .unique();
    default:
      return await db
        .query("workflow_benchmark_materialized")
        .withIndex("by_run_path", (q) =>
          q.eq("runToken", runToken).eq("path", path),
        )
        .unique();
  }
}

async function insertNode(
  db: DatabaseWriter,
  node: {
    runToken: string;
    path: string;
    parentPath?: string;
    level: number;
    shard: number;
    checksum: number;
    payload: string;
  },
) {
  // Workflow actions are retried durably. Make the application-side effect
  // idempotent as production workflow code should be: a retry after the first
  // mutation committed returns the same logical node instead of duplicating
  // the fan-out tree.
  const existing = await readNode(db, node.runToken, node.path, node.level);
  if (existing) {
    if (
      existing.parentPath !== node.parentPath ||
      existing.checksum !== node.checksum
    ) {
      throw new Error(`conflicting retry for ${node.runToken}/${node.path}`);
    }
    return existing._id;
  }
  switch (node.level % 3) {
    case 0:
      return await db.insert("workflow_benchmark_ingest", node);
    case 1:
      return await db.insert("workflow_benchmark_enriched", node);
    default:
      return await db.insert("workflow_benchmark_materialized", node);
  }
}

export const seedRoot = internalMutation({
  args: {
    runToken: v.string(),
    payloadBytes: v.number(),
  },
  returns: nodeResult,
  handler: async (ctx, args): Promise<NodeResult> => {
    const checksum = mixChecksum(0, 0, args.runToken.length, 32);
    await insertNode(ctx.db, {
      runToken: args.runToken,
      path: "root",
      level: 0,
      shard: 0,
      checksum,
      payload: "r".repeat(args.payloadBytes),
    });
    return { path: "root", level: 0, checksum };
  },
});

export const loadParent = internalQuery({
  args: {
    runToken: v.string(),
    path: v.string(),
    level: v.number(),
  },
  returns: nodeResult,
  handler: async (ctx, args): Promise<NodeResult> => {
    const node = await readNode(ctx.db, args.runToken, args.path, args.level);
    if (!node) {
      throw new Error(`missing parent ${args.runToken}/${args.path}`);
    }
    return {
      path: node.path,
      level: node.level,
      checksum: node.checksum,
    };
  },
});

export const persistActionNode = internalMutation({
  args: {
    runToken: v.string(),
    path: v.string(),
    parentPath: v.string(),
    level: v.number(),
    shard: v.number(),
    checksum: v.number(),
    payloadBytes: v.number(),
  },
  returns: nodeResult,
  handler: async (ctx, args): Promise<NodeResult> => {
    await insertNode(ctx.db, {
      runToken: args.runToken,
      path: args.path,
      parentPath: args.parentPath,
      level: args.level,
      shard: args.shard,
      checksum: args.checksum,
      payload: "a".repeat(args.payloadBytes),
    });
    return {
      path: args.path,
      level: args.level,
      checksum: args.checksum,
    };
  },
});

export const processNodeAction = internalAction({
  args: nodeArgs,
  returns: nodeResult,
  handler: async (ctx, args): Promise<NodeResult> => {
    const parent: NodeResult = await ctx.runQuery(
      internal.workflow_benchmark.loadParent,
      {
        runToken: args.runToken,
        path: args.parentPath,
        level: args.parentLevel,
      },
    );
    const checksum = mixChecksum(
      parent.checksum,
      args.level,
      args.ordinal,
      args.cpuWork,
    );
    return await ctx.runMutation(
      internal.workflow_benchmark.persistActionNode,
      {
        runToken: args.runToken,
        path: args.path,
        parentPath: args.parentPath,
        level: args.level,
        shard: args.shard,
        checksum,
        payloadBytes: args.payloadBytes,
      },
    );
  },
});

export const processNodeMutation = internalMutation({
  args: nodeArgs,
  returns: nodeResult,
  handler: async (ctx, args): Promise<NodeResult> => {
    const parent = await readNode(
      ctx.db,
      args.runToken,
      args.parentPath,
      args.parentLevel,
    );
    if (!parent) {
      throw new Error(`missing parent ${args.runToken}/${args.parentPath}`);
    }
    const checksum = mixChecksum(
      parent.checksum,
      args.level,
      args.ordinal,
      args.cpuWork,
    );
    await insertNode(ctx.db, {
      runToken: args.runToken,
      path: args.path,
      parentPath: args.parentPath,
      level: args.level,
      shard: args.shard,
      checksum,
      payload: "m".repeat(args.payloadBytes),
    });
    return { path: args.path, level: args.level, checksum };
  },
});

export const finalize = internalMutation({
  args: {
    runToken: v.string(),
    expectedNodes: v.number(),
  },
  returns: v.object({
    actualNodes: v.number(),
    expectedNodes: v.number(),
    checksum: v.number(),
  }),
  handler: async (
    ctx,
    args,
  ): Promise<{
    actualNodes: number;
    expectedNodes: number;
    checksum: number;
  }> => {
    // These scans intentionally span independent tables at one consistent
    // snapshot, matching a real fan-in/materialization phase.
    const [ingest, enriched, materialized] = await Promise.all([
      ctx.db
        .query("workflow_benchmark_ingest")
        .withIndex("by_run", (q) => q.eq("runToken", args.runToken))
        .collect(),
      ctx.db
        .query("workflow_benchmark_enriched")
        .withIndex("by_run", (q) => q.eq("runToken", args.runToken))
        .collect(),
      ctx.db
        .query("workflow_benchmark_materialized")
        .withIndex("by_run", (q) => q.eq("runToken", args.runToken))
        .collect(),
    ]);
    const nodes = [...ingest, ...enriched, ...materialized];
    const actualNodes = nodes.length;
    if (actualNodes !== args.expectedNodes) {
      throw new Error(
        `fan-in saw ${actualNodes} nodes, expected ${args.expectedNodes}`,
      );
    }
    const checksum = nodes.reduce(
      (sum, node) => (sum + node.checksum) % 9_007_199_254_740_881,
      0,
    );
    const completedAt = Date.now();
    await ctx.db.insert("workflow_benchmark_aggregates", {
      runToken: args.runToken,
      completedAt,
      actualNodes,
      expectedNodes: args.expectedNodes,
      checksum,
    });
    const run = await ctx.db
      .query("workflow_benchmark_runs")
      .withIndex("by_run_token", (q) => q.eq("runToken", args.runToken))
      .unique();
    if (!run) {
      throw new Error(`missing benchmark run ${args.runToken}`);
    }
    await ctx.db.patch(run._id, {
      workloadCompletedAt: completedAt,
      actualNodes,
      checksum,
    });
    return { actualNodes, expectedNodes: args.expectedNodes, checksum };
  },
});

export const fanoutWorkflow = workflow.define({
  args: {
    runToken: v.string(),
    branchingFactor: v.number(),
    depth: v.number(),
    payloadBytes: v.number(),
    cpuWork: v.number(),
  },
  returns: v.object({
    actualNodes: v.number(),
    expectedNodes: v.number(),
    checksum: v.number(),
  }),
  handler: async (
    step,
    args,
  ): Promise<{
    actualNodes: number;
    expectedNodes: number;
    checksum: number;
  }> => {
    const size = validateShape(
      args.branchingFactor,
      args.depth,
      args.payloadBytes,
      args.cpuWork,
    );
    let parents: NodeResult[] = [
      await step.runMutation(
        internal.workflow_benchmark.seedRoot,
        {
          runToken: args.runToken,
          payloadBytes: args.payloadBytes,
        },
        { name: "level-0-seed" },
      ),
    ];

    for (let level = 1; level <= args.depth; level += 1) {
      const children = parents.flatMap((parent, parentOrdinal) =>
        Array.from({ length: args.branchingFactor }, (_, childOrdinal) => {
          const ordinal = parentOrdinal * args.branchingFactor + childOrdinal;
          return {
            runToken: args.runToken,
            path: `${parent.path}.${childOrdinal}`,
            parentPath: parent.path,
            parentLevel: parent.level,
            level,
            shard: ordinal % 16,
            ordinal,
            payloadBytes: args.payloadBytes,
            cpuWork: args.cpuWork,
          };
        }),
      );
      // Level 1 takes the V8 action -> indexed query -> mutation path, level 2
      // uses a direct indexed read/write mutation, and level 3 takes the same
      // nested path through the local Node executor pool. Each entire level is
      // submitted together, creating a real breadth-first fan-out barrier.
      parents = await Promise.all(
        children.map((child) =>
          level % 3 === 0
            ? step.runAction(
                internal.workflow_benchmark_node.processNodeAction,
                child,
                { name: `level-${level}-node-action`, retry: true },
              )
            : level % 2 === 1
              ? step.runAction(
                  internal.workflow_benchmark.processNodeAction,
                  child,
                  { name: `level-${level}-action`, retry: true },
                )
              : step.runMutation(
                  internal.workflow_benchmark.processNodeMutation,
                  child,
                  { name: `level-${level}-mutation` },
                ),
        ),
      );
    }

    return await step.runMutation(
      internal.workflow_benchmark.finalize,
      {
        runToken: args.runToken,
        expectedNodes: size.nodes,
      },
      { name: "fan-in-finalize" },
    );
  },
});

export const complete = internalMutation({
  args: {
    workflowId: vWorkflowId,
    result: v.any(),
    context: v.object({ runToken: v.string() }),
  },
  handler: async (ctx, args) => {
    const run = await ctx.db
      .query("workflow_benchmark_runs")
      .withIndex("by_run_token", (q) => q.eq("runToken", args.context.runToken))
      .unique();
    if (!run) {
      return;
    }
    const completedAt = Date.now();
    if (args.result.kind === "success") {
      await ctx.db.patch(run._id, {
        status: "completed",
        workflowCompletedAt: completedAt,
        actualNodes: args.result.returnValue.actualNodes,
        checksum: args.result.returnValue.checksum,
      });
    } else {
      await ctx.db.patch(run._id, {
        status: args.result.kind === "canceled" ? "canceled" : "failed",
        workflowCompletedAt: completedAt,
        error:
          args.result.kind === "failed"
            ? args.result.error
            : "workflow canceled",
      });
    }
  },
});

export const start = mutation({
  args: {
    runToken: v.string(),
    branchingFactor: v.number(),
    depth: v.number(),
    payloadBytes: v.number(),
    cpuWork: v.number(),
  },
  handler: async (
    ctx,
    args,
  ): Promise<{
    runToken: string;
    workflowId: WorkflowId;
    createdAt: number;
    expectedNodes: number;
    expectedSteps: number;
  }> => {
    const size = validateShape(
      args.branchingFactor,
      args.depth,
      args.payloadBytes,
      args.cpuWork,
    );
    const existing = await ctx.db
      .query("workflow_benchmark_runs")
      .withIndex("by_run_token", (q) => q.eq("runToken", args.runToken))
      .unique();
    if (existing) {
      throw new Error(`duplicate run token ${args.runToken}`);
    }
    const createdAt = Date.now();
    const runId = await ctx.db.insert("workflow_benchmark_runs", {
      runToken: args.runToken,
      status: "running",
      createdAt,
      branchingFactor: args.branchingFactor,
      depth: args.depth,
      expectedNodes: size.nodes,
      expectedSteps: size.steps,
    });
    const workflowId: WorkflowId = await workflow.start(
      ctx,
      internal.workflow_benchmark.fanoutWorkflow,
      args,
      {
        onComplete: internal.workflow_benchmark.complete,
        context: { runToken: args.runToken },
        startAsync: true,
      },
    );
    await ctx.db.patch(runId, { workflowId });
    return {
      runToken: args.runToken,
      workflowId,
      createdAt,
      expectedNodes: size.nodes,
      expectedSteps: size.steps,
    };
  },
});

export const status = query({
  args: { runToken: v.string() },
  handler: async (ctx, args) => {
    const run = await ctx.db
      .query("workflow_benchmark_runs")
      .withIndex("by_run_token", (q) => q.eq("runToken", args.runToken))
      .unique();
    if (!run) {
      return null;
    }
    const componentStatus =
      run.status === "running" && run.workflowId
        ? await workflow.status(ctx, run.workflowId as WorkflowId)
        : null;
    return { ...run, componentStatus };
  },
});

export const cancel = mutation({
  args: { runToken: v.string() },
  handler: async (ctx, args) => {
    const run = await ctx.db
      .query("workflow_benchmark_runs")
      .withIndex("by_run_token", (q) => q.eq("runToken", args.runToken))
      .unique();
    if (!run?.workflowId || run.status !== "running") {
      return false;
    }
    await workflow.cancel(ctx, run.workflowId as WorkflowId);
    return true;
  },
});

export const cleanup = mutation({
  args: { runToken: v.string() },
  handler: async (ctx, args) => {
    const run = await ctx.db
      .query("workflow_benchmark_runs")
      .withIndex("by_run_token", (q) => q.eq("runToken", args.runToken))
      .unique();
    if (!run || run.status === "running") {
      return false;
    }
    if (run.workflowId) {
      await workflow.cleanup(ctx, run.workflowId as WorkflowId);
    }
    const [ingest, enriched, materialized, aggregates] = await Promise.all([
      ctx.db
        .query("workflow_benchmark_ingest")
        .withIndex("by_run", (q) => q.eq("runToken", args.runToken))
        .collect(),
      ctx.db
        .query("workflow_benchmark_enriched")
        .withIndex("by_run", (q) => q.eq("runToken", args.runToken))
        .collect(),
      ctx.db
        .query("workflow_benchmark_materialized")
        .withIndex("by_run", (q) => q.eq("runToken", args.runToken))
        .collect(),
      ctx.db
        .query("workflow_benchmark_aggregates")
        .withIndex("by_run_token", (q) => q.eq("runToken", args.runToken))
        .collect(),
    ]);
    for (const doc of [
      ...ingest,
      ...enriched,
      ...materialized,
      ...aggregates,
    ]) {
      await ctx.db.delete(doc._id);
    }
    await ctx.db.delete(run._id);
    return true;
  },
});

// Keep the public action path available for one-off diagnosis from the
// dashboard without making it part of the timed benchmark.
export const inspect = action({
  args: { runToken: v.string() },
  handler: async (ctx, args): Promise<NodeResult> => {
    return await ctx.runQuery(internal.workflow_benchmark.loadParent, {
      runToken: args.runToken,
      path: "root",
      level: 0,
    });
  },
});
