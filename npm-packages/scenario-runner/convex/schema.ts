import { defineSchema, defineTable } from "convex/server";
import { v } from "convex/values";
import { EMBEDDING_SIZE } from "../types";

export default defineSchema({
  messages: defineTable({
    channel: v.string(),
    timestamp: v.number(),
    body: v.string(),
    rand: v.number(),
    ballastArray: v.array(v.number()),
  }).index("by_channel_rand", ["channel", "rand"]),
  messages_with_search: defineTable({
    channel: v.string(),
    timestamp: v.number(),
    body: v.string(),
    rand: v.number(),
    ballastArray: v.array(v.number()),
  })
    .index("by_channel_rand", ["channel", "rand"])
    .index("by_rand", ["rand"])
    .searchIndex("search_body", {
      searchField: "body",
      filterFields: ["channel"],
    }),
  openclaurd: defineTable({
    user: v.string(),
    timestamp: v.number(),
    text: v.string(),
    rand: v.number(),
    embedding: v.array(v.number()),
  })
    .index("by_rand", ["rand"])
    .vectorIndex("embedding", {
      vectorField: "embedding",
      dimensions: EMBEDDING_SIZE,
      filterFields: ["user"],
    })
    .searchIndex("search_text", {
      searchField: "text",
      filterFields: ["user"],
    }),
  workflow_benchmark_runs: defineTable({
    runToken: v.string(),
    status: v.union(
      v.literal("running"),
      v.literal("completed"),
      v.literal("failed"),
      v.literal("canceled"),
    ),
    createdAt: v.number(),
    workloadCompletedAt: v.optional(v.number()),
    workflowCompletedAt: v.optional(v.number()),
    workflowId: v.optional(v.string()),
    branchingFactor: v.number(),
    depth: v.number(),
    expectedNodes: v.number(),
    expectedSteps: v.number(),
    actualNodes: v.optional(v.number()),
    checksum: v.optional(v.number()),
    error: v.optional(v.string()),
  }).index("by_run_token", ["runToken"]),
  workflow_benchmark_ingest: defineTable({
    runToken: v.string(),
    path: v.string(),
    parentPath: v.optional(v.string()),
    level: v.number(),
    shard: v.number(),
    checksum: v.number(),
    payload: v.string(),
  })
    .index("by_run", ["runToken"])
    .index("by_run_path", ["runToken", "path"])
    .index("by_run_level_shard", ["runToken", "level", "shard"]),
  workflow_benchmark_enriched: defineTable({
    runToken: v.string(),
    path: v.string(),
    parentPath: v.optional(v.string()),
    level: v.number(),
    shard: v.number(),
    checksum: v.number(),
    payload: v.string(),
  })
    .index("by_run", ["runToken"])
    .index("by_run_path", ["runToken", "path"])
    .index("by_run_level_shard", ["runToken", "level", "shard"]),
  workflow_benchmark_materialized: defineTable({
    runToken: v.string(),
    path: v.string(),
    parentPath: v.optional(v.string()),
    level: v.number(),
    shard: v.number(),
    checksum: v.number(),
    payload: v.string(),
  })
    .index("by_run", ["runToken"])
    .index("by_run_path", ["runToken", "path"])
    .index("by_run_level_shard", ["runToken", "level", "shard"]),
  workflow_benchmark_aggregates: defineTable({
    runToken: v.string(),
    completedAt: v.number(),
    actualNodes: v.number(),
    expectedNodes: v.number(),
    checksum: v.number(),
  }).index("by_run_token", ["runToken"]),
});
