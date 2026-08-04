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
    expect(stepIndex(job, "Record the receipt source identity")).toBeLessThan(
      stepIndex(job, "Clippy")
    );
    expect(stepIndex(job, "Clippy")).toBeLessThan(
      stepIndex(job, "Build and verify the lint receipt")
    );
    expect(workflowStep(job, "Clippy").run).toContain("--message-format=json");
    expect(workflowStep(job, "Clippy").run).toContain("clippy.json");
    expect(workflowStep(job, "Clippy").run).not.toContain("2>&1");
    expect(workflowStep(job, "Build and verify the lint receipt").run).toContain(
      "aex-release-tool -- evidence new-cargo"
    );
    expect(workflowStep(job, "Build and verify the lint receipt").run).toContain(
      "aex-release-tool -- evidence attach"
    );
    expect(workflowStep(job, "Build and verify the lint receipt").run).toContain(
      "aex-release-tool -- evidence verify"
    );
    expect(workflowStep(job, "Build and verify the lint receipt").run).toContain(
      "artifact://lint-${RECEIPT_LANE}-${SELECTED_PACKAGE}-${PARTITION_INDEX}/clippy.json"
    );
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
    expect(source).toContain('class: "lint"');
    expect(source).toContain('packages: [$package]');
    expect(source).toContain(
      "LINT_RECEIPT: target/aex-evidence/${{ matrix.name }}/receipt-lint-${{ matrix.name }}-${{ matrix.partition }}.json"
    );
    expect(source).toContain("name: receipt-${{ inputs.lane }}-rust-lint-");
    expect(source).toContain("name: lint-${{ inputs.lane }}-${{ matrix.name }}-");
    expect(source).toContain("name: receipt-${{ inputs.lane }}-rust-");
    expect(source).not.toContain("composition-inputs");
    expect(source).not.toContain('class: "deny"');
    expect(source).not.toContain('class: "sbom"');
    expect(source).not.toContain('class: "license"');
    expect(source).not.toContain('class: "vulnerability"');
    expect(source).not.toContain('class: "package-integrity"');
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

  test("the Node lane derives and verifies Stripe unit receipts from two real Bun reports", () => {
    const source = readRepoFile(".github/workflows/_node-lane.yml");
    const workflow = readWorkflow(".github/workflows/_node-lane.yml");
    const job = workflowJob(workflow, "unit");
    const workflowCall = workflowTriggers(workflow).workflow_call as {
      readonly inputs: Readonly<Record<string, { readonly required?: boolean }>>;
    };

    expect(workflowCall.inputs.matrix?.required).toBeTrue();
    expect(workflowCall.inputs.job_name?.required).toBeTrue();
    expect(workflowCall.inputs.selection_mode?.required).toBeTrue();
    expect(job.strategy?.["fail-fast"]).toBeFalse();
    expect(stepIndex(job, "Record the declared test inventory")).toBeLessThan(
      stepIndex(job, "Test the selected package")
    );
    expect(stepIndex(job, "Test the selected package")).toBeLessThan(
      stepIndex(job, "Build and verify the unit receipt")
    );
    expect(workflowStep(job, "Record the declared test inventory").run).toContain(
      "--reporter=junit"
    );
    expect(workflowStep(job, "Record the declared test inventory").run).toContain(
      "assert-no-skips.mjs"
    );
    expect(workflowStep(job, "Test the selected package").run).toContain(
      "assert-no-skips.mjs"
    );
    expect(workflowStep(job, "Build and verify the unit receipt").run).toContain(
      "aex-release-tool -- evidence new"
    );
    expect(workflowStep(job, "Build and verify the unit receipt").run).toContain(
      "aex-release-tool -- evidence verify"
    );
    expect(source).toContain("SELECTED_PACKAGE: ${{ matrix.name }}");
    expect(source).toContain("SELECTED_PACKAGE_DIR: ${{ matrix.directory }}");
    expect(source).toContain("SELECTED_UNITS_JSON: ${{ toJSON(matrix.units) }}");
    expect(source).toContain("PARTITION_INDEX: ${{ matrix.partition }}");
    expect(source).toContain("PARTITION_TOTAL: ${{ matrix.partitions }}");
    expect(source).toContain("commitSha: $commitSha");
    expect(source).toContain("workflowRunId: $workflowRunId");
    expect(source).toContain("runAttempt: $runAttempt");
    expect(source).toContain("subject: {unitIds: $unitIds}");
    expect(source).toContain("name: receipt-${{ inputs.lane }}-node-${{ matrix.partition }}");
    expect(source).not.toContain("composition-inputs");
    expect(source).not.toContain('class: "sbom"');
    expect(source).not.toContain('class: "license"');
    expect(source).not.toContain('class: "vulnerability"');
  });

  test("every Node caller passes the release-derived matrix, aggregate job id, and routing mode", () => {
    const cases = [
      [".github/workflows/pr.yml", "node", "affected"],
      [".github/workflows/main.yml", "node", "full"]
    ] as const;

    for (const [path, jobId, mode] of cases) {
      const job = workflowJob(readWorkflow(path), jobId);
      const inputs = job.with as Readonly<Record<string, unknown>>;
      expect(inputs.matrix).toBe("${{ needs.route.outputs.node_matrix }}");
      expect(inputs.job_name).toBe(jobId);
      expect(inputs.selection_mode).toBe(mode);
    }
  });

  test("the router derives the Node matrix from release metadata", () => {
    const source = readRepoFile(".github/workflows/_route.yml");
    const workflow = readWorkflow(".github/workflows/_route.yml");
    const job = workflowJob(workflow, "route");

    expect(source).toContain("node_matrix:");
    expect(source).toContain("has_node:");
    expect(workflowStep(job, "Node matrix").run).toContain("--kind node");
  });

  test("the Terraform lane derives and verifies a receipt from one real test run", () => {
    const source = readRepoFile(".github/workflows/_terraform-lane.yml");
    const workflow = readWorkflow(".github/workflows/_terraform-lane.yml");
    const job = workflowJob(workflow, "terraform");
    const workflowCall = workflowTriggers(workflow).workflow_call as {
      readonly inputs: Readonly<Record<string, { readonly required?: boolean }>>;
    };

    expect(workflowCall.inputs.job_name?.required).toBeTrue();
    expect(workflowCall.inputs.selection_mode?.required).toBeTrue();
    expect(stepIndex(job, "Record the test identity")).toBeLessThan(
      stepIndex(job, "Test against mocked providers")
    );
    expect(stepIndex(job, "Test against mocked providers")).toBeLessThan(
      stepIndex(job, "Prove the complete test inventory")
    );
    expect(stepIndex(job, "Prove the complete test inventory")).toBeLessThan(
      stepIndex(job, "Build and verify the Terraform receipt")
    );
    expect(workflowStep(job, "Test against mocked providers").run).toContain(
      "terraform test -json"
    );
    expect(workflowStep(job, "Test against mocked providers").run).toContain("-junit-xml=");
    expect(workflowStep(job, "Prove the complete test inventory").run).toContain(
      'type == "test_abstract"'
    );
    expect(workflowStep(job, "Prove the complete test inventory").run).toContain(
      'test_run.progress == "complete"'
    );
    expect(workflowStep(job, "Prove the complete test inventory").run).toContain(
      'type == "test_summary"'
    );
    expect(workflowStep(job, "Prove the complete test inventory").run).toContain(
      'type == "test_cleanup"'
    );
    expect(workflowStep(job, "Build and verify the Terraform receipt").run).toContain(
      "aex-release-tool -- evidence new"
    );
    expect(workflowStep(job, "Build and verify the Terraform receipt").run).toContain(
      "aex-release-tool -- evidence verify"
    );
    expect(source).toContain("commitSha: $commitSha");
    expect(source).toContain("workflowRunId: $workflowRunId");
    expect(source).toContain("runAttempt: $runAttempt");
    expect(source).toContain("TERRAFORM_NODE: ${{ matrix.id }}");
    expect(source).toContain("jobName: $jobName");
  });

  test("every Terraform caller supplies its aggregate job id and routing mode", () => {
    const cases = [
      [".github/workflows/pr.yml", "terraform", "affected"],
      [".github/workflows/main.yml", "terraform", "full"]
    ] as const;

    for (const [path, jobId, mode] of cases) {
      const job = workflowJob(readWorkflow(path), jobId);
      const inputs = job.with as Readonly<Record<string, unknown>>;
      expect(inputs.job_name).toBe(jobId);
      expect(inputs.selection_mode).toBe(mode);
    }
  });

  test("final checks gate all jobs but request receipts only from selected producers", () => {
    const cases = [
      {
        path: ".github/workflows/pr.yml",
        resultJobs: ["route", "gates", "rust", "node", "scenarios", "terraform", "artifacts"],
        receiptJobs: ["rust", "node", "terraform"]
      },
      {
        path: ".github/workflows/main.yml",
        resultJobs: ["route", "gates", "verify", "node", "scenarios", "terraform", "build", "manifest"],
        receiptJobs: ["verify", "node", "terraform"]
      }
    ] as const;

    for (const { path, resultJobs, receiptJobs } of cases) {
      const checks = workflowJob(readWorkflow(path), path.endsWith("pr.yml") ? "checks" : "receipts");
      const inputs = checks.with as Readonly<Record<string, string>>;
      const requiredJobResults = inputs.required_job_results;
      const receiptJobsInput = inputs.receipt_jobs;
      if (requiredJobResults === undefined || receiptJobsInput === undefined) {
        throw new Error(`${path}: final checks must declare job results and receipt producers`);
      }
      const results = JSON.parse(requiredJobResults) as Readonly<
        Record<string, { readonly result: string; readonly allowSkipped: boolean }>
      >;
      const producers = JSON.parse(receiptJobsInput) as Readonly<
        Record<string, { readonly job: string; readonly selected: string }>
      >;

      expect(Object.keys(results)).toEqual([...resultJobs]);
      expect(Object.keys(producers)).toEqual(["rust", "node", "terraform"]);
      expect(inputs).not.toHaveProperty("declared_jobs");
      for (const job of receiptJobs) {
        const producer = Object.values(producers).find((candidate) => candidate.job === job);
        expect(producer?.selected).toContain("needs.route.outputs.has_");
      }
      expect(inputs.rust_matrix).toBe("${{ needs.route.outputs.test_matrix }}");
      expect(inputs.node_matrix).toBe("${{ needs.route.outputs.node_matrix }}");
      expect(inputs.terraform_matrix).toBe("${{ needs.route.outputs.terraform_matrix }}");
    }
  });

  test("receipt aggregation validates job results before collecting selected evidence", () => {
    const source = readRepoFile(".github/workflows/_receipts.yml");
    const workflow = readWorkflow(".github/workflows/_receipts.yml");
    const job = workflowJob(workflow, "aggregate");
    const workflowCall = workflowTriggers(workflow).workflow_call as {
      readonly inputs: Readonly<Record<string, { readonly required?: boolean }>>;
    };

    expect(workflowCall.inputs.receipt_jobs?.required).toBeTrue();
    expect(workflowCall.inputs.rust_matrix?.required).toBeTrue();
    expect(workflowCall.inputs.node_matrix?.required).toBeTrue();
    expect(workflowCall.inputs.terraform_matrix?.required).toBeTrue();
    expect(workflowCall.inputs.required_job_results?.required).toBeTrue();
    expect(stepIndex(job, "Require every workflow job to pass")).toBeLessThan(
      stepIndex(job, "Declare the expected receipt job set")
    );
    expect(stepIndex(job, "Declare the expected receipt job set")).toBeLessThan(
      stepIndex(job, "Download every receipt")
    );
    expect(workflowStep(job, "Require every workflow job to pass").run).toContain(
      '.value.result == "success"'
    );
    expect(workflowStep(job, "Require every workflow job to pass").run).toContain(
      '.value.allowSkipped and .value.result == "skipped"'
    );
    expect(workflowStep(job, "Require every workflow job to pass").run).toContain(
      'aex.workflow-job-results.v1'
    );
    expect(workflowStep(job, "Require every workflow job to pass").run).toContain(
      '> required-job-results.json'
    );
    expect(workflowStep(job, "Declare the expected receipt job set").run).toContain(
      'schema: "aex.declared-producers.v1"'
    );
    expect(workflowStep(job, "Declare the expected receipt job set").run).toContain(
      "release/semantic-receipts.json"
    );
    expect(workflowStep(job, "Declare the expected receipt job set").run).toContain(
      'partition($entry); "lint")'
    );
    expect(workflowStep(job, "Declare the expected receipt job set").run).toContain(
      "{index: $entry.partition, total: $entry.partitions}"
    );
    const download = workflowStep(job, "Download every receipt");
    expect(download.with?.["merge-multiple"]).toBeFalse();
    expect(workflowStep(job, "Aggregate").run).toContain(
      "find receipts -type f -name '*.json' -print0"
    );
    expect(workflowStep(job, "Aggregate").run).not.toContain("receipts/*.json");
    expect(source).toContain("if: steps.expected.outputs.has_receipts == 'true'");
    expect(source).toContain("required-job-results.json");
    expect(source).not.toContain("no receipt was collected at all");
  });
});
