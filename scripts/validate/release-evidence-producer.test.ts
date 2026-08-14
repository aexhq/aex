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
  capturedCommandOutput,
  assertUserJourneyInventory,
  diagnoseAdmission,
  validateHygieneReport
} = releaseEvidence;

const admitted = { status: 200, items: [] as unknown[], requestId: undefined };
const refusedAnonymously = { status: 401, items: undefined, requestId: "req-anon" };

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

  it("merges Bun reporter stderr only for user inventory captures", () => {
    const result = { stdout: "json\n", stderr: "(pass) live.registry-list\n" };
    expect(capturedCommandOutput(result)).toBe("json\n");
    expect(capturedCommandOutput(result, true)).toBe("json\n(pass) live.registry-list\n");
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

  // The lane's whole receipt set rests on one request. Twelve consecutive runs
  // failed on it and reported `left: 401, right: 200`, which does not say
  // whether the key was wrong, the row was missing, or the route was absent.
  // These pin the sentence each answer produces instead.
  it("names each admission failure class rather than reporting a bare status", () => {
    expect(diagnoseAdmission(admitted, refusedAnonymously)).toMatchObject({
      schema: "aex.release-evidence-admission-preflight.v1",
      admitted: true
    });

    expect(() => diagnoseAdmission({ status: 401, requestId: "req-1" }, refusedAnonymously))
      .toThrow(/refused the protected credential/);
    // The operator's next move is in the message, not in a runbook they have to
    // find: the request id joins this failure to the plane's admission log.
    expect(() => diagnoseAdmission({ status: 401, requestId: "req-1" }, refusedAnonymously))
      .toThrow(/req-1/);
    expect(() => diagnoseAdmission({ status: 401, requestId: "req-1" }, refusedAnonymously))
      .toThrow(/aex\.regional\.admission/);

    expect(() => diagnoseAdmission({ status: 404 }, { status: 404 }))
      .toThrow(/is not mounted on this plane/);
    expect(() => diagnoseAdmission({ status: 503 }, refusedAnonymously))
      .toThrow(/retryable/);
    expect(() => diagnoseAdmission(admitted, { status: 200, items: [] }))
      .toThrow(/admitted an anonymous caller/);
    expect(() => diagnoseAdmission({ status: 200, items: undefined }, refusedAnonymously))
      .toThrow(/omitted its items array/);
  });

  it("preflights the credential before spending the run on suites that need it", () => {
    const job = workflowJob(readWorkflow(".github/workflows/release-evidence.yml"), "evidence");
    expect(stepIndex(job, "Verify the exact deployed release before evidence")).toBeLessThan(
      stepIndex(job, "Verify the protected credential is admitted before evidence")
    );
    expect(stepIndex(job, "Verify the protected credential is admitted before evidence")).toBeLessThan(
      stepIndex(job, "Run release-bound E2E and user suites")
    );
    // The preflight observes; it must never become a second place a receipt can
    // claim a pass from. Only the suites produce evidence.
    expect(
      workflowStep(job, "Verify the protected credential is admitted before evidence").run
    ).not.toMatch(/--out\b/);
  });

  it("uses plane-qualified protected inputs, exact bytes, real suites, and attested immutable receipts", () => {
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
      "plane",
      "public_source_sha",
      "release_id",
      "release_tag",
      "release_tool_digest",
      "release_tool_size_bytes",
      "release_tool_uri"
    ]);
    expect(inputs.plane).toMatchObject({
      type: "choice",
      required: true,
      options: ["dev", "prd"]
    });
    expect(source).not.toMatch(/(?:e2e|user)_receipt_(?:uri|digest|size)/);
    expect(job["runs-on"]).toBe("ubuntu-latest");
    expect(workflow.concurrency).toEqual({
      group: "release-evidence-${{ inputs.plane }}-${{ inputs.release_id }}",
      "cancel-in-progress": false
    });
    expect(job.environment).toBe("aex-release-evidence-${{ inputs.plane }}");
    expect(job.env?.PLANE).toBe("${{ inputs.plane }}");
    expect(job.env?.AEX_RELEASE_EVIDENCE_CONFIGURED_PLANE).toBe(
      "${{ vars.AEX_RELEASE_EVIDENCE_PLANE }}"
    );
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
    expect(workflowStep(job, "Bind the protected run and create its source archive").run).toContain(
      '[[ "$PLANE" == "dev" || "$PLANE" == "prd" ]]'
    );
    expect(workflowStep(job, "Bind the protected run and create its source archive").run).toContain(
      '[[ "$AEX_RELEASE_EVIDENCE_CONFIGURED_PLANE" == "$PLANE" ]]'
    );
    expect(workflowStep(job, "Require non-empty release scenario and user-journey inventories").run).toContain("graph verify --release");
    expect(workflowStep(job, "Require non-empty release scenario and user-journey inventories").run).toContain(
      "AEX_RELEASE_EVIDENCE_MODE=inventory"
    );
    expect(workflowStep(job, "Require non-empty release scenario and user-journey inventories").run).toContain(
      'bun test apps/user-tests/test/live > "$RUN_ROOT/user/list.txt" 2>&1'
    );
    expect(source).toContain("AEX_RELEASE_EVIDENCE_HEALTH_URL");
    expect(source).toContain("https://{0}/api/release/health");
    expect(source).toContain("format('https://{0}', vars.AEX_RELEASE_EVIDENCE_API_HOST)");
    expect(source).not.toContain("secrets.AEX_RELEASE_EVIDENCE_API_URL");
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
    expect(producerSource).toContain('AEX_RELEASE_EVIDENCE_MODE: "inventory"');
    expect(producerSource).toContain('AEX_RELEASE_EVIDENCE_MODE: "execute"');
    expect(producerSource).not.toContain('"--list-tests"');
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

  it("mounts the public release-health router only in session-stream-api", () => {
    const root = resolve(import.meta.dir, "../..");
    const owners = [...new Bun.Glob("services/**/src/**/*.rs").scanSync({ cwd: root })]
      .filter((path) => readFileSync(resolve(root, path), "utf8").includes("release_health::router"))
      .map((path) => path.replaceAll("\\", "/"))
      .sort();
    expect(owners).toEqual(["services/session-stream-api/src/main.rs"]);
  });

  // The producer lists the live suite by RUNNING it under
  // AEX_RELEASE_EVIDENCE_MODE=inventory, because `bun test` has no list-only
  // mode. A test that reaches the network during that pass makes the listing
  // depend on a live plane, and the release becomes uncertifiable for as long as
  // the plane is unwell. release-evidence runs 31127031271 and 31136928714 both
  // died exactly here, first on ConnectionRefused and then on 401.
  it("every live test that reaches the network returns early under inventory mode", () => {
    const root = resolve(import.meta.dir, "../..");
    const dir = "apps/user-tests/test/live";
    const files = [...new Bun.Glob("*.test.ts").scanSync({ cwd: resolve(root, dir) })].sort();
    expect(files.length).toBeGreaterThan(0);

    const guard = 'if (process.env.AEX_RELEASE_EVIDENCE_MODE === "inventory") return;';
    const helperGuard = /function inventory\(\)[\s\S]{0,400}?AEX_RELEASE_EVIDENCE_MODE\s*===\s*"inventory"/u;
    const unguarded = files.filter((file) => {
      const source = readFileSync(resolve(root, dir, file), "utf8");
      const reachesNetwork = /\bfetch\s*\(|\brequired\s*\(\s*"AEX_/.test(source);
      const returnsEarly = source.includes(guard) || helperGuard.test(source);
      return reachesNetwork && !returnsEarly;
    });

    expect(unguarded).toEqual([]);
  });
});
