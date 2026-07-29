"use node";

import { v } from "convex/values";
import { internal } from "./_generated/api";
import { internalAction } from "./_generated/server";
import { mixChecksum } from "./workflow_benchmark_shared";

type NodeResult = {
  path: string;
  level: number;
  checksum: number;
};

export const processNodeAction = internalAction({
  args: {
    runToken: v.string(),
    path: v.string(),
    parentPath: v.string(),
    parentLevel: v.number(),
    level: v.number(),
    shard: v.number(),
    ordinal: v.number(),
    payloadBytes: v.number(),
    cpuWork: v.number(),
  },
  returns: v.object({
    path: v.string(),
    level: v.number(),
    checksum: v.number(),
  }),
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
