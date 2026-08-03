import { describe, expect, test } from "bun:test";
import {
  readRepoFile,
  readWorkflow,
  stepIndex,
  workflowJob,
  workflowStep,
  workflowTriggers
} from "./workflow-test-helpers.js";

describe("workflow evidence producers", () => {
  test("the Rust lane derives and verifies a receipt from real runner output", () => {
    const source = readRepoFile(".github/workflows/_rust-lane.yml");
    const workflow = readWorkflow(".github/workflows/_rust-lane.yml");
    const job = workflowJob(workflow, "test");
    const workflowCall = workflowTriggers(workflow).workflow_call as {
      readonly inputs: Readonly<Record<string, { readonly required?: boolean }>>;
    };

    expect(workflowCall.inputs.job_name?.required).toBeTrue();
    expect(workflowCall.inputs.selection_mode?.required).toBeTrue();
    expect(stepIndex(job, "Record the declared test inventory")).toBeLessThan(stepIndex(job, "Test"));
    expect(stepIndex(job, "Test")).toBeLessThan(stepIndex(job, "Prove the no-skip inventory"));
    expect(stepIndex(job, "Prove the no-skip inventory")).toBeLessThan(
      stepIndex(job, "Build and verify the unit receipt")
    );
    expect(workflowStep(job, "Record the declared test inventory").run).toContain(
      "cargo nextest list"
    );
    expect(workflowStep(job, "Prove the no-skip inventory").run).toContain(
      "aex-workspace-check -- flake scan"
    );
    expect(workflowStep(job, "Build and verify the unit receipt").run).toContain(
      "aex-release-tool -- evidence new"
    );
    expect(workflowStep(job, "Build and verify the unit receipt").run).toContain(
      "aex-release-tool -- evidence verify"
    );
    expect(source).toContain("commitSha: $commitSha");
    expect(source).toContain("workflowRunId: $workflowRunId");
    expect(source).toContain("runAttempt: $runAttempt");
    expect(source).toContain("subject: {unitIds: $unitIds}");
    expect(source).toContain("SELECTED_UNITS_JSON: ${{ toJSON(matrix.units) }}");
    expect(source).toContain("name: receipt-${{ inputs.lane }}-rust-");
    expect(source).not.toContain("composition-inputs");
  });

  test("every Rust caller supplies its aggregate job id and routing mode", () => {
    const cases = [
      [".github/workflows/pr.yml", "rust", "affected"],
      [".github/workflows/main.yml", "verify", "full"],
      [".github/workflows/assurance.yml", "full-graph", "shadow"]
    ] as const;

    for (const [path, jobId, mode] of cases) {
      const job = workflowJob(readWorkflow(path), jobId);
      const inputs = job.with as Readonly<Record<string, unknown>>;
      expect(inputs.job_name).toBe(jobId);
      expect(inputs.selection_mode).toBe(mode);
    }
  });
});
