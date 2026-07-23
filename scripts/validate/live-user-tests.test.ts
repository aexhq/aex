import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";
import {
  GATE_KEY_ENV,
  GATE_PROVIDER,
  gateModel
} from "../../apps/user-tests/test/_fixtures/provider.js";
import { PUBLISHED_ARTIFACT_SMOKE_FILES } from "../../apps/user-tests/test/_fixtures/smoke-suite.js";
import {
  jobNeeds,
  readWorkflow,
  type WorkflowDocument,
  type WorkflowJob,
  type WorkflowStep,
  workflowJob,
  workflowStepBefore,
  workflowStepRunning,
  workflowStepUsing,
  workflowSteps
} from "./workflow-test-helpers.js";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

function read(path: string): string {
  // Normalize CRLF so multi-line assertions behave the same on Windows
  // checkouts (autocrlf) and CI.
  return readFileSync(resolve(repoRoot, path), "utf8").replace(/\r\n/g, "\n");
}

function steps(workflow: WorkflowDocument): readonly WorkflowStep[] {
  return Object.values(workflow.jobs).flatMap(workflowSteps);
}

function readUniqueLiveScenario(matcher: RegExp): string {
  const matches: string[] = [];
  const visit = (directory: string): void => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) visit(path);
      else if (entry.isFile() && entry.name.endsWith(".ts") && matcher.test(read(path))) matches.push(path);
    }
  };
  visit(resolve(repoRoot, "apps/user-tests/test/live"));
  if (matches.length !== 1) throw new Error(`expected one live scenario matching ${matcher}, found ${matches.length}`);
  return read(matches[0]!);
}

function runStep(job: WorkflowJob, matcher: RegExp): WorkflowStep {
  return workflowStepRunning(job, matcher);
}

function usesStep(job: WorkflowJob, action: string): WorkflowStep {
  return workflowStepUsing(job, action);
}

