import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  GATE_KEY_ENV,
  GATE_PROVIDER,
  gateModel
} from "../../apps/user-tests/test/_fixtures/provider.js";
import {
  jobNeeds,
  readWorkflow,
  type WorkflowDocument,
  type WorkflowJob,
  type WorkflowStep,
  workflowJob
} from "./workflow-test-helpers.js";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

function read(path: string): string {
  // Normalize CRLF so multi-line assertions behave the same on Windows
  // checkouts (autocrlf) and CI.
  return readFileSync(resolve(repoRoot, path), "utf8").replace(/\r\n/g, "\n");
}

function steps(workflow: WorkflowDocument): readonly WorkflowStep[] {
  return Object.values(workflow.jobs).flatMap((job) => job.steps ?? []);
}

function runStep(job: WorkflowJob, needle: string): WorkflowStep {
  const step = job.steps?.find((candidate) => candidate.run?.includes(needle));
  if (!step) throw new Error(`workflow step containing ${JSON.stringify(needle)} is missing`);
  return step;
}

function usesStep(job: WorkflowJob, action: string): WorkflowStep {
  const step = job.steps?.find((candidate) => candidate.uses?.startsWith(action));
  if (!step) throw new Error(`workflow step using ${action} is missing`);
  return step;
}

describe("live user-test release gate", () => {
  it("uploads only a redacted live-test log artifact", () => {
    const workflow = readWorkflow(".github/workflows/live-user-tests.yml");
    const job = workflowJob(workflow, "live-user-tests");
    const live = runStep(job, "bun run test:user:files");
    const redact = runStep(job, "function redactSignedUrls");
    const upload = usesStep(job, "actions/upload-artifact@");

    expect(job.strategy?.matrix?.include).toBe("${{ fromJSON(needs.prepare-artifact.outputs.test_matrix) }}");
    expect(live.env).toMatchObject({
      AEX_USER_TEST_MAX_WORKERS: 1,
      TEST_FILE: "${{ matrix.file }}",
      RAW_LOG: "${{ github.workspace }}/.suite-diagnostics/raw/live-user-tests-shard-${{ matrix.shard }}.log",
      REPORT: "${{ github.workspace }}/.suite-diagnostics/raw/live-user-tests-shard-${{ matrix.shard }}.report.json"
    });
    expect(live.run).toContain('--outputFile.json="$REPORT"');
    expect(live.run).toContain('assert-no-skips.mjs "$REPORT"');
    expect(redact.env).toMatchObject({
      REDACTED_LOG: "${{ github.workspace }}/.suite-diagnostics/redacted/live-user-tests-shard-${{ matrix.shard }}.log"
    });
    expect(redact.run).toContain('["AEX_API_KEY", "DEEPSEEK_API_KEY"]');
    expect(redact.run).toContain("text.split(value).join(`[REDACTED:${name}]`)");
    expect(upload.with).toMatchObject({
      path: ".suite-diagnostics/redacted",
      "retention-days": 7
    });
    expect(JSON.stringify(upload.with)).not.toContain(".suite-diagnostics/raw");
  });

  it("redacts signed object-storage URLs from live-test artifacts", () => {
    for (const path of [".github/workflows/live-user-tests.yml", ".github/workflows/release.yml"]) {
      const redaction = steps(readWorkflow(path)).find((step) => step.run?.includes("function redactSignedUrls"))?.run;
      expect(redaction, path).toBeDefined();
      expect(redaction, path).toContain("text = redactSignedUrls(text);");
      expect(redaction, path).toContain("X-Amz-");
      expect(redaction, path).toContain("X-Goog-");
      expect(redaction, path).toContain("?[redacted]");
      expect(redaction, path).toContain("Security-Token");
    }
  });

  it("preflights before live execution without blocking immutable canary publication", () => {
    const workflows = [
      readWorkflow(".github/workflows/release.yml"),
      readWorkflow(".github/workflows/live-user-tests.yml")
    ];
    expect(jobNeeds(workflowJob(workflows[0]!, "publish"))).not.toContain("live-user-tests-preflight");
    expect(jobNeeds(workflowJob(workflows[0]!, "live-user-tests"))).toContain("live-user-tests-preflight");
    expect(jobNeeds(workflowJob(workflows[1]!, "live-user-tests"))).toContain("live-user-tests-preflight");

    for (const workflow of workflows) {
      const preflight = workflowJob(workflow, "live-user-tests-preflight");
      const run = runStep(preflight, "preflight-live-user-tests.mjs");
      expect(usesStep(preflight, "actions/checkout@").uses).toBe("actions/checkout@v6");
      expect(usesStep(preflight, "oven-sh/setup-bun@").with).toMatchObject({ "bun-version": "1.3.14" });
      expect(run.env).toMatchObject({
        AEX_API_URL: "${{ vars.AEX_API_URL }}",
        AEX_EXPECTED_API_HOST: "${{ vars.AEX_EXPECTED_API_HOST || 'dev-api.aex.dev' }}",
        AEX_API_KEY: "${{ secrets.AEX_API_KEY }}",
        DEEPSEEK_API_KEY: "${{ secrets.DEEPSEEK_API_KEY }}"
      });
      expect(JSON.stringify(workflow)).not.toContain("AEX_API_TOKEN");
    }
    const release = workflows[0]!;
    const live = workflows[1]!;
    const releaseCapacity = runStep(workflowJob(release, "live-user-tests-preflight"), "preflight-live-user-tests.mjs")
      .env?.LIVE_USER_TEST_MIN_MAX_CONCURRENT_SESSIONS;
    const releaseWorkers = runStep(workflowJob(release, "live-user-tests"), "test:user:smoke")
      .env?.AEX_USER_TEST_MAX_WORKERS;
    expect(releaseCapacity).toBe(releaseWorkers);
    expect(runStep(workflowJob(release, "live-user-tests-preflight"), "preflight-live-user-tests.mjs")
      .env?.LIVE_USER_TEST_REQUIRED_SCOPES).toBe("sessions:read,sessions:write,files:read");

    const liveJob = workflowJob(live, "live-user-tests");
    const liveCapacity = runStep(workflowJob(live, "live-user-tests-preflight"), "preflight-live-user-tests.mjs")
      .env?.LIVE_USER_TEST_MIN_MAX_CONCURRENT_SESSIONS;
    const liveWorkers = runStep(liveJob, "test:user:files").env?.AEX_USER_TEST_MAX_WORKERS;
    expect(liveCapacity).toBe("${{ needs.prepare-artifact.outputs.test_count }}");
    expect(liveWorkers).toBe(1);
    expect(jobNeeds(workflowJob(live, "live-user-tests-preflight"))).toContain("prepare-artifact");
    expect(runStep(workflowJob(live, "live-user-tests-preflight"), "preflight-live-user-tests.mjs")
      .env?.LIVE_USER_TEST_REQUIRED_SCOPES).toBe(
      "sessions:read,sessions:write,sessions:cancel,sessions:delete,files:read,files:write,assets:write," +
      "skills:write,tools:write,instructions:write,secrets:read,secrets:write"
    );
  });

  it("derives one live job per discovered file while keeping release smoke focused", () => {
    const workflow = readWorkflow(".github/workflows/live-user-tests.yml");
    const prepare = workflowJob(workflow, "prepare-artifact");
    const live = workflowJob(workflow, "live-user-tests");
    const release = workflowJob(readWorkflow(".github/workflows/release.yml"), "live-user-tests");
    const liveRun = runStep(live, "test:user:files");
    const smokeRun = runStep(release, "test:user:smoke");

    const matrixStep = runStep(prepare, "shard-files.mjs --matrix");
    expect(matrixStep.run).toContain('echo "matrix=$matrix" >> "$GITHUB_OUTPUT"');
    expect(matrixStep.run).toContain('echo "count=$count" >> "$GITHUB_OUTPUT"');
    expect(live.strategy?.matrix?.include).toBe("${{ fromJSON(needs.prepare-artifact.outputs.test_matrix) }}");
    expect(live.strategy?.["max-parallel"]).toBeUndefined();
    expect(liveRun.env?.AEX_USER_TEST_MAX_WORKERS).toBe(1);
    expect(liveRun.env?.TEST_FILE).toBe("${{ matrix.file }}");
    expect(liveRun.run).toContain('test:user:files -- "$TEST_FILE"');
    expect(liveRun.run).not.toContain("--shard");
    expect(jobNeeds(release)).toContain("publish");
    expect(smokeRun.env?.AEX_USER_TEST_MAX_WORKERS).toBe(2);
    expect(smokeRun.run).not.toContain("shard-files.mjs");
    expect(usesStep(release, "actions/upload-artifact@").with?.name).toBe("release-smoke-redacted-log");
  });

  it("keeps BYOK leak probes as no-tool exact-reply turns", () => {
    const source = read("apps/user-tests/test/live/edge-byok-secrets.user.test.ts");

    expect(source).toContain('const probe = rand("byok-echo");');
    expect(source).toContain('message: "Reply with exactly this text and nothing else: " + probe');
    expect(source).not.toContain("SessionFile verbatim");
    expect(source).not.toContain("keyleak-probe");
    expect(source.match(/includeBuiltinTools: false/g) ?? []).toHaveLength(4);
    expect(source.match(/tools: \[\]/g) ?? []).toHaveLength(4);
    expect(source.match(/overrides: \{ maxTurns: 3 \}/g) ?? []).toHaveLength(4);
  });

  it("serializes Bun installs across live-test worker processes", () => {
    const source = read("apps/user-tests/test/_fixtures/install.ts");

    expect(source).toContain("Bun keeps a process-external global package cache");
    expect(source).toContain("aex-user-test-bun-install-");
    expect(source).toContain("withInstallLock(async () => runBun(args, { cwd: installDir }, timeoutMs))");
    expect(source).toContain("timed out waiting for Bun install lock");
  });

  it("treats blank live-test model vars as missing", () => {
    const name = "AEX_USER_TEST_DEEPSEEK_MODEL";
    const previous = process.env[name];
    try {
      delete process.env[name];
      expect(gateModel()).toBe("deepseek-v4-flash");
      process.env[name] = "   ";
      expect(gateModel()).toBe("deepseek-v4-flash");
      process.env[name] = "  deepseek-custom  ";
      expect(gateModel()).toBe("deepseek-custom");
    } finally {
      if (previous === undefined) delete process.env[name];
      else process.env[name] = previous;
    }
  });

  it("keeps the release gate on the DeepSeek gate provider only (no Anthropic billing dependency)", () => {
    expect(GATE_PROVIDER).toBe("deepseek");
    expect(GATE_KEY_ENV).toBe("DEEPSEEK_API_KEY");
    for (const path of [".github/workflows/live-user-tests.yml", ".github/workflows/release.yml"]) {
      const serialized = JSON.stringify(readWorkflow(path));
      expect(serialized, path).not.toContain("ANTHROPIC_API_KEY");
      expect(serialized, path).not.toContain("AEX_USER_TEST_ANTHROPIC_MODEL");
      expect(serialized, path).toContain("DEEPSEEK_API_KEY");
      expect(serialized, path).not.toContain("test:user:providers");
    }

    const providerRun = runStep(
      workflowJob(readWorkflow(".github/workflows/live-on-demand-tests.yml"), "provider-tests"),
      "test:user:providers"
    );
    expect(providerRun.env).toMatchObject({
      ANTHROPIC_API_KEY: "${{ secrets.ANTHROPIC_API_KEY }}"
    });
  });

  it("fans out provider and heavy on-demand suites after one artifact preparation", () => {
    const workflow = readWorkflow(".github/workflows/live-on-demand-tests.yml");
    const providers = workflowJob(workflow, "provider-tests");
    const heavy = workflowJob(workflow, "heavy-session");
    expect(jobNeeds(providers)).toEqual(["prepare-artifact"]);
    expect(jobNeeds(heavy)).toEqual(["prepare-artifact"]);
    expect(runStep(providers, "test:user:providers")).toBeDefined();
    expect(runStep(heavy, "test:user:heavy")).toBeDefined();
    expect(workflow.jobs).not.toHaveProperty("tool-fuzz-tests");
  });

  it("keeps lineage observability scratch file inside the live-test sandbox", () => {
    const source = read("apps/user-tests/test/live/edge-lineage-observability.user.test.ts");

    expect(source).toContain('writeFileSync(join(install.installDir, "lineage-wave1-out.json")');
    expect(source).not.toContain("C:/Users/");
    expect(source).not.toContain("/tmp/claude/");
    expect(source).not.toContain("scratchpad/lineage-wave1-out.json");
  });
});
