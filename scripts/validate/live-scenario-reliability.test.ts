import { describe, expect, it } from "bun:test";

import {
  readRepoFile,
  readWorkflow,
  workflowJob,
  workflowStepRunning
} from "./workflow-test-helpers.js";

// A whole-scenario retry can repeat externally visible effects. The strict-v1
// registry therefore retries at the operation boundary, never at Bun's test
// runner or workflow boundary.
const RETRY_FLAG = /(?:^|\s)--retry(?:[=\s]|$)/;

describe("registered scenario reliability", () => {
  it("keeps whole-scenario retries disabled in every user-test entry point", () => {
    const scripts = Object.entries(
      (JSON.parse(readRepoFile("apps/user-tests/package.json")) as {
        scripts?: Record<string, string>;
      }).scripts ?? {}
    ).filter(([name]) => name.startsWith("test:user"));

    expect(scripts.length).toBeGreaterThan(0);
    for (const [name, command] of scripts) expect(command, name).not.toMatch(RETRY_FLAG);
    expect(readRepoFile("apps/user-tests/bunfig.toml")).not.toMatch(RETRY_FLAG);
    expect(readRepoFile(".github/workflows/_scenario-lane.yml")).not.toMatch(RETRY_FLAG);
  });

  it("detects both Bun retry spellings without matching unrelated flags", () => {
    expect("bun test --isolate --retry=2 test/live").toMatch(RETRY_FLAG);
    expect("bun test --isolate --retry 2 test/live").toMatch(RETRY_FLAG);
    expect("bun test --isolate --retry").toMatch(RETRY_FLAG);
    expect("bun test --isolate --retry-free test/live").not.toMatch(RETRY_FLAG);
  });

  it("keeps live behavior behind the typed registered suite", () => {
    const source = readRepoFile("apps/user-tests/test/live/registered.test.ts");
    expect(source).toContain('suite }) => suite === "live"');
    expect(source).not.toMatch(/\.filter\([^;\n]*\|\|\s*true\b/);
  });

  it("statically validates each graph-selected target before post-deploy execution", () => {
    const job = workflowJob(readWorkflow(".github/workflows/_scenario-lane.yml"), "scenario");
    const step = workflowStepRunning(job, /SCENARIO_TARGET/);

    expect(job.strategy?.["fail-fast"]).toBe(false);
    expect(job.strategy?.matrix).toBe("${{ fromJSON(inputs.matrix) }}");
    expect(step.run).toContain('scripts?.[process.env.SCRIPT_NAME]');
    expect(step.run).toContain("cargo nextest list --locked --profile live");
    expect(step.run).toContain("--features live");
    expect(step.run).toContain("--ignore-default-filter");
    expect(step.run).toContain('."test-count" > 0');
    expect(step.run).not.toContain("cargo nextest run");
    expect(step.run).not.toContain("bun --filter");
    expect(step.run).not.toContain("--profile ci");
    expect(step.run).not.toMatch(/--shard(?:\s|$)/);
    expect(step.run).not.toMatch(RETRY_FLAG);
  });

  it("makes release routing reject an empty runnable scenario matrix", () => {
    const job = workflowJob(readWorkflow(".github/workflows/_route.yml"), "route");
    const step = workflowStepRunning(job, /ROUTING_LANE/);

    expect(step.env?.ROUTING_LANE).toBe("${{ inputs.lane }}");
    expect(step.run).toContain('[[ "$ROUTING_LANE" == "release" ]]');
    expect(step.run).toContain("graph verify --release");
  });
});
