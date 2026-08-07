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
});
