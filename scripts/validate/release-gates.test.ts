import { describe, expect, it } from "vitest";
import {
  jobNeeds,
  readRepoFile,
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
    expect(dispatch.run).toContain('"event_type": "aex-canary-published"');
    expect(dispatch.run).toContain('"sdk_version": "${SDK_VERSION}"');
    expect(dispatch.run).toContain('"integrity": "${SDK_INTEGRITY}"');
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
    expect(parityCheck.env?.AEX_PLATFORM_DIR).toBe("${{ github.workspace }}/_platform");
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
    const platformRun = workflowStep(promote, "Verify green platform deploy tested this version");
    const platformManifest = workflowStep(promote, "Verify platform validation manifest");
    const registry = workflowStep(promote, "Verify exact registry integrity");
    const monotonic = workflowStep(promote, "Require current monotonic release candidate");
    const addTag = workflowStep(promote, "Add npm dist-tag");

    expect(new Set(Object.keys(triggers))).toEqual(new Set(["workflow_dispatch"]));
    for (const input of [
      "version",
      "release_run_id",
      "platform_deploy_run_id",
      "public_sha",
      "sdk_integrity",
      "npm_dist_tag"
    ]) {
      expect(workflowDispatch.inputs?.[input]?.required, input).toBe(true);
    }
    expect(workflow.env).toMatchObject({
      PACKAGE_VERSION: "${{ inputs.version }}",
      RELEASE_RUN_ID: "${{ inputs.release_run_id }}",
      PLATFORM_DEPLOY_RUN_ID: "${{ inputs.platform_deploy_run_id }}",
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
    expect(platformRun.run).toContain('.github/workflows/deploy.yml');
    expect(platformRun.run).toContain('proof_schema}" != "2"');
    expect(platformRun.run).toContain("aex-platform-promotion-proof");
    expect(platformManifest.run).toContain("release-manifest.mjs verify-platform");
    const proofGates = /for gate in ([^;]+); do/.exec(platformRun.run ?? "")?.[1]?.trim().split(/\s+/) ?? [];
    expect(new Set(proofGates)).toEqual(new Set(["suite_dev", "spot_canary_dev", "suite_prod", "smoke_prod"]));
    expect(platformRun.run).toContain("for image in brain egress byok; do");
    expect(platformRun.run).toContain("$e.prd.digest == $e.dev.digest");
    expect(platformRun.run).toContain('test("^sha256:[0-9a-f]{64}$")');
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

  it("keeps feature-gate.yml manual, local, and non-publishing", () => {
    const workflow = readWorkflow(".github/workflows/feature-gate.yml");
    const triggers = workflowTriggers(workflow);
    const rootPackageJson = JSON.parse(readRepoFile("package.json")) as { scripts?: Record<string, string> };
    const commands = Object.values(workflow.jobs)
      .flatMap((job) => job.steps ?? [])
      .map((step) => step.run ?? "")
      .join("\n");
    const serialized = JSON.stringify(workflow);

    expect(rootPackageJson.scripts?.["gate:feature"]).toBe("bun scripts/cicd/feature-gate.mjs");
    expect(Object.keys(triggers)).toEqual(["workflow_dispatch"]);
    expect(commands).toContain("bun scripts/cicd/feature-gate.mjs");
    expect(commands).toContain("bun run test:user:offline");
    expect(commands).toContain("bun run pack:sdk");
    expect(commands).not.toContain("npm publish");
    expect(commands).not.toContain("npm dist-tag");
    expect(serialized).not.toContain("AEX_API_KEY");
  });
});
