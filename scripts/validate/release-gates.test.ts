import { describe, expect, it } from "vitest";
import {
  findWorkflowStep,
  jobNeeds,
  readRepoFile,
  readWorkflow,
  workflowJob,
  workflowStepBefore,
  workflowStepRunning,
  workflowStepUsing,
  workflowTriggers
} from "./workflow-test-helpers.js";

describe("release pipeline gates", () => {
  it("runs public checks for pull requests and merge queues", () => {
    const workflow = readWorkflow(".github/workflows/ci.yml");
    const triggers = workflowTriggers(workflow);
    const parity = workflowJob(workflow, "contract-parity");
    const requireToken = workflowStepRunning(parity, /contract parity gate cannot run/);
    const parityCheck = workflowStepRunning(parity, /contracts:parity:check/);

    expect(triggers).toHaveProperty("pull_request");
    expect(triggers).toHaveProperty("merge_group");
    expect(parity.if).toMatch(/github\.event_name\s*!=\s*'pull_request'/);
    expect(parity.env).toHaveProperty("HAS_PLATFORM_TOKEN");
    expect(requireToken.run).toMatch(/contract parity gate cannot run/);
    expect(parityCheck.run).toMatch(/\bbun\s+run\s+contracts:parity:check\b/);
  });

  it("publishes an immutable canary without granting the public workflow platform mutation authority", () => {
    const source = readRepoFile(".github/workflows/release.yml");
    const workflow = readWorkflow(".github/workflows/release.yml");
    const triggers = workflowTriggers(workflow);
    const workflowDispatch = triggers.workflow_dispatch as {
      readonly inputs?: Readonly<Record<string, { readonly required?: boolean }>>;
    };
    const authorize = workflowJob(workflow, "authorize");
    const version = workflowJob(workflow, "version");
    const publish = workflowJob(workflow, "publish");
    const manifest = workflowJob(workflow, "public-release-manifest");
    const resolveVersion = workflowStepRunning(version, /canary-version\.mjs\s+resolve/);
    const resolvePublication = workflowStepRunning(version, /already_published=/);
    const applyVersion = workflowStepRunning(publish, /canary-version\.mjs\s+apply/);
    const bindSource = workflowStepRunning(publish, /release-source\.mjs\s+apply/);
    const registryEvidence = workflowStepRunning(publish, /wait-for-npm\.mjs/);
    const uploadManifest = findWorkflowStep(
      manifest,
      (step) => step.uses?.startsWith("actions/upload-artifact@") === true && step.with?.path === "public-release-manifest.json",
      "public manifest upload"
    );

    expect(Object.keys(triggers)).toEqual(["workflow_dispatch"]);
    expect(workflowDispatch.inputs?.platform_sha?.required).toBe(true);
    expect(workflowDispatch.inputs?.release_key?.required).toBe(true);
    expect(workflow.concurrency).toMatchObject({
      group: expect.stringContaining("github.sha"),
      "cancel-in-progress": false
    });
    const authorizeSource = workflowStepRunning(authorize, /expected_ref=/);
    expect(authorize.if).toBeUndefined();
    expect(authorizeSource.run).toMatch(/refs\/tags\/release\/sha-\$\{RELEASE_HEAD_SHA\}/);
    expect(authorizeSource.run).toMatch(/PLATFORM_SHA.*\[0-9a-f\].*40/);
    expect(authorizeSource.run).toMatch(/-z.*RELEASE_KEY/);
    expect(resolveVersion.run).toMatch(/canary-version\.mjs\s+resolve/);
    expect(resolvePublication.run).toMatch(/already_published=true/);
    expect(applyVersion.run).toMatch(/canary-version\.mjs\s+apply/);
    expect(applyVersion.if).toMatch(/needs\.version\.outputs\.already_published\s*!=\s*'true'/);
    expect(bindSource.run).toMatch(/release-source\.mjs\s+apply[\s\S]*RELEASE_HEAD_SHA/);
    expect(bindSource.if).toMatch(/needs\.version\.outputs\.already_published\s*!=\s*'true'/);
    workflowStepBefore(publish, bindSource, workflowStepRunning(publish, /bun\s+pm\s+pack/));
    expect(registryEvidence.id).toBe("registry-evidence");
    expect(registryEvidence.run).toMatch(/wait-for-npm\.mjs\s+@aexhq\/sdk\s+.*SDK_VERSION/);
    expect(registryEvidence.run).toMatch(/--source-sha\s+.*RELEASE_HEAD_SHA/);
    expect(registryEvidence.run).toMatch(/--github-output\s+.*GITHUB_OUTPUT/);
    expect(
      publish.steps?.filter((step) => typeof step.run === "string" && step.run.includes("npm view"))
    ).toHaveLength(0);
    expect(workflowStepRunning(publish, /\bnpm\s+publish\b/).if).toMatch(/needs\.version\.outputs\.already_published\s*!=\s*'true'/);
    expect(jobNeeds(publish)).not.toContain("live-user-tests-preflight");
    expect(jobNeeds(publish)).not.toContain("platform-dispatch-preflight");
    expect(jobNeeds(workflowJob(workflow, "live-user-tests"))).toContain("live-user-tests-preflight");
    expect(uploadManifest.id).toBeUndefined();
    expect(workflow.jobs).not.toHaveProperty("platform-dispatch-preflight");
    expect(source).not.toContain("PLATFORM_REPO_TOKEN");
    expect(source).not.toContain("repos/aexhq/platform/actions/workflows");
  });

  it("runs every public gate for the controller-owned release", () => {
    const workflow = readWorkflow(".github/workflows/release.yml");
    const ci = readWorkflow(".github/workflows/ci.yml");
    const ciPublicNeeds = jobNeeds(workflowJob(ci, "public"));
    // Contract parity is controller/platform-owned in release.yml; all other
    // public CI gates must remain direct publish prerequisites.
    const releaseGateIds = ciPublicNeeds.filter((id) => id !== "contract-parity");
    expect(releaseGateIds.length).toBeGreaterThan(0);
    for (const id of releaseGateIds) {
      expect(workflowJob(workflow, id).if, id).toBeUndefined();
    }

    const publish = workflowJob(workflow, "publish");
    for (const id of releaseGateIds) {
      expect(jobNeeds(publish), id).toContain(id);
    }
    expect(workflowTriggers(workflow)).not.toHaveProperty("workflow_run");
  });

  it("publishes a source-bound immutable canary for controller workflow dispatch", () => {
    const workflow = readWorkflow(".github/workflows/release.yml");
    const version = workflowJob(workflow, "version");
    const publish = workflowJob(workflow, "publish");
    const resolveVersion = workflowStepRunning(version, /canary-version\.mjs\s+resolve/);
    const resolvePublication = workflowStepRunning(version, /already_published=/);
    const applyVersion = workflowStepRunning(publish, /canary-version\.mjs\s+apply/);

    expect(workflow.env?.RELEASE_MODE).toBe("immutable-canary");
    expect(resolveVersion.run).toMatch(/canary-version\.mjs\s+resolve/);
    expect(applyVersion.if).toBe("${{ needs.version.outputs.already_published != 'true' }}");
  });

  it("requires the canary dist-tag before publish", () => {
    const workflow = readWorkflow(".github/workflows/release.yml");
    const workflowDispatch = workflowTriggers(workflow).workflow_dispatch as {
      readonly inputs?: Readonly<Record<string, { readonly options?: readonly string[] }>>;
    };
    const version = workflowJob(workflow, "version");
    const publish = workflowJob(workflow, "publish");
    const guard = workflowStepRunning(version, /NPM_DIST_TAG/);

    expect(workflowDispatch.inputs?.npm_dist_tag?.options).toEqual(["canary"]);
    expect(workflowDispatch.inputs?.release_key).toMatchObject({ required: true, type: "string" });
    expect(workflowDispatch.inputs?.platform_sha).toMatchObject({ required: true, type: "string" });
    expect(workflow["run-name"]).toMatch(/inputs\.release_key/);
    expect(guard.run).toMatch(/NPM_DIST_TAG.*canary/);
    expect(jobNeeds(publish)).toContain("version");
    expect(workflowStepRunning(publish, /\bnpm\s+publish\b/)).toBeDefined();
  });

  it("fails the contract-parity job loudly when the platform gate is disarmed", () => {
    const workflow = readWorkflow(".github/workflows/ci.yml");
    const parity = workflowJob(workflow, "contract-parity");
    const requireToken = workflowStepRunning(parity, /HAS_PLATFORM_TOKEN/);
    const checkout = findWorkflowStep(
      parity,
      (step) => step.uses?.startsWith("actions/checkout@") === true && step.with?.repository === "aexhq/platform",
      "platform checkout"
    );
    const parityCheck = workflowStepRunning(parity, /contracts:parity:check/);

    expect(parity.steps?.some((step) => step["continue-on-error"] === true)).toBe(false);
    expect(requireToken.if).toBe("${{ env.HAS_PLATFORM_TOKEN != 'true' }}");
    expect(checkout.if).toBeUndefined();
    expect(parityCheck.if).toBeUndefined();
    expect(parityCheck.run).toMatch(/\bbun\s+run\s+contracts:parity:check\b/);
    expect(parityCheck.env?.PLATFORM_DIR).toBe("${{ github.workspace }}/_platform");
  });

  it("runs the private semantic mirror inventory with redacted explicit roots", () => {
    const workflow = readWorkflow(".github/workflows/ci.yml");
    const parity = workflowJob(workflow, "contract-parity");
    const checkout = findWorkflowStep(
      parity,
      (step) => step.uses?.startsWith("actions/checkout@") === true && step.with?.repository === "aexhq/platform",
      "platform checkout"
    );
    const install = workflowStepRunning(parity, /\bbun\s+ci\b/);
    const parityCheck = workflowStepRunning(parity, /contracts:parity:check/);
    const inventory = workflowStepRunning(parity, /semantic-mirror-inventory\.mjs/);
    const aggregate = workflowJob(workflow, "public");

    workflowStepBefore(parity, checkout, install);
    workflowStepBefore(parity, install, inventory);
    workflowStepBefore(parity, parityCheck, inventory);
    expect(inventory.run).toMatch(/_platform\/scripts\/cicd\/semantic-mirror-inventory\.mjs/);
    expect(inventory.run).toMatch(/--check/);
    expect(inventory.run).toMatch(/--report\s+redacted/);
    expect(inventory.run).toMatch(/--platform-root[\s\S]*_platform/);
    expect(inventory.run).toMatch(/--public-root[\s\S]*github\.workspace/);
    expect(inventory.run).not.toMatch(/--write/);
    expect(inventory.if).toBeUndefined();
    expect(jobNeeds(aggregate)).toContain("contract-parity");
  });

  it("promotes only an exact version proven by successful public and platform runs", () => {
    const workflow = readWorkflow(".github/workflows/promote.yml");
    const triggers = workflowTriggers(workflow);
    const workflowDispatch = triggers.workflow_dispatch as {
      readonly inputs?: Readonly<Record<string, { readonly required?: boolean }>>;
    };
    const promote = workflowJob(workflow, "promote");
    const releaseRun = workflowStepRunning(promote, /workflow_path=/);
    const publicManifest = workflowStepRunning(promote, /release-manifest\.mjs\s+verify-public/);
    const platformRun = workflowStepRunning(promote, /repos\/aexhq\/platform\/actions\/runs/);
    const platformManifest = workflowStepRunning(promote, /release-manifest\.mjs\s+verify-platform/);
    const registry = workflowStepRunning(promote, /registry_integrity/);
    const monotonic = workflowStepRunning(promote, /assert-monotonic-promotion\.mjs/);
    const addTag = workflowStepRunning(promote, /npm\s+dist-tag\s+add/);

    expect(new Set(Object.keys(triggers))).toEqual(new Set(["workflow_dispatch"]));
    for (const input of [
      "version",
      "release_run_id",
      "dev_validation_run_id",
      "platform_promotion_run_id",
      "public_sha",
      "sdk_integrity",
      "npm_dist_tag"
    ]) {
      expect(workflowDispatch.inputs?.[input]?.required, input).toBe(true);
    }
    expect(workflowDispatch.inputs?.release_key).toMatchObject({ required: false, type: "string" });
    expect(workflow["run-name"]).toMatch(/inputs\.release_key/);
    expect(workflow.env).toMatchObject({
      PACKAGE_VERSION: "${{ inputs.version }}",
      RELEASE_RUN_ID: "${{ inputs.release_run_id }}",
      DEV_VALIDATION_RUN_ID: "${{ inputs.dev_validation_run_id }}",
      PLATFORM_PROMOTION_RUN_ID: "${{ inputs.platform_promotion_run_id }}",
      RELEASE_SOURCE_SHA: "${{ inputs.public_sha }}",
      RELEASE_INTEGRITY: "${{ inputs.sdk_integrity }}",
      NPM_DIST_TAG: "${{ inputs.npm_dist_tag }}"
    });
    expect(JSON.stringify(workflow.env)).not.toContain("client_payload");
    expect(JSON.stringify(workflow.env)).not.toContain("github.event");
    expect(workflow.permissions).toMatchObject({ actions: "read" });
    expect(workflow.concurrency).toMatchObject({
      group: "promote-aex",
      "cancel-in-progress": false
    });
    expect(releaseRun.run).toMatch(/workflows\/release\.yml/);
    expect(releaseRun.run).toMatch(/status.*completed/);
    expect(publicManifest.run).toMatch(/release-manifest\.mjs\s+verify-public/);
    expect(platformRun.env).toMatchObject({
      DEV_VALIDATION_RUN_ID: "${{ env.DEV_VALIDATION_RUN_ID }}",
      PLATFORM_PROMOTION_RUN_ID: "${{ env.PLATFORM_PROMOTION_RUN_ID }}"
    });
    expect(platformManifest.run).toMatch(/release-manifest\.mjs\s+verify-platform/);
    expect(publicManifest.run).toMatch(/--head-sha\s+"\$\{RELEASE_SOURCE_SHA\}"/);
    expect(publicManifest.run).toMatch(/--integrity\s+"\$\{RELEASE_INTEGRITY\}"/);
    expect(registry.env).toMatchObject({ RELEASE_INTEGRITY: "${{ env.RELEASE_INTEGRITY }}" });
    expect(registry.run).toMatch(/registry_integrity/);
    expect(registry.run).toMatch(/registry_integrity.*RELEASE_INTEGRITY/);
    expect(monotonic.run).toMatch(/commits\/main/);
    expect(monotonic.run).toMatch(/dist-tags\s+--json/);
    expect(monotonic.run).toMatch(/aexRelease\.sourceSha/);
    expect(monotonic.run).toMatch(/--current-latest-version/);
    expect(monotonic.run).toMatch(/--current-latest-sha/);
    expect(monotonic.run).toMatch(/--current-canary-version/);
    expect(monotonic.run).toMatch(/--current-canary-sha/);
    expect(monotonic.run).toMatch(/assert-monotonic-promotion\.mjs/);
    expect(findWorkflowStep(promote, (step) => step.uses?.startsWith("actions/checkout@") === true, "source checkout").with).toMatchObject({
      ref: "${{ env.RELEASE_SOURCE_SHA }}",
      "fetch-depth": 0
    });
    for (const gate of [releaseRun, publicManifest, platformRun, platformManifest, registry, monotonic]) {
      workflowStepBefore(promote, gate, addTag);
    }
  });

  it("emits a public release manifest only after published-artifact smoke", () => {
    const workflow = readWorkflow(".github/workflows/release.yml");
    const smoke = workflowJob(workflow, "live-user-tests");
    const manifest = workflowJob(workflow, "public-release-manifest");
    const write = workflowStepRunning(manifest, /release-manifest\.mjs\s+write-public/);
    const upload = findWorkflowStep(
      manifest,
      (step) => step.uses?.startsWith("actions/upload-artifact@") === true && step.with?.path === "public-release-manifest.json",
      "public manifest upload"
    );

    expect(jobNeeds(manifest)).toContain("live-user-tests");
    expect(workflowStepRunning(smoke, /\btest:user:smoke\b/)).toBeDefined();
    expect(write.run).toMatch(/release-manifest\.mjs\s+write-public/);
    expect(upload.with).toMatchObject({
      name: "public-release-manifest-${{ github.run_id }}",
      path: "public-release-manifest.json",
      overwrite: true
    });
    expect(upload.id).toBeUndefined();
  });

  it("cannot report success after publishing without smoke and manifest", () => {
    const workflow = readWorkflow(".github/workflows/release.yml");
    const smoke = workflowJob(workflow, "live-user-tests");
    const manifest = workflowJob(workflow, "public-release-manifest");
    const complete = workflowJob(workflow, "release-complete");
    const smokeRun = workflowStepRunning(smoke, /\btest:user:smoke\b/);
    const verify = workflowStepRunning(complete, /SMOKE_RESULT/);

    expect(smoke.if).toMatch(/always\(\)/);
    expect(smoke.if).toMatch(/needs\.publish\.result\s*==\s*'success'/);
    expect(smoke.if).toMatch(/needs\.live-user-tests-preflight\.result\s*==\s*'success'/);

    expect(manifest.if).toMatch(/always\(\)/);
    expect(manifest.if).toMatch(/needs\.publish\.result\s*==\s*'success'/);
    expect(manifest.if).toMatch(/needs\.live-user-tests\.result\s*==\s*'success'/);
    expect(smoke["continue-on-error"]).not.toBe(true);
    expect(smokeRun.if).toBeUndefined();
    expect(smokeRun["continue-on-error"]).not.toBe(true);
    expect(new Set(jobNeeds(complete))).toEqual(
      new Set(["publish", "live-user-tests", "public-release-manifest"])
    );
    expect(complete.if).toMatch(/always\(\)/);
    expect(complete.if).toMatch(/needs\.publish\.result\s*==\s*'success'/);
    expect(complete["continue-on-error"]).not.toBe(true);
    expect(verify.if).toBeUndefined();
    expect(verify["continue-on-error"]).not.toBe(true);
    expect(verify.env).toMatchObject({
      SMOKE_RESULT: "${{ needs.live-user-tests.result }}",
      MANIFEST_RESULT: "${{ needs.public-release-manifest.result }}"
    });
    expect(verify.run).toMatch(/SMOKE_RESULT.*success/);
    expect(verify.run).toMatch(/MANIFEST_RESULT.*success/);
    expect(verify.run).toMatch(/exit\s+1/);
  });

  it("overwrites reusable release artifacts when a workflow run is rerun", () => {
    const workflow = readWorkflow(".github/workflows/release.yml");
    const reusableUploads = Object.values(workflow.jobs)
      .flatMap((job) => job.steps ?? [])
      .filter((step) => step.uses?.startsWith("actions/upload-artifact@"))
      .filter((step) => !String(step.with?.name ?? "").includes("github.run_attempt"));

    expect(reusableUploads.length).toBeGreaterThan(0);
    for (const upload of reusableUploads) {
      expect(upload.with?.overwrite, upload.name).toBe(true);
    }
  });

});
