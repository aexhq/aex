import { describe, expect, it } from "vitest";
import {
  jobNeeds,
  readWorkflow,
  stepIndex,
  workflowJob,
  workflowStep,
  workflowTriggers
} from "./workflow-test-helpers.js";

const MANUAL_GATE_JOBS = ["lint", "unit-tests", "offline-user-tests", "docs-build", "pack-sdk"] as const;

describe("release pipeline gates", () => {
  it("runs public checks for pull requests and merge queues", () => {
    const workflow = readWorkflow(".github/workflows/ci.yml");
    const triggers = workflowTriggers(workflow);
    const parity = workflowJob(workflow, "contract-parity");
    const requireToken = workflowStep(parity, "Require platform token");
    const parityCheck = workflowStep(parity, "Contract parity");

    expect(triggers).toHaveProperty("pull_request");
    expect(triggers).toHaveProperty("merge_group");
    expect(parity.if).toContain("github.event_name != 'pull_request'");
    expect(parity.env).toHaveProperty("HAS_PLATFORM_TOKEN");
    expect(requireToken.run).toContain("contract parity gate cannot run");
    expect(parityCheck.run).toContain("bun run contracts:parity:check");
  });

  it("automatically publishes an immutable canary only after green main CI", () => {
    const workflow = readWorkflow(".github/workflows/release.yml");
    const workflowRun = workflowTriggers(workflow).workflow_run as Record<string, unknown>;
    const authorize = workflowJob(workflow, "authorize");
    const version = workflowJob(workflow, "version");
    const publish = workflowJob(workflow, "publish");
    const manifest = workflowJob(workflow, "public-release-manifest");
    const resolveVersion = workflowStep(version, "Resolve package version");
    const resolvePublication = workflowStep(version, "Resolve immutable publication state");
    const applyVersion = workflowStep(publish, "Apply immutable canary version");
    const bindSource = workflowStep(publish, "Bind package to release source");
    const registryEvidence = workflowStep(publish, "Resolve immutable registry evidence");
    const dispatch = workflowStep(manifest, "Dispatch exact candidate to platform");

    expect(workflowRun.workflows).toEqual(["CI"]);
    expect(workflowRun.types).toEqual(["completed"]);
    expect(workflowRun.branches).toEqual(["main"]);
    expect(workflow.concurrency).toMatchObject({
      group: expect.stringContaining("github.event.workflow_run.head_sha"),
      "cancel-in-progress": false
    });
    expect(authorize.if).toContain("github.event.workflow_run.conclusion == 'success'");
    expect(authorize.if).toContain("github.event.workflow_run.event == 'push'");
    expect(resolveVersion.run).toContain("canary-version.mjs resolve");
    expect(resolvePublication.run).toContain("already_published=true");
    expect(applyVersion.run).toContain("canary-version.mjs apply");
    expect(applyVersion.if).toContain("needs.version.outputs.already_published != 'true'");
    expect(bindSource.run).toContain('release-source.mjs apply --sha "${RELEASE_HEAD_SHA}"');
    expect(bindSource.if).toContain("needs.version.outputs.already_published != 'true'");
    expect(stepIndex(publish, bindSource.name!)).toBeLessThan(
      stepIndex(publish, workflowStep(publish, "Pack publish tarball").name!)
    );
    expect(registryEvidence.run).toContain("aexRelease.sourceSha");
    expect(registryEvidence.run).toContain('!= "${RELEASE_HEAD_SHA}"');
    expect(workflowStep(publish, "Publish to npm").if).toContain("needs.version.outputs.already_published != 'true'");
    expect(jobNeeds(publish)).not.toContain("live-user-tests-preflight");
    expect(jobNeeds(workflowJob(workflow, "live-user-tests"))).toContain("live-user-tests-preflight");
    expect(dispatch.run).toContain('event_type: "aex-canary-published"');
    expect(dispatch.run).toContain("jq -n");
    expect(dispatch.run).toContain('--arg sdk_version "${SDK_VERSION}"');
    expect(dispatch.run).toContain('--arg integrity "${SDK_INTEGRITY}"');
    expect(dispatch.run).toContain('--arg platform_sha "${PLATFORM_SHA}"');
    expect(dispatch.run).toContain('--arg release_key "${RELEASE_KEY}"');
  });

  it("reuses green main CI gates and reruns them only for manual releases", () => {
    const workflow = readWorkflow(".github/workflows/release.yml");
    for (const id of MANUAL_GATE_JOBS) {
      expect(workflowJob(workflow, id).if, id).toBe("${{ github.event_name == 'workflow_dispatch' }}");
    }

    const publish = workflowJob(workflow, "publish");
    expect(publish.if).toContain("github.event_name == 'workflow_run'");
    for (const id of MANUAL_GATE_JOBS) {
      expect(jobNeeds(publish), id).toContain(id);
      expect(publish.if, id).toContain(`needs.${id}.result == 'success'`);
    }
  });

  it("forbids latest as a release.yml dist-tag before publish", () => {
    const workflow = readWorkflow(".github/workflows/release.yml");
    const workflowDispatch = workflowTriggers(workflow).workflow_dispatch as {
      readonly inputs?: Readonly<Record<string, { readonly options?: readonly string[] }>>;
    };
    const version = workflowJob(workflow, "version");
    const publish = workflowJob(workflow, "publish");
    const guard = workflowStep(version, "Forbid direct publish to latest");

    expect(workflowDispatch.inputs?.npm_dist_tag?.options).toEqual(["canary", "next"]);
    expect(workflowDispatch.inputs?.release_key).toMatchObject({ required: false, type: "string" });
    expect(workflowDispatch.inputs?.platform_sha).toMatchObject({ required: false, type: "string" });
    expect(workflow["run-name"]).toContain("inputs.release_key");
    expect(guard.run).toContain('if [ "${NPM_DIST_TAG}" = "latest" ]');
    expect(jobNeeds(publish)).toContain("version");
    expect(workflowStep(publish, "Publish to npm")).toBeDefined();
  });

  it("fails the contract-parity job loudly when the platform gate is disarmed", () => {
    const workflow = readWorkflow(".github/workflows/ci.yml");
    const parity = workflowJob(workflow, "contract-parity");
    const requireToken = workflowStep(parity, "Require platform token");
    const checkout = workflowStep(parity, "Checkout platform (for contract parity)");
    const parityCheck = workflowStep(parity, "Contract parity");

    expect(parity.steps?.some((step) => step["continue-on-error"] === true)).toBe(false);
    expect(requireToken.if).toBe("${{ env.HAS_PLATFORM_TOKEN != 'true' }}");
    expect(checkout.if).toBeUndefined();
    expect(parityCheck.if).toBeUndefined();
    expect(parityCheck.run).toContain("bun run contracts:parity:check");
    expect(parityCheck.env?.PLATFORM_DIR).toBe("${{ github.workspace }}/_platform");
  });

  it("promotes only an exact version proven by successful public and platform runs", () => {
    const workflow = readWorkflow(".github/workflows/promote.yml");
    const triggers = workflowTriggers(workflow);
    const workflowDispatch = triggers.workflow_dispatch as {
      readonly inputs?: Readonly<Record<string, { readonly required?: boolean }>>;
    };
    const promote = workflowJob(workflow, "promote");
    const releaseRun = workflowStep(promote, "Verify green release workflow attempt published this version");
    const publicManifest = workflowStep(promote, "Verify public release manifest");
    const platformRun = workflowStep(promote, "Verify green dev validation and production promotion");
    const platformManifest = workflowStep(promote, "Verify platform validation manifest");
    const registry = workflowStep(promote, "Verify exact registry integrity");
    const monotonic = workflowStep(promote, "Require current monotonic release candidate");
    const addTag = workflowStep(promote, "Add npm dist-tag");

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
    expect(workflow["run-name"]).toContain("inputs.release_key");
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
    expect(releaseRun.run).toContain('.github/workflows/release.yml');
    expect(releaseRun.run).toContain('status}" != "completed');
    expect(publicManifest.run).toContain("release-manifest.mjs verify-public");
    expect(platformRun.env).toMatchObject({
      DEV_VALIDATION_RUN_ID: "${{ env.DEV_VALIDATION_RUN_ID }}",
      PLATFORM_PROMOTION_RUN_ID: "${{ env.PLATFORM_PROMOTION_RUN_ID }}"
    });
    expect(platformManifest.run).toContain("release-manifest.mjs verify-platform");
    expect(publicManifest.run).toContain('--head-sha "${RELEASE_SOURCE_SHA}"');
    expect(publicManifest.run).toContain('--integrity "${RELEASE_INTEGRITY}"');
    expect(registry.env).toMatchObject({ RELEASE_INTEGRITY: "${{ env.RELEASE_INTEGRITY }}" });
    expect(registry.run).toContain('registry_integrity');
    expect(registry.run).toContain('!= "${RELEASE_INTEGRITY}"');
    expect(monotonic.run).toContain("commits/main");
    expect(monotonic.run).toContain("dist-tags --json");
    expect(monotonic.run).toContain("aexRelease.sourceSha");
    expect(monotonic.run).toContain("--current-latest-version");
    expect(monotonic.run).toContain("--current-latest-sha");
    expect(monotonic.run).toContain("--current-canary-version");
    expect(monotonic.run).toContain("--current-canary-sha");
    expect(monotonic.run).toContain("assert-monotonic-promotion.mjs");
    expect(workflowStep(promote, "Checkout").with).toMatchObject({
      ref: "${{ env.RELEASE_SOURCE_SHA }}",
      "fetch-depth": 0
    });
    expect(stepIndex(promote, releaseRun.name!)).toBeLessThan(stepIndex(promote, addTag.name!));
    expect(stepIndex(promote, publicManifest.name!)).toBeLessThan(stepIndex(promote, addTag.name!));
    expect(stepIndex(promote, platformRun.name!)).toBeLessThan(stepIndex(promote, addTag.name!));
    expect(stepIndex(promote, platformManifest.name!)).toBeLessThan(stepIndex(promote, addTag.name!));
    expect(stepIndex(promote, registry.name!)).toBeLessThan(stepIndex(promote, addTag.name!));
    expect(stepIndex(promote, monotonic.name!)).toBeLessThan(stepIndex(promote, addTag.name!));
  });

  it("emits a public release manifest only after published-artifact smoke", () => {
    const workflow = readWorkflow(".github/workflows/release.yml");
    const smoke = workflowJob(workflow, "live-user-tests");
    const manifest = workflowJob(workflow, "public-release-manifest");
    const write = workflowStep(manifest, "Write public release manifest");
    const upload = workflowStep(manifest, "Upload public release manifest");

    expect(jobNeeds(manifest)).toContain("live-user-tests");
    expect(workflowStep(smoke, "Published-artifact smoke")).toBeDefined();
    expect(write.run).toContain("release-manifest.mjs write-public");
    expect(upload.with).toMatchObject({
      name: "public-release-manifest-${{ github.run_id }}",
      path: "public-release-manifest.json",
      overwrite: true
    });
  });

  it("cannot report success after publishing without smoke, manifest, and dispatch", () => {
    const workflow = readWorkflow(".github/workflows/release.yml");
    const smoke = workflowJob(workflow, "live-user-tests");
    const manifest = workflowJob(workflow, "public-release-manifest");
    const complete = workflowJob(workflow, "release-complete");
    const smokeRun = workflowStep(smoke, "Published-artifact smoke");
    const dispatch = workflowStep(manifest, "Dispatch exact candidate to platform");
    const verify = workflowStep(complete, "Require complete published candidate");

    expect(smoke.if).toContain("always()");
    expect(smoke.if).toContain("needs.publish.result == 'success'");
    expect(smoke.if).toContain("needs.live-user-tests-preflight.result == 'success'");

    expect(manifest.if).toContain("always()");
    expect(manifest.if).toContain("needs.publish.result == 'success'");
    expect(manifest.if).toContain("needs.live-user-tests.result == 'success'");
    expect(smoke["continue-on-error"]).not.toBe(true);
    expect(smokeRun.if).toBeUndefined();
    expect(smokeRun["continue-on-error"]).not.toBe(true);
    expect(dispatch.if).toBeUndefined();
    expect(dispatch["continue-on-error"]).not.toBe(true);

    expect(new Set(jobNeeds(complete))).toEqual(
      new Set(["publish", "live-user-tests", "public-release-manifest"])
    );
    expect(complete.if).toContain("always()");
    expect(complete.if).toContain("needs.publish.result == 'success'");
    expect(complete["continue-on-error"]).not.toBe(true);
    expect(verify.if).toBeUndefined();
    expect(verify["continue-on-error"]).not.toBe(true);
    expect(verify.env).toMatchObject({
      SMOKE_RESULT: "${{ needs.live-user-tests.result }}",
      MANIFEST_RESULT: "${{ needs.public-release-manifest.result }}"
    });
    expect(verify.run).toContain('"${SMOKE_RESULT}" != "success"');
    expect(verify.run).toContain('"${MANIFEST_RESULT}" != "success"');
    expect(verify.run).toContain("exit 1");
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
