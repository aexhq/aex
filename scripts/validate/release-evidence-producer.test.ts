import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

// @ts-expect-error JavaScript CI policy helper is validated directly.
import * as releaseEvidence from "../cicd/release-evidence.mjs";
import {
  readRepoFile,
  readWorkflow,
  stepIndex,
  workflowJob,
  workflowStep
} from "./workflow-test-helpers.js";

const sha = "1".repeat(40);
const digest = `sha256:${"2".repeat(64)}`;
const releaseId = `sha256:${"3".repeat(64)}`;
const deploymentContextDigest = `sha256:${"4".repeat(64)}`;
const releaseTag = `main-${sha}-run-123-attempt-1`;
const base = `https://github.com/aexhq/aex/releases/download/${releaseTag}`;
const {
  assertExactCoordinates,
  assertExactDeploymentHealth,
  checkExactDeploymentHealth,
  assertRunnableScenarioMatrix,
  assertUserJourneyInventory,
  validateHygieneReport
} = releaseEvidence;

describe("release-bound public evidence producer", () => {
  it("accepts only one exact main release coordinate closure", () => {
    expect(() => assertExactCoordinates({
      repository: "aexhq/aex",
      sourceSha: sha,
      releaseId,
      deploymentContextDigest,
      releaseTag,
      manifestUri: `${base}/composition-manifest.json`,
      manifestDigest: digest,
      manifestSizeBytes: "123",
      releaseToolUri: `${base}/aex-release-tool`,
      releaseToolDigest: digest,
      releaseToolSizeBytes: "456"
    })).not.toThrow();

    for (const invalid of [
      { sourceSha: "main" },
      { releaseId: "release-latest" },
      { deploymentContextDigest: "continuation-latest" },
      { releaseTag: `main-${sha}-run-123-attempt-2` },
      { manifestUri: "https://example.invalid/composition-manifest.json" },
      { releaseToolUri: `${base}/composition-manifest.json` },
      { manifestSizeBytes: "0" }
    ]) {
      expect(() => assertExactCoordinates({
        repository: "aexhq/aex",
        sourceSha: sha,
        releaseId,
        deploymentContextDigest,
        releaseTag,
        manifestUri: `${base}/composition-manifest.json`,
        manifestDigest: digest,
        manifestSizeBytes: "123",
        releaseToolUri: `${base}/aex-release-tool`,
        releaseToolDigest: digest,
        releaseToolSizeBytes: "456",
        ...invalid
      })).toThrow();
    }
  });

  it("accepts only the canonical ready response for the exact deployed release", () => {
    expect(assertExactDeploymentHealth({
      schema: "aex.release-health.v1",
      releaseId,
      status: "ready"
    }, { releaseId })).toEqual({
      schema: "aex.release-health.v1",
      releaseId,
      status: "ready"
    });

    for (const invalid of [
      { schema: "aex.release-health.v0", releaseId, status: "ready" },
      { schema: "aex.release-health.v1", releaseId: digest, status: "ready" },
      { schema: "aex.release-health.v1", releaseId, status: "healthy" },
      { schema: "aex.release-health.v1", releaseId, status: "ready", deploymentContextDigest }
    ]) {
      expect(() => assertExactDeploymentHealth(invalid, { releaseId })).toThrow();
    }
  });

  it("checks an exact protected health URL without redirecting or accepting cacheable bytes", async () => {
    const calls: Array<{ url: string; init: RequestInit }> = [];
    const observation = await checkExactDeploymentHealth({
      healthUrl: "https://eu-west-1.dev-api.aex.dev/internal/release-health",
      expectedHost: "eu-west-1.dev-api.aex.dev",
      releaseId,
      deploymentContextDigest,
      phase: "before",
      fetchImpl: async (url: URL, init: RequestInit) => {
        calls.push({ url: String(url), init });
        return new Response(JSON.stringify({
          schema: "aex.release-health.v1",
          releaseId,
          status: "ready"
        }), {
          status: 200,
          headers: { "content-type": "application/json", "cache-control": "no-store" }
        });
      }
    });
    expect(calls).toHaveLength(1);
    expect(calls[0]?.init.redirect).toBe("error");
    expect(observation).toMatchObject({
      schema: "aex.release-health-observation.v1",
      releaseId,
      deploymentContextDigest,
      phase: "before"
    });

    await expect(checkExactDeploymentHealth({
      healthUrl: "https://other.example/internal/release-health",
      expectedHost: "eu-west-1.dev-api.aex.dev",
      releaseId,
      deploymentContextDigest,
      phase: "before",
      fetchImpl: async () => new Response()
    })).rejects.toThrow("host differs");
  });

  it("fails closed on an empty scenario route or a registry-only user suite", () => {
    expect(() => assertRunnableScenarioMatrix({ include: [] })).toThrow("non-empty");
    expect(() => assertRunnableScenarioMatrix({
      include: [{
        id: "scenario:SC-SESSION-ADMIT",
        name: "SC-SESSION-ADMIT",
        package: "cargo:aex-live-session-api",
        target: "live",
        partition: 0,
        partitions: 1,
        units: []
      }]
    })).not.toThrow();

    const liveIds = ["live.session-round-trip", "live.provider-model-pair"];
    expect(() => assertUserJourneyInventory(
      "(pass) live scenarios fail closed onto dev descriptors",
      liveIds
    )).toThrow("non-empty runnable user journey");
    expect(assertUserJourneyInventory(
      "(pass) live.session-round-trip\n(pass) live.provider-model-pair",
      liveIds
    )).toEqual(["live.provider-model-pair", "live.session-round-trip"]);
  });

  it("derives receipt hygiene only from a bounded cleanup, spend and canary report", () => {
    const valid = {
      schema: "aex.release-evidence-hygiene.v1",
      suite: "e2e",
      releaseId,
      workflowRunId: "999",
      budgetMicroUsd: 1000,
      spentMicroUsd: 500,
      cleanupLedgerDigest: digest,
      provisioned: [{ kind: "session", id: "ses_test", reclaimed: true }],
      residue: [],
      secretCanaryDigest: digest,
      secretCanaryObserved: false
    } as const;

    expect(validateHygieneReport(valid, {
      suite: "e2e",
      releaseId,
      workflowRunId: "999",
      secretCanaryDigest: digest,
      maximumBudgetMicroUsd: 1000
    })).toEqual({
      budgetMicroUsd: 1000,
      spentMicroUsd: 500,
      cleanupLedgerDigest: digest,
      residue: "none",
      secretCanaryObserved: false
    });

    for (const invalid of [
      { spentMicroUsd: 1001 },
      { provisioned: [{ kind: "session", id: "ses_test", reclaimed: false }] },
      { residue: ["session:ses_test"] },
      { secretCanaryObserved: true },
      { secretCanaryDigest: `sha256:${"4".repeat(64)}` }
    ]) {
      expect(() => validateHygieneReport({ ...valid, ...invalid }, {
        suite: "e2e",
        releaseId,
        workflowRunId: "999",
        secretCanaryDigest: digest,
        maximumBudgetMicroUsd: 1000
      })).toThrow();
    }
  });

  it("uses protected dev inputs, exact bytes, real suites, and attested immutable receipts", () => {
    const path = ".github/workflows/release-evidence.yml";
    const source = readRepoFile(path);
    const producerSource = readRepoFile("scripts/cicd/release-evidence.mjs");
    const workflow = readWorkflow(path);
    const dispatch = (workflow.on as Record<string, Record<string, unknown>>).workflow_dispatch;
    if (!dispatch) throw new Error("release-evidence workflow_dispatch is missing");
    const inputs = dispatch.inputs as Record<string, unknown>;
    const job = workflowJob(workflow, "evidence");
    const publish = workflowJob(workflow, "publish");

    expect(Object.keys(inputs).sort()).toEqual([
      "deployment_context_digest",
      "manifest_digest",
      "manifest_size_bytes",
      "manifest_uri",
      "public_source_sha",
      "release_id",
      "release_tag",
      "release_tool_digest",
      "release_tool_size_bytes",
      "release_tool_uri"
    ]);
    expect(source).not.toMatch(/(?:e2e|user)_receipt_(?:uri|digest|size)/);
    expect(job["runs-on"]).toBe("ubuntu-latest");
    expect(job.environment).toBe("aex-release-evidence-dev");
    expect(job.permissions).toEqual({ contents: "read" });
    expect(publish["runs-on"]).toBe("ubuntu-latest");
    expect(publish.environment).toBeUndefined();
    expect(publish.permissions).toEqual({
      contents: "write",
      "id-token": "write",
      attestations: "write",
      "artifact-metadata": "write"
    });

    expect(workflowStep(job, "Acquire and verify the exact public bytes").run).toContain("gh attestation verify");
    expect(workflowStep(job, "Acquire and verify the exact public bytes").run).toContain("--source-digest \"$PUBLIC_SOURCE_SHA\"");
    expect(workflowStep(job, "Acquire and verify the exact public bytes").run).toContain(".github/workflows/main.yml");
    expect(workflowStep(job, "Acquire and verify the exact public bytes").run).toContain(".github/workflows/_build-artifacts.yml");
    expect(workflowStep(job, "Bind the protected run and create its source archive").run).toContain(
      '[[ "$DISPATCH_REF" == "refs/tags/$RELEASE_TAG" ]]'
    );
    expect(workflowStep(job, "Require non-empty release scenario and user-journey inventories").run).toContain("graph verify --release");
    expect(source).toContain("AEX_RELEASE_EVIDENCE_HEALTH_URL");
    expect(source).toContain("https://{0}/api/release/health");
    expect(source).not.toContain("secrets.AEX_RELEASE_EVIDENCE_HEALTH_URL");
    expect(source).not.toContain("AEX_RELEASE_EVIDENCE_SYNTHETIC_IDENTITY");
    expect(source).not.toContain("AEX_RELEASE_EVIDENCE_PROVIDER_CREDENTIALS_JSON");
    expect(workflowStep(job, "Bind the protected run and create its source archive").run).toContain(
      '[[ "$AEX_RELEASE_EVIDENCE_MAXIMUM_BUDGET_MICRO_USD" == "0" ]]'
    );
    expect(workflowStep(job, "Verify the exact deployed release before evidence").run).toContain("deployment-health");
    expect(workflowStep(job, "Verify the exact deployed release before evidence").run).toContain("--phase before");
    expect(workflowStep(job, "Verify the exact deployed release after evidence").run).toContain("deployment-health");
    expect(workflowStep(job, "Verify the exact deployed release after evidence").run).toContain("--phase after");
    expect(workflowStep(job, "Verify the exact deployed release after evidence").run).toContain("deployment-health.json");
    expect(source).not.toContain("preflight-live-user-tests.mjs");
    expect(workflowStep(job, "Run release-bound E2E and user suites").run).toContain("run-e2e");
    expect(workflowStep(job, "Run release-bound E2E and user suites").run).toContain("run-user");
    expect(workflowStep(job, "Verify inventory, cleanup, spend and secret-canary evidence").run).toContain("assert-no-skips.mjs");
    expect(workflowStep(job, "Verify inventory, cleanup, spend and secret-canary evidence").run).toContain("grep -rFq");
    expect(workflowStep(job, "Build the release-bound receipts").run).toContain("evidence new");
    expect(workflowStep(job, "Build the release-bound receipts").run).toContain("evidence verify");
    expect(workflowStep(job, "Build the release-bound receipts").run).toContain("deploymentContextDigest");
    expect(workflowStep(job, "Build the release-bound receipts").run).toContain('selection:{mode:"full"');
    expect(workflowStep(job, "Build the release-bound receipts").run).not.toContain('selection:{mode:"release"');
    expect(workflowStep(job, "Build the release-bound receipts").run).toContain('concerns:["contract"]');
    expect(workflowStep(job, "Build the release-bound receipts").run).not.toContain("concerns:[$class]");
    expect(workflowStep(job, "Build the release-bound receipts").run).toContain("--kind deployment-health");
    expect(producerSource.match(/"--features", "live"/g)).toHaveLength(2);
    expect(producerSource.match(/"--ignore-default-filter"/g)).toHaveLength(2);
    expect(workflowStep(publish, "Validate the closed handoff shape").run).toContain("deploymentContextDigest");
    expect(source.match(/actions\/attest@59d89421af93a897026c735860bf21b6eb4f7b26/g)).toHaveLength(3);
    expect(workflowStep(publish, "Publish the immutable evidence handoff").run).toContain("gh release create");
    expect(workflowStep(publish, "Publish the immutable evidence handoff").run).toContain(
      '--source-ref "refs/tags/$RELEASE_TAG"'
    );
    expect(workflowStep(publish, "Publish the immutable evidence handoff").run).not.toContain("--clobber");
    expect(source).not.toMatch(/(?:^|\s)--retry(?:[=\s]|$)/m);

    expect(stepIndex(job, "Acquire and verify the exact public bytes")).toBeLessThan(
      stepIndex(job, "Require non-empty release scenario and user-journey inventories")
    );
    expect(stepIndex(job, "Require non-empty release scenario and user-journey inventories")).toBeLessThan(
      stepIndex(job, "Verify the exact deployed release before evidence")
    );
    expect(stepIndex(job, "Verify the exact deployed release before evidence")).toBeLessThan(
      stepIndex(job, "Run release-bound E2E and user suites")
    );
    expect(stepIndex(job, "Run release-bound E2E and user suites")).toBeLessThan(
      stepIndex(job, "Verify the exact deployed release after evidence")
    );
    expect(stepIndex(job, "Verify the exact deployed release after evidence")).toBeLessThan(
      stepIndex(job, "Verify inventory, cleanup, spend and secret-canary evidence")
    );
    expect(stepIndex(job, "Verify inventory, cleanup, spend and secret-canary evidence")).toBeLessThan(
      stepIndex(job, "Build the release-bound receipts")
    );
  });

  it("mounts the public release-health router only in regional-session-api", () => {
    const root = resolve(import.meta.dir, "../..");
    const owners = [...new Bun.Glob("services/**/src/**/*.rs").scanSync({ cwd: root })]
      .filter((path) => readFileSync(resolve(root, path), "utf8").includes("release_health::router"))
      .map((path) => path.replaceAll("\\", "/"))
      .sort();
    expect(owners).toEqual(["services/regional-session-api/src/main.rs"]);
  });
});
