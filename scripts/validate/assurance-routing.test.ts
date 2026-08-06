import { describe, expect, test } from "bun:test";

import { readWorkflow, workflowJob } from "./workflow-test-helpers.js";

describe("scheduled assurance routing", () => {
  test("full-graph uses an explicit full-suite selection when schedule has no diff", () => {
    const workflow = readWorkflow(".github/workflows/assurance.yml");
    const route = workflowJob(workflow, "route");
    const routeInputs = route.with as Readonly<Record<string, unknown>>;
    const fullGraph = workflowJob(workflow, "full-graph");
    const fullGraphInputs = fullGraph.with as Readonly<Record<string, unknown>>;

    expect(routeInputs.base_sha).toBe("${{ github.sha }}");
    expect(routeInputs.head_sha).toBe("${{ github.sha }}");
    expect(routeInputs.mode).toBe("full");
    expect(routeInputs.artifact_mode).toBe("full");
    expect(fullGraphInputs.selection_mode).toBe("full");
  });
});
