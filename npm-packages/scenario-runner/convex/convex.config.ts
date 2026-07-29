import workflow from "@convex-dev/workflow/convex.config.js";
import { defineApp } from "convex/server";
import counterComponent from "../counterComponent/convex.config.js";

const app = defineApp();
app.use(counterComponent);
app.use(workflow);
export default app;
