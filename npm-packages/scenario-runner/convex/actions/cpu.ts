"use node";

import { action } from "../_generated/server";

// A bounded CPU-only action for measuring local Node executor parallelism.
export const spin = action({
  args: {},
  handler: (): number => {
    const deadline = performance.now() + 50;
    let value = 0x12345678;
    while (performance.now() < deadline) {
      value = Math.imul(value ^ (value >>> 13), 0x5bd1e995);
    }
    return value;
  },
});