describe("live user-test release gate", () => {
  it("keeps every required published-artifact smoke capability backed by a test file", () => {
    const files = Object.values(PUBLISHED_ARTIFACT_SMOKE_FILES);
    expect(files.length).toBeGreaterThan(0);
    expect(new Set(files).size).toBe(files.length);
    for (const file of files) {
      expect(existsSync(resolve(repoRoot, "apps/user-tests", file)), file).toBe(true);
    }
  });

  it("uploads only a redacted live-test log artifact", () => {
    const workflow = readWorkflow(".github/workflows/live-user-tests.yml");
    const job = workflowJob(workflow, "live-user-tests");
    const live = runStep(job, /\btest:user:files\b/);
    const redact = runStep(job, /function\s+redactSignedUrls/);
    const upload = job.steps?.find((step) =>
      step.uses?.startsWith("actions/upload-artifact@") && String(step.with?.name ?? "").includes("redacted-log")
    );
    if (!upload) throw new Error("redacted log upload is missing");

    expect(job.strategy?.matrix?.include).toBe("${{ fromJSON(needs.prepare-live-test-matrix.outputs.test_matrix) }}");
    expect(live.env).toMatchObject({
      AEX_USER_TEST_MAX_WORKERS: 1,
      AEX_USER_TEST_RUNTIME_KIND: "${{ matrix.runtimeKind }}",
      AEX_USER_TEST_CANDIDATE_IDENTITY: "${{ needs.prepare-artifact.outputs.candidate_identity }}",
      TEST_FILE: "${{ matrix.file }}",
      RAW_LOG: "${{ github.workspace }}/.suite-diagnostics/raw/live-user-tests-shard-${{ matrix.shard }}.log",
      REPORT: "${{ github.workspace }}/.suite-diagnostics/raw/live-user-tests-shard-${{ matrix.shard }}.report.json"
    });
    expect(live.run).toMatch(/--reporter=junit\s+--reporter-outfile="\$REPORT"/);
    expect(live.run).not.toMatch(/--reporter=json|outputFile\.json/);
    expect(live.run).toMatch(/assert-no-skips\.mjs.*REPORT/);
    expect(live.run).toMatch(/>\s*"\$RAW_LOG"\s+2>&1/);
    expect(live.run).not.toMatch(/\btee\b/);
    expect(live.run).toMatch(/test_status=\$\?/);
    expect(live.run).toMatch(/exit\s+"\$test_status"/);
    expect(redact.env).toMatchObject({
      REDACTED_LOG: "${{ github.workspace }}/.suite-diagnostics/redacted/live-user-tests-shard-${{ matrix.shard }}.log"
    });
    expect(redact.run).toMatch(/AEX_API_KEY/);
    expect(redact.run).toMatch(/DEEPSEEK_API_KEY/);
    expect(redact.run).toMatch(/split\(value\).*REDACTED/);
    expect(redact.run).toMatch(/cat\s+"\$REDACTED_LOG"/);
    expect(upload.with).toMatchObject({
      path: ".suite-diagnostics/redacted",
      "retention-days": 7
    });
    expect(JSON.stringify(upload.with)).not.toContain(".suite-diagnostics/raw");
  });

  it("requires evidence-bound runtime verdict closure for every authenticated paired matrix cell", () => {
    const workflow = readWorkflow(".github/workflows/live-user-tests.yml");
    const live = workflowJob(workflow, "live-user-tests");
    const aggregate = workflowJob(workflow, "runtime-parity-verdict");
    const emit = runStep(live, /runtime-parity-verdicts\.mjs\s+emit/);
    const validate = runStep(aggregate, /runtime-parity-verdicts\.mjs\s+validate/);

    expect(emit.if).toBe("${{ always() && matrix.runtimeKind != null }}");
    expect(emit.env).toMatchObject({
      PARITY_CELLS: "${{ toJSON(matrix.parityCells) }}",
      STEP_OUTCOME: "${{ steps.live-test.outcome }}",
      CANDIDATE_IDENTITY: "${{ needs.prepare-artifact.outputs.candidate_identity }}"
    });
    expect(jobNeeds(aggregate)).toContain("live-user-tests");
    expect(validate.env).toMatchObject({
      TEST_MATRIX: "${{ needs.prepare-live-test-matrix.outputs.test_matrix }}",
      CANDIDATE_IDENTITY: "${{ needs.prepare-artifact.outputs.candidate_identity }}"
    });
    expect(validate.run).toMatch(/--verdict-directory\s+"\$RUNNER_TEMP\/runtime-parity-verdicts"/);
  });

  it("preserves the exact prepared canary identity through install and verdict aggregation", () => {
    const workflow = readWorkflow(".github/workflows/live-user-tests.yml");
    const prepare = workflowJob(workflow, "prepare-artifact");
    const live = workflowJob(workflow, "live-user-tests");
    const select = workflowStepRunning(prepare, /candidate_identity=/);
    const use = runStep(live, /downloaded_identity=/);

    expect((prepare.outputs as Record<string, unknown> | undefined)?.candidate_identity).toBe(
      "${{ steps.artifact.outputs.candidate_identity }}"
    );
    expect(select.run).toMatch(/candidate_identity=.*PACKAGE_VERSION/);
    expect(select.run).toMatch(/candidate_identity=sha256:.*digest/);
    expect(use.env?.CANDIDATE_IDENTITY).toBe("${{ needs.prepare-artifact.outputs.candidate_identity }}");
    expect(use.run).toMatch(/downloaded_identity.*CANDIDATE_IDENTITY/);
  });

  it("never writes raw hosted-test output to public Actions logs", () => {
    const cases = [
      {
        job: workflowJob(readWorkflow(".github/workflows/live-user-tests.yml"), "live-user-tests"),
        command: /\btest:user:files\b/
      },
      {
        job: workflowJob(readWorkflow(".github/workflows/release.yml"), "live-user-tests"),
        command: /\btest:user:smoke\b/
      }
    ];

    for (const { job, command } of cases) {
      const live = runStep(job, command);
      const redact = runStep(job, /function\s+redactSignedUrls/);
      expect(live.run).toMatch(/>\s*"\$RAW_LOG"\s+2>&1/);
      expect(live.run).not.toMatch(/\btee\b/);
      expect(live.run).toMatch(/test_status=\$\?/);
      expect(live.run).toMatch(/exit\s+"\$test_status"/);
      expect(redact?.if).toBe("${{ always() }}");
      expect(redact?.run).toMatch(/cat\s+"\$REDACTED_LOG"/);
      expect(redact?.run).not.toMatch(/cat\s+"\$RAW_LOG"/);
    }
  });

  it("redacts signed object-storage URLs from live-test artifacts", () => {
    for (const path of [".github/workflows/live-user-tests.yml", ".github/workflows/release.yml"]) {
      const redaction = steps(readWorkflow(path)).find((step) => /function\s+redactSignedUrls/.test(step.run ?? ""))?.run;
      expect(redaction, path).toBeDefined();
      expect(redaction, path).toMatch(/redactSignedUrls\(text\)/);
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
      const run = runStep(preflight, /preflight-live-user-tests\.mjs/);
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
    const releaseCapacity = runStep(workflowJob(release, "live-user-tests-preflight"), /preflight-live-user-tests\.mjs/)
      .env?.LIVE_USER_TEST_MIN_MAX_CONCURRENT_SESSIONS;
    const releaseWorkers = runStep(workflowJob(release, "live-user-tests"), /\btest:user:smoke\b/)
      .env?.AEX_USER_TEST_MAX_WORKERS;
    expect(releaseCapacity).toBe(releaseWorkers);
    expect(runStep(workflowJob(release, "live-user-tests-preflight"), /preflight-live-user-tests\.mjs/)
      .env?.LIVE_USER_TEST_REQUIRED_SCOPES).toBe("sessions:read,sessions:write,files:read");
    expect(runStep(workflowJob(release, "live-user-tests-preflight"), /preflight-live-user-tests\.mjs/)
      .env?.LIVE_USER_TEST_REQUIRED_RUNTIME_KINDS).toBe("");

    const liveJob = workflowJob(live, "live-user-tests");
    const liveCapacity = runStep(workflowJob(live, "live-user-tests-preflight"), /preflight-live-user-tests\.mjs/)
      .env?.LIVE_USER_TEST_MIN_MAX_CONCURRENT_SESSIONS;
    const liveWorkers = runStep(liveJob, /\btest:user:files\b/).env?.AEX_USER_TEST_MAX_WORKERS;
    expect(liveCapacity).toBe(1);
    expect(liveWorkers).toBe(1);
    expect(jobNeeds(workflowJob(live, "live-user-tests-preflight"))).toContain("prepare-artifact");
    expect(jobNeeds(liveJob)).toContain("prepare-live-test-matrix");
    expect(runStep(workflowJob(live, "live-user-tests-preflight"), /preflight-live-user-tests\.mjs/)
      .env?.LIVE_USER_TEST_REQUIRED_SCOPES).toBe(
      "sessions:read,sessions:write,sessions:cancel,sessions:delete,files:read,files:write,assets:write," +
      "skills:write,tools:write,instructions:write,secrets:read,secrets:write"
    );
    expect(runStep(workflowJob(live, "live-user-tests-preflight"), /preflight-live-user-tests\.mjs/)
      .env?.LIVE_USER_TEST_REQUIRED_RUNTIME_KINDS).toBeUndefined();
  });

  it("derives one live job per discovered file while keeping release smoke focused", () => {
    const workflow = readWorkflow(".github/workflows/live-user-tests.yml");
    const prepare = workflowJob(workflow, "prepare-artifact");
    const prepareMatrix = workflowJob(workflow, "prepare-live-test-matrix");
    const live = workflowJob(workflow, "live-user-tests");
    const release = workflowJob(readWorkflow(".github/workflows/release.yml"), "live-user-tests");
    const liveRun = runStep(live, /\btest:user:files\b/);
    const smokeRun = runStep(release, /\btest:user:smoke\b/);

    const matrixStep = runStep(prepareMatrix, /shard-files\.mjs\s+--matrix/);
    workflowStepBefore(prepareMatrix, usesStep(prepareMatrix, "oven-sh/setup-bun@"), matrixStep);
    expect(matrixStep.run).toMatch(/\bbun\s+apps\/user-tests\/scripts\/shard-files\.mjs\b/);
    expect(matrixStep.run).not.toMatch(/\bnode\s+apps\/user-tests\/scripts\/shard-files\.mjs\b/);
    expect(matrixStep.run).toMatch(/--runtime-capabilities-json\s+"\$RUNTIME_CAPABILITIES"/);
    expect(matrixStep.env?.RUNTIME_CAPABILITIES).toBe(
      "${{ needs.live-user-tests-preflight.outputs.runtime_capabilities }}"
    );
    expect(matrixStep.run).toMatch(/\$peak_session_slots.*-gt.*\$MAX_CONCURRENT_SESSIONS/);
    expect(matrixStep.run).toMatch(/matrix=.*GITHUB_OUTPUT/);
    expect(matrixStep.run).toMatch(/peak_session_slots=.*GITHUB_OUTPUT/);
    expect(matrixStep.run).toMatch(/entry\.sessionSlots/);
    expect(prepare.outputs).not.toHaveProperty("test_count");
    expect(live.strategy?.matrix?.include).toBe("${{ fromJSON(needs.prepare-live-test-matrix.outputs.test_matrix) }}");
    expect(live.strategy?.["max-parallel"]).toBeUndefined();
    expect(liveRun.env?.AEX_USER_TEST_MAX_WORKERS).toBe(1);
    expect(liveRun.env?.TEST_FILE).toBe("${{ matrix.file }}");
    expect(liveRun.run).toMatch(/\btest:user:files\b/);
    expect(liveRun.run).toMatch(/\$TEST_FILE/);
    expect(liveRun.run).not.toMatch(/--shard(?:\s|$)/);
    expect(jobNeeds(release)).toContain("publish");
    expect(smokeRun.env?.AEX_USER_TEST_MAX_WORKERS).toBe(2);
    expect(smokeRun.run).toMatch(/--reporter=junit\s+--reporter-outfile="\$REPORT"/);
    expect(smokeRun.run).not.toMatch(/--reporter=json|outputFile\.json/);
    expect(smokeRun.run).toMatch(/assert-no-skips\.mjs.*REPORT/);
    expect(smokeRun.run).not.toMatch(/shard-files\.mjs/);
    expect(usesStep(release, "actions/upload-artifact@").with?.name).toMatch(/redacted-log/);
  });

  it("keeps BYOK leak probes on the canonical no-tool API", () => {
    const source = readUniqueLiveScenario(/const probe = rand\("byok-echo"\)/);

    expect(source).toContain('const probe = rand("byok-echo");');
    expect(source).toContain('message: "Reply with exactly this text and nothing else: " + probe');
    expect(source).not.toContain("SessionFile verbatim");
    expect(source).not.toContain("keyleak-probe");
    expect(source).not.toContain("includeBuiltinTools:");
    expect(source).toContain('builtinTools: "none"');
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
      /\btest:user:providers\b/
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
    expect(runStep(providers, /\btest:user:providers\b/)).toBeDefined();
    expect(runStep(heavy, /\btest:user:heavy\b/)).toBeDefined();
    expect(workflow.jobs).not.toHaveProperty("tool-fuzz-tests");
  });

  it("keeps lineage observability scratch file inside the live-test sandbox", () => {
    const source = readUniqueLiveScenario(/lineage-wave1-out\.json/);

    expect(source).toContain('writeFileSync(join(install.installDir, "lineage-wave1-out.json")');
    expect(source).not.toContain("C:/Users/");
    expect(source).not.toContain("/tmp/claude/");
    expect(source).not.toContain("scratchpad/lineage-wave1-out.json");
  });
});
