import { readdirSync } from "node:fs";
import { describe, expect, test } from "bun:test";

import {
  jobNeeds,
  readRepoFile,
  readWorkflow,
  stepIndex,
  workflowJob,
  workflowStep,
  workflowSteps,
  workflowTriggers
} from "./workflow-test-helpers.js";

const workflowPaths = readdirSync(new URL("../../.github/workflows/", import.meta.url))
  .filter((name) => name.endsWith(".yml") || name.endsWith(".yaml"))
  .map((name) => `.github/workflows/${name}`)
  .sort();

const uploadArtifact =
  "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a";
const downloadArtifact =
  "actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c";

describe("workflow evidence producers", () => {
  test("artifact transfers use the current pinned action generations", () => {
    const transfers = workflowPaths.flatMap((path) =>
      Object.values(readWorkflow(path).jobs ?? {})
        .flatMap((job) => workflowSteps(job))
        .flatMap((step) => step.uses?.startsWith("actions/upload-artifact@") ||
          step.uses?.startsWith("actions/download-artifact@")
          ? [{ path, uses: step.uses }]
          : [])
    );
    const uploads = transfers.filter(({ uses }) => uses.startsWith("actions/upload-artifact@"));
    const downloads = transfers.filter(({ uses }) => uses.startsWith("actions/download-artifact@"));

    expect(uploads.length).toBeGreaterThan(0);
    expect(downloads.length).toBeGreaterThan(0);
    expect(uploads.filter(({ uses }) => uses !== uploadArtifact)).toEqual([]);
    expect(downloads.filter(({ uses }) => uses !== downloadArtifact)).toEqual([]);

    for (const path of workflowPaths) {
      expect(readRepoFile(path), path).not.toContain("assurance");
    }
  });

  test("nextest is exact, verified, and downloaded only once per matrix lane", () => {
    const download = downloadArtifact;

    for (const [path, consumerId, artifactName] of [
      [
        ".github/workflows/_rust-lane.yml",
        "test",
        "cargo-nextest-${{ inputs.lane }}-${{ inputs.job_name }}-${{ github.run_id }}"
      ],
      [
        ".github/workflows/_rust-lane.yml",
        "capacity-limit-projection",
        "cargo-nextest-${{ inputs.lane }}-${{ inputs.job_name }}-${{ github.run_id }}"
      ],
      [
        ".github/workflows/_scenario-lane.yml",
        "scenario",
        "cargo-nextest-${{ inputs.lane }}-scenario-${{ github.run_id }}"
      ]
    ] as const) {
      const workflow = readWorkflow(path);
      const producer = workflowJob(workflow, "nextest");
      const consumer = workflowJob(workflow, consumerId);
      const build = workflowStep(producer, "Build, stage, and verify exact nextest");
      const upload = workflowStep(producer, "Upload pinned nextest");
      const acquire = workflowStep(consumer, "Download pinned nextest");
      const verify = workflowStep(consumer, "Use pinned nextest");

      expect(build.run).toContain("cargo install --locked --version 0.9.108 cargo-nextest");
      expect(build.run).toContain("cargo-nextest 0\\.9\\.108");
      expect(upload.with?.name).toBe(artifactName);
      // A partial re-run does not re-execute a producer that already
      // succeeded, so an attempt-scoped name would leave the consumer with
      // nothing to download; `overwrite` covers the full re-run that does.
      expect(upload.with?.name, path).not.toContain("run_attempt");
      expect(upload.with?.overwrite, path).toBeTrue();
      expect(jobNeeds(consumer)).toContain("nextest");
      expect(acquire.uses).toBe(download);
      expect(acquire.with?.name).toBe(artifactName);
      expect(verify.run).toContain("sha256sum --check cargo-nextest.sha256");
      expect(verify.run).toContain("cargo-nextest 0\\.9\\.108");
      expect(workflowSteps(consumer).some((step) => step.uses?.startsWith("taiki-e/"))).toBeFalse();
    }

    const evidence = workflowJob(readWorkflow(".github/workflows/release-evidence.yml"), "evidence");
    const build = workflowStep(evidence, "Build and verify exact nextest");
    expect(build.run).toContain("cargo install --locked --version 0.9.108 cargo-nextest");
    expect(build.run).toContain("cargo-nextest 0\\.9\\.108");
  });

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
      "aex-release-tool evidence new-cargo"
    );
    expect(workflowStep(job, "Build and verify the lint receipt").run).toContain(
      "aex-release-tool evidence attach"
    );
    expect(workflowStep(job, "Build and verify the lint receipt").run).toContain(
      "aex-release-tool evidence verify"
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
    expect(workflowStep(job, "Doctests").run).toContain("--color never");
    expect(workflowStep(job, "Doctests").run).toContain("cargo metadata --locked --no-deps");
    expect(workflowStep(job, "Doctests").run).toContain('[[ "$package_count" == "1" ]]');
    expect(workflowStep(job, "Doctests").run).toContain('[[ "$has_library" == "true" ]]');
    expect(workflowStep(job, "Doctests").run).toContain("has no library target");
    expect(workflowStep(job, "Prove the no-skip inventory").run).toContain(
      "aex-workspace-check flake scan"
    );
    expect(workflowStep(job, "Prove the no-skip inventory").run).toContain(
      "--default-features-only"
    );
    expect(workflowStep(job, "Build and verify the unit receipt").run).toContain(
      "aex-release-tool evidence new"
    );
    expect(workflowStep(job, "Build and verify the unit receipt").run).toContain(
      "aex-release-tool evidence verify"
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
      [".github/workflows/main.yml", "verify", "affected"]
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
    const aggregateJob = workflowJob(workflow, "node");
    const job = workflowJob(workflow, "unit");
    const workflowCall = workflowTriggers(workflow).workflow_call as {
      readonly inputs: Readonly<Record<string, { readonly required?: boolean }>>;
    };

    expect(workflowCall.inputs.matrix?.required).toBeTrue();
    expect(workflowCall.inputs.job_name?.required).toBeTrue();
    expect(workflowCall.inputs.selection_mode?.required).toBeTrue();
    expect(
      workflowSteps(aggregateJob).some((step) => step.with?.path === "reports/junit.xml")
    ).toBeFalse();
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
      "aex-release-tool evidence new"
    );
    expect(workflowStep(job, "Build and verify the unit receipt").run).toContain(
      "aex-release-tool evidence verify"
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
      [".github/workflows/main.yml", "node", "affected"]
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

  test("main routes changed-package checks separately from the exhaustive artifact build", () => {
    const route = readWorkflow(".github/workflows/_route.yml");
    const routeCall = workflowTriggers(route).workflow_call as {
      readonly inputs: Readonly<Record<string, { readonly required?: boolean }>>;
    };
    const routeJob = workflowJob(route, "route");
    const mainRoute = workflowJob(readWorkflow(".github/workflows/main.yml"), "route");
    const mainInputs = mainRoute.with as Readonly<Record<string, unknown>>;

    expect(routeCall.inputs.artifact_mode?.required).toBeTrue();
    expect(mainInputs.mode).toBe("affected");
    expect(mainInputs.artifact_mode).toBe("full");
    expect(workflowStep(routeJob, "Select").run).toContain('--mode "${{ inputs.mode }}"');
    expect(workflowStep(routeJob, "Select artifact graph").run).toContain(
      '--mode "${{ inputs.artifact_mode }}"'
    );
    expect(workflowStep(routeJob, "Artifact matrix").run).toContain(
      "--selection artifact-selection.json"
    );
    for (const step of ["Test matrix", "Node matrix"]) {
      expect(workflowStep(routeJob, step).run).toContain(
        "--artifact-selection artifact-selection.json"
      );
    }
    for (const step of ["Scenario matrix", "Terraform matrix"]) {
      expect(workflowStep(routeJob, step).run).toContain("--selection selection.json");
    }
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
      "aex-release-tool evidence new"
    );
    expect(workflowStep(job, "Build and verify the Terraform receipt").run).toContain(
      "aex-release-tool evidence verify"
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
      [".github/workflows/main.yml", "terraform", "affected"]
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
        // No `artifacts` here: the PR lane is read-only, so it cannot call
        // `_build-artifacts.yml` (which requests write scopes) without failing
        // the whole run at startup. Bytes are proved buildable on main.
        resultJobs: ["preflight", "tools", "route", "gates", "rust", "integration", "node", "scenarios", "terraform"],
        receiptJobs: ["rust", "node", "terraform"]
      },
      {
        path: ".github/workflows/main.yml",
        // `compile` gates nothing but is required to have SUCCEEDED: it is the
        // only producer of the bytes `build` publishes, so a red compile that
        // was merely tolerated would publish a shorter release.
        resultJobs: ["preflight", "tools", "route", "gates", "verify", "integration", "node", "scenarios", "terraform", "compile", "build", "manifest"],
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
      // `integration` is deliberately absent from the producers: it gates (a red
      // engine lane fails the run) but emits no evidence receipt, because a
      // semantic receipt is keyed to a deployable unit's own package. The lane
      // contains libraries and the owner-invoked `central-schema-admin` tool,
      // none of which has a unit row. Giving non-unit evidence a seat in the ledger is a
      // `release/units.toml` decision, so until it is taken this asymmetry is
      // recorded here rather than left to be discovered.
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

  test("the tool-independent gates run unblocked and download no workspace tools", () => {
    // `Format` and `Secret scan` need no prebuilt binary. Holding them behind
    // `tools` meant a formatting violation took ~3 minutes to report, ~1m46s
    // of which was a build the job never used. They own a job with no `needs:`;
    // everything left in `gates` genuinely runs a downloaded tool.
    for (const path of [".github/workflows/main.yml", ".github/workflows/pr.yml"] as const) {
      const workflow = readWorkflow(path);
      const preflight = workflowJob(workflow, "preflight");
      const gates = workflowJob(workflow, "gates");

      expect(jobNeeds(preflight), path).toEqual([]);
      expect(
        workflowSteps(preflight).map((step) => step.name),
        path
      ).toEqual(["Checkout", "Install Rust", "Format", "Secret scan"]);
      expect(workflowStep(preflight, "Format").run, path).toBe("cargo fmt --all -- --check");
      // The same pinned toolchain `gates` installs — a second pin would drift.
      expect(workflowStep(preflight, "Install Rust").uses, path).toBe(
        workflowStep(gates, "Install Rust").uses
      );
      expect(workflowStep(preflight, "Install Rust").with?.toolchain, path).toBe(
        workflowStep(gates, "Install Rust").with?.toolchain
      );
      // Downloading the tools artifact would re-couple this job to `tools`.
      expect(
        workflowSteps(preflight).some((step) =>
          step.uses?.startsWith("actions/download-artifact@")
        ),
        path
      ).toBeFalse();

      expect(jobNeeds(gates), path).toEqual(["tools"]);
      const gateSteps = workflowSteps(gates).map((step) => step.name);
      expect(gateSteps, path).not.toContain("Format");
      expect(gateSteps, path).not.toContain("Secret scan");
      // Every check left behind is one that cannot run without a built tool,
      // which is what earns `gates` its wait on `tools`.
      for (const name of [
        "Workspace structure",
        "Delivery graph",
        "Router selftest",
        "Workflow structure",
        "Terraform packages no code",
        "Janitor policy"
      ]) {
        expect(workflowStep(gates, name).run, `${path} ${name}`).toMatch(
          /^aex-(release-tool|workspace-check)\b/
        );
      }
    }
  });

  test("empty Node selections skip the lane without admitting selected Node failures", () => {
    for (const [path, checksId] of [
      [".github/workflows/pr.yml", "checks"],
      [".github/workflows/main.yml", "receipts"]
    ] as const) {
      const workflow = readWorkflow(path);
      expect(workflowJob(workflow, "node").if).toBe(
        "needs.route.outputs.has_node == 'true'"
      );

      const inputs = workflowJob(workflow, checksId).with as Readonly<Record<string, string>>;
      const results = JSON.parse(inputs.required_job_results!) as Readonly<
        Record<string, { readonly result: string; readonly allowSkipped: boolean }>
      >;
      const producers = JSON.parse(inputs.receipt_jobs!) as Readonly<
        Record<string, { readonly job: string; readonly selected: string }>
      >;
      expect(results.node).toEqual({
        result: "${{ needs.node.result }}",
        allowSkipped: true
      });
      expect(producers.node).toEqual({
        job: "node",
        selected: "${{ needs.route.outputs.has_node }}"
      });
    }

    const mainBuild = workflowJob(readWorkflow(".github/workflows/main.yml"), "build");
    expect(mainBuild.if).toContain(
      "(needs.node.result == 'success' || needs.node.result == 'skipped')"
    );
    expect(mainBuild.if).not.toContain("needs.node.result != 'failure'");
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
    expect(stepIndex(job, "Capture the required workflow job results")).toBeLessThan(
      stepIndex(job, "Require every workflow job to pass")
    );
    expect(stepIndex(job, "Require every workflow job to pass")).toBeLessThan(
      stepIndex(job, "Declare the expected receipt job set")
    );
    // A failed `tools` job must be reported here by name, not turn into a
    // missing artifact that hides which job actually failed.
    expect(stepIndex(job, "Require every workflow job to pass")).toBeLessThan(
      stepIndex(job, "Use the prepared workspace tools")
    );
    expect(stepIndex(job, "Declare the expected receipt job set")).toBeLessThan(
      stepIndex(job, "Download every receipt")
    );
    expect(workflowStep(job, "Require every workflow job to pass").run).toContain(
      'required workflow jobs did not pass:'
    );
    expect(workflowStep(job, "Require every workflow job to pass").run).toContain(
      '.value.result != "success"'
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
    expect(source).toContain("required-job-results-input.json");
    expect(source).not.toContain("no receipt was collected at all");
    expect(workflowStep(job, "Upload the lane receipt").with?.overwrite).toBeTrue();
  });

  test("one producer builds both workspace tools and every consumer proves the digest", () => {
    const download = downloadArtifact;
    const artifact = "workspace-tools-${{ github.run_id }}";
    const producer = workflowJob(readWorkflow(".github/workflows/_tools.yml"), "tools");
    const build = workflowStep(producer, "Build both workspace tools once");
    const upload = workflowStep(producer, "Upload the workspace tools");
    const outputs = producer.outputs as Readonly<Record<string, string>> | undefined;

    expect(build.run).toContain("-p aex-release-tool -p aex-workspace-check");
    expect(upload.with?.name).toBe(artifact);
    // A partial re-run does not re-execute this producer, so an
    // attempt-scoped name would leave every consumer with nothing to download.
    expect(upload.with?.name).not.toContain("run_attempt");
    expect(upload.with?.overwrite).toBeTrue();
    expect(outputs?.release_tool_sha256).toBeDefined();
    expect(outputs?.workspace_check_sha256).toBeDefined();

    for (const [path, consumerId] of [
      [".github/workflows/_route.yml", "route"],
      [".github/workflows/_rust-lane.yml", "test"],
      [".github/workflows/_node-lane.yml", "unit"],
      [".github/workflows/_terraform-lane.yml", "terraform"],
      [".github/workflows/_receipts.yml", "aggregate"],
      [".github/workflows/_build-artifacts.yml", "build"],
      [".github/workflows/_compile-artifacts.yml", "compile"],
      [".github/workflows/main.yml", "gates"],
      [".github/workflows/pr.yml", "gates"]
    ] as const) {
      const consumer = workflowJob(readWorkflow(path), consumerId);
      const acquire = workflowStep(consumer, "Download the prepared workspace tools");
      const verify = workflowStep(consumer, "Use the prepared workspace tools");

      expect(acquire.uses, path).toBe(download);
      expect(acquire.with?.name, path).toBe(artifact);
      expect(verify.run, path).toContain("sha256sum --check --strict expected.sha256");
      expect(verify.run, path).toContain('[[ "$RELEASE_TOOL_SHA256" =~ ^[0-9a-f]{64}$ ]]');
      expect(verify.run, path).toContain('[[ "$WORKSPACE_CHECK_SHA256" =~ ^[0-9a-f]{64}$ ]]');
      expect(verify.run, path).toContain('echo "$RUNNER_TEMP/aex-tools" >> "$GITHUB_PATH"');
      expect(readRepoFile(path), path).not.toContain(
        "cargo run --locked --release -p aex-release-tool"
      );
      expect(readRepoFile(path), path).not.toContain("cargo run --locked -p aex-workspace-check");
    }
  });

  test("every local caller supplies every required input of the workflow it calls", () => {
    for (const path of workflowPaths) {
      for (const [jobId, job] of Object.entries(readWorkflow(path).jobs)) {
        const target = typeof job.uses === "string" ? job.uses : undefined;
        if (target === undefined || !target.startsWith("./")) continue;
        const called = workflowTriggers(readWorkflow(target.slice(2))).workflow_call as
          | { readonly inputs?: Readonly<Record<string, { readonly required?: boolean }>> }
          | undefined;
        const supplied = (job.with ?? {}) as Readonly<Record<string, unknown>>;
        const missing = Object.entries(called?.inputs ?? {})
          .filter(([name, spec]) => spec.required === true && supplied[name] === undefined)
          .map(([name]) => name)
          .sort();

        expect(missing, `${path} job ${jobId} calls ${target}`).toEqual([]);
      }
    }
  });

  test("every caller of a tool-consuming lane reads the digests from its own tools job", () => {
    for (const [path, jobIds] of [
      [".github/workflows/main.yml", ["route", "verify", "node", "terraform", "compile", "build", "receipts"]],
      [".github/workflows/pr.yml", ["route", "rust", "node", "terraform", "checks"]]
    ] as const) {
      const workflow = readWorkflow(path);
      expect(workflowJob(workflow, "tools").uses, path).toBe("./.github/workflows/_tools.yml");

      for (const jobId of jobIds) {
        const job = workflowJob(workflow, jobId);
        const inputs = (job.with ?? {}) as Readonly<Record<string, unknown>>;
        const where = `${path} job ${jobId}`;

        expect(jobNeeds(job), where).toContain("tools");
        expect(inputs.release_tool_sha256, where).toBe(
          "${{ needs.tools.outputs.release_tool_sha256 }}"
        );
        expect(inputs.workspace_check_sha256, where).toBe(
          "${{ needs.tools.outputs.workspace_check_sha256 }}"
        );
      }
    }
  });
});
