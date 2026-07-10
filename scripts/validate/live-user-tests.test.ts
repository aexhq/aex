import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

function read(path: string): string {
  // Normalize CRLF so multi-line assertions behave the same on Windows
  // checkouts (autocrlf) and CI.
  return readFileSync(resolve(repoRoot, path), "utf8").replace(/\r\n/g, "\n");
}

describe("live user-test release gate", () => {
  it("uploads only a redacted live-test log artifact", () => {
    const workflow = read(".github/workflows/live-user-tests.yml");

    expect(workflow).toContain("name: Live user tests shard ${{ matrix.shard }}/50");
    expect(workflow).toContain("shard: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12");
    expect(workflow).toContain("RAW_LOG: ${{ github.workspace }}/.suite-diagnostics/raw/live-user-tests-shard-${{ matrix.shard }}.log");
    expect(workflow).toContain("REPORT: ${{ github.workspace }}/.suite-diagnostics/raw/live-user-tests-shard-${{ matrix.shard }}.report.json");
    expect(workflow).toContain('bun run test:user:files -- $FILES --reporter=default --reporter=json --outputFile.json="$REPORT" 2>&1 | tee "$RAW_LOG"');
    expect(workflow).toContain('node scripts/cicd/assert-no-skips.mjs "$REPORT" 2>&1 | tee -a "$RAW_LOG"');
    expect(workflow).toContain("AEX_USER_TEST_MAX_WORKERS: 1");
    expect(workflow).toContain("Redact live user test log");
    expect(workflow).toContain("Upload redacted live user test log");
    expect(workflow).toContain("REDACTED_LOG: ${{ github.workspace }}/.suite-diagnostics/redacted/live-user-tests-shard-${{ matrix.shard }}.log");
    expect(workflow).toContain('mkdir -p "$(dirname "$RAW_LOG")"');
    expect(workflow).toContain('mkdir -p "$(dirname "$REDACTED_LOG")"');
    expect(workflow).toContain("path: .suite-diagnostics/redacted");
    expect(workflow).toContain("retention-days: 7");
    expect(workflow).toContain('["AEX_API_KEY", "DEEPSEEK_API_KEY"]');
    expect(workflow).toContain("text.split(value).join(`[REDACTED:${name}]`)");
    expect(workflow).not.toContain("path: .suite-diagnostics/raw");
  });

  it("redacts signed object-storage URLs from live-test artifacts", () => {
    for (const path of [".github/workflows/live-user-tests.yml", ".github/workflows/release.yml"]) {
      const workflow = read(path);

      expect(workflow, path).toContain("text = redactSignedUrls(text);");
      expect(workflow, path).toContain("function redactSignedUrls(input)");
      expect(workflow, path).toContain("X-Amz-");
      expect(workflow, path).toContain("X-Goog-");
      expect(workflow, path).toContain("?[redacted]");
      expect(workflow, path).toContain("Security-Token");
    }
  });

  it("preflights live user-test environment before publishing or sharding", () => {
    const release = read(".github/workflows/release.yml");
    const live = read(".github/workflows/live-user-tests.yml");

    expect(release).toContain("live-user-tests-preflight:");
    expect(release).toContain("name: Live user tests preflight");
    expect(release).toContain("- live-user-tests-preflight");
    expect(release.indexOf("live-user-tests-preflight:")).toBeLessThan(release.indexOf("publish:"));
    expect(release.indexOf("- live-user-tests-preflight")).toBeLessThan(release.indexOf("npm-release"));

    expect(live).toContain("live-user-tests-preflight:");
    expect(live).toContain("name: Live user tests preflight");
    expect(live).toContain("- live-user-tests-preflight");
    expect(live.indexOf("live-user-tests-preflight:")).toBeLessThan(live.indexOf("  live-user-tests:"));

    for (const workflow of [release, live]) {
      const preflightStart = workflow.indexOf("live-user-tests-preflight:");
      const preflightEnd = workflow.indexOf("run: bun scripts/cicd/preflight-live-user-tests.mjs", preflightStart);
      const preflightBlock = workflow.slice(preflightStart, preflightEnd);

      expect(preflightBlock).toContain("uses: actions/checkout@v6");
      expect(preflightBlock).toContain("uses: oven-sh/setup-bun@v2");
      expect(preflightBlock).toContain('bun-version: "1.3.14"');
      expect(workflow).toContain("AEX_API_URL: ${{ vars.AEX_API_URL }}");
      expect(workflow).toContain("AEX_EXPECTED_API_HOST: ${{ vars.AEX_EXPECTED_API_HOST || 'dev-api.aex.dev' }}");
      expect(workflow).toContain("AEX_API_KEY: ${{ secrets.AEX_API_KEY }}");
      expect(workflow).toContain("DEEPSEEK_API_KEY: ${{ secrets.DEEPSEEK_API_KEY }}");
      expect(workflow).toContain("LIVE_USER_TEST_REQUIRED_SCOPES: sessions:read,sessions:write,files:read");
      expect(workflow).toContain("bun scripts/cicd/preflight-live-user-tests.mjs");
      expect(workflow).toContain("LIVE_USER_TEST_MIN_MAX_CONCURRENT_SESSIONS: 50");
      expect(workflow).not.toContain("AEX_API_TOKEN");
    }
    const preflight = read("scripts/cicd/preflight-live-user-tests.mjs");
    expect(preflight).toContain("live-user-tests environment is missing required value(s)");
    expect(preflight).toContain('new URL("/api/whoami"');
    expect(preflight).toContain("isRetryableWhoamiStatus");
    expect(preflight).toContain("retryAfterDelayMs");
    expect(preflight).toContain('res.headers.get("retry-after")');
    expect(preflight).toContain('res.headers.get("x-amzn-requestid")');
    expect(preflight).toContain('res.headers.get("x-request-id")');
    expect(preflight).toContain('res.headers.get("apigw-requestid")');
    expect(preflight).toContain("transient HTTP");
    expect(preflight).toContain("limits.maxConcurrentSessions");
    expect(preflight).toContain("maxConcurrentSessions=${maxConcurrentSessions}");
    expect(preflight).toContain("DEFAULT_REQUIRED_SCOPES");
    expect(preflight).toContain("missing required scope(s)");
    expect(preflight).toContain("AEX_EXPECTED_API_HOST");
    expect(preflight).toContain("LIVE_USER_TEST_MAX_MAX_CONCURRENT_SESSIONS");
  });

  it("keeps the full live matrix in live-user-tests.yml and release.yml on published-artifact smoke", () => {
    const live = read(".github/workflows/live-user-tests.yml");
    const release = read(".github/workflows/release.yml");
    const strategy = live.indexOf("strategy:");
    const matrix = live.indexOf("matrix:", strategy);

    expect(strategy).toBeGreaterThan(-1);
    expect(matrix).toBeGreaterThan(strategy);
    expect(live).toContain("name: Live user tests shard ${{ matrix.shard }}/50");
    expect(live).toContain("shard: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12");
    expect(live).toContain("48, 49, 50]");
    expect(live).toContain("maxConcurrentSessions >= 50");
    expect(live).toContain("AEX_USER_TEST_MAX_WORKERS: 1");
    expect(live).not.toContain("max-parallel: 1");

    expect(release).toContain("name: Published-artifact smoke");
    expect(release).toContain("needs: publish");
    expect(release).toContain("bun run test:user:smoke");
    expect(release).toContain("AEX_USER_TEST_MAX_WORKERS: 2");
    expect(release).toContain("release-smoke-redacted-log");
    expect(release).toContain("full behavioral matrix is the");
    expect(release).not.toContain("name: Live user tests shard");
    expect(release).not.toContain("scripts/shard-files.mjs --shard");
  });

  it("keeps published CLI smoke on the current public start verb", () => {
    const installedSource = read("apps/user-tests/test/live/live-cli-installed.test.ts");
    const edgeSource = read("apps/user-tests/test/live/edge-cli.user.test.ts");

    for (const [path, source] of [
      ["apps/user-tests/test/live/live-cli-installed.test.ts", installedSource],
      ["apps/user-tests/test/live/edge-cli.user.test.ts", edgeSource]
    ] as const) {
      expect(source, path).toContain('"start",');
      expect(source, path).not.toMatch(/(?:executeCli|runCommand)\(\s*\[\s*"run"\s*,/s);
      expect(source, path).not.toContain("submits with run --follow");
    }
    expect(installedSource).toContain("submits with start --follow");
  });

  it("keeps the published DeepSeek SDK smoke status sourced from the start result", () => {
    const source = read("apps/user-tests/test/live/live-sdk-deepseek.test.ts");

    expect(source).toContain("const run = {");
    expect(source).toContain("sessionStatus: run.status");
    expect(source).not.toContain("sessionStatus: session.status");
  });

  it("keeps generated live SDK scripts from reading status off session handles", () => {
    const liveDir = resolve(repoRoot, "apps/user-tests/test/live");
    const paths = [
      ...readdirSync(liveDir)
        .filter((name) => name.endsWith(".ts"))
        .map((name) => `apps/user-tests/test/live/${name}`),
      ...readdirSync(resolve(liveDir, "providers"))
        .filter((name) => name.endsWith(".ts"))
        .map((name) => `apps/user-tests/test/live/providers/${name}`)
    ];

    for (const path of paths) {
      const source = read(path);
      expect(source, path).not.toContain("sessionStatus: session.status");
    }
  });

  it("keeps the high-concurrency exact-reply probe from using missing-file wording", () => {
    const source = read("apps/user-tests/test/live/edge-concurrency-scale.user.test.ts");

    expect(source).toContain("Reply with exactly this marker and no other text:");
    expect(source).not.toContain("SessionFile verbatim");
  });

  it("serializes Bun installs across live-test worker processes", () => {
    const source = read("apps/user-tests/test/_fixtures/install.ts");

    expect(source).toContain("Bun keeps a process-external global package cache");
    expect(source).toContain("aex-user-test-bun-install-");
    expect(source).toContain("withInstallLock(async () => runBun(args, { cwd: installDir }, timeoutMs))");
    expect(source).toContain("timed out waiting for Bun install lock");
  });

  it("shards live user tests by recorded duration, not file count", () => {
    // vitest --shard splits by file count (per-file live durations vary
    // ~1s..6.5min, giving 1m42s..12m7s shard walls, and shard 12/12 once
    // collected ZERO tests). Both release-gate workflows must instead ask
    // scripts/shard-files.mjs for an explicit, duration-balanced,
    // guaranteed-non-empty file list BEFORE invoking vitest. release.yml is now
    // a published-artifact smoke only; the full matrix lives here and in platform
    // deploy gates.
    const workflow = read(".github/workflows/live-user-tests.yml");

    expect(workflow).toContain(
      'FILES="$(node apps/user-tests/scripts/shard-files.mjs --shard ${{ matrix.shard }}/50)"'
    );
    expect(workflow).toContain("REPORT: ${{ github.workspace }}/.suite-diagnostics/raw/");
    expect(workflow).not.toContain("REPORT: .suite-diagnostics/raw/");
    expect(workflow).toContain('bun run test:user:files -- $FILES --reporter=default --reporter=json --outputFile.json="$REPORT" 2>&1 | tee "$RAW_LOG"');
    expect(workflow).toContain('node scripts/cicd/assert-no-skips.mjs "$REPORT" 2>&1 | tee -a "$RAW_LOG"');
    expect(workflow).not.toContain("--shard=");
    const shardCall = workflow.indexOf("scripts/shard-files.mjs --shard");
    const vitestCall = workflow.indexOf("bun run test:user:files -- $FILES");
    expect(shardCall).toBeGreaterThan(-1);
    expect(vitestCall).toBeGreaterThan(shardCall);

    // The explicit-file lane must NOT carry the catch-all `test` positional
    // filter — it would match every file and defeat the shard list.
    const packageJson = JSON.parse(read("apps/user-tests/package.json")) as {
      scripts?: Record<string, string>;
    };
    expect(packageJson.scripts?.["test:user:files"]).toBe(
      "bun scripts/user-vitest.mjs --config vitest.config.ts"
    );
    expect(packageJson.scripts?.["test:user:admission-gates"]).toBe(
      "bun scripts/user-vitest.mjs --config vitest.admission-gates.config.ts"
    );
    const rootPackageJson = JSON.parse(read("package.json")) as { scripts?: Record<string, string> };
    expect(rootPackageJson.scripts?.["test:user:files"]).toBe(
      "bun run --filter @aexhq/user-tests test:user:files"
    );
    expect(rootPackageJson.scripts?.["test:user:admission-gates"]).toBe(
      "bun run --filter @aexhq/user-tests test:user:admission-gates"
    );

    // The bin-packer's exclude list must mirror vitest.config.ts so both
    // collect the same file set.
    const script = read("apps/user-tests/scripts/shard-files.mjs");
    const vitestConfig = read("apps/user-tests/vitest.config.ts");
    const admissionGateConfig = read("apps/user-tests/vitest.admission-gates.config.ts");
    for (const excluded of [
      "test/live/edge-admission-gates.user.test.ts",
      "test/live/live-sdk-heavy-session.test.ts",
      "test/live/live-api-fuzz.test.ts",
      "test/live/live-sdk-tool-capability-fuzz.test.ts"
    ]) {
      expect(script).toContain(`"${excluded}"`);
      expect(vitestConfig).toContain(`"${excluded}"`);
    }
    expect(admissionGateConfig).toContain('"test/live/edge-admission-gates.user.test.ts"');
    expect(admissionGateConfig).not.toContain('"test/live/live-sdk-heavy-session.test.ts"');
    expect(script).toContain("shard-durations.json");
  });

  it("keeps shard flags out of conformance prebuilds", () => {
    const packageJson = JSON.parse(read("apps/user-tests/package.json")) as {
      scripts?: Record<string, string>;
    };
    const wrapper = read("apps/user-tests/scripts/user-vitest.mjs");

    for (const [name, script] of Object.entries(packageJson.scripts ?? {})) {
      expect(name.startsWith("pretest:user"), name).toBe(false);
      expect(script, name).not.toContain("pretest:user");
    }
    expect(wrapper).toContain("await buildConformance();");
    expect(wrapper).toContain('["run", "--cwd", repoRoot, "--filter", "@aexhq/conformance", "build"]');
    expect(wrapper).toContain("const vitestArgs = process.argv.slice(2);");
    expect(wrapper).toContain("buildUserVitestSpawnInvocation(vitestArgs)");
    expect(wrapper).toContain('["run", "vitest", "run", ...vitestArgs]');
    expect(wrapper).toContain("options: { shell: false }");
    expect(wrapper).not.toContain('shell: process.platform === "win32"');
  });

  it("treats blank live-test model vars as missing", () => {
    const liveDir = resolve(repoRoot, "apps/user-tests/test/live");
    const sources = readdirSync(liveDir)
      .filter((name) => name.endsWith(".ts"))
      .map((file) => ({ file, source: read(`apps/user-tests/test/live/${file}`) }));

    for (const { file, source } of sources) {
      expect(source, file).not.toContain('AEX_USER_TEST_DEEPSEEK_MODEL"] ??');
      expect(source, file).not.toContain("AEX_USER_TEST_DEEPSEEK_MODEL ??");
      expect(source, file).not.toContain('AEX_USER_TEST_ANTHROPIC_MODEL"] ??');
    }
    for (const { file, source } of sources.filter(({ source }) => source.includes("AEX_USER_TEST_DEEPSEEK_MODEL"))) {
      // Blank-safe handling lives either inline or in the shared gate-provider
      // fixture (test/_fixtures/provider.ts, itself pinned below).
      expect(
        source.includes('?.trim() || "deepseek-v4-flash"') || source.includes("gateModel()"),
        `${file}: must read the gate model blank-safe (inline ?.trim() default or fixture gateModel())`
      ).toBe(true);
    }
    const providerFixture = read("apps/user-tests/test/_fixtures/provider.ts");
    expect(providerFixture).toContain('?.trim() || "deepseek-v4-flash"');
    for (const { file, source } of sources.filter(({ source }) => source.includes("AEX_USER_TEST_ANTHROPIC_MODEL"))) {
      expect(source, file).toContain('?.trim() || "claude-haiku-4-5"');
    }
  });

  it("keeps the release gate on the DeepSeek gate provider only (no Anthropic billing dependency)", () => {
    // 2026-07-03: the shared BYOK ANTHROPIC_API_KEY ran out of credit and
    // killed multiple gating live shards. Gating tests exercise PLATFORM behavior,
    // so they all run on the funded DeepSeek gate provider (SSoT fixture);
    // Anthropic coverage lives in the non-gating providers suite.
    for (const path of [".github/workflows/live-user-tests.yml", ".github/workflows/release.yml"]) {
      const workflow = read(path);
      expect(workflow, path).not.toContain("ANTHROPIC_API_KEY");
      expect(workflow, path).not.toContain("AEX_USER_TEST_ANTHROPIC_MODEL");
      expect(workflow, path).toContain("DEEPSEEK_API_KEY: ${{ secrets.DEEPSEEK_API_KEY }}");
      expect(workflow, path).not.toContain("test:user:providers");
    }

    // The SSoT fixture pins the gate provider + model.
    const fixture = read("apps/user-tests/test/_fixtures/provider.ts");
    expect(fixture).toContain('export const GATE_PROVIDER = "deepseek" as const;');
    expect(fixture).toContain('export const GATE_KEY_ENV = "DEEPSEEK_API_KEY" as const;');
    expect(fixture).toContain('?.trim() || "deepseek-v4-flash"');

    // No gating live test may require an Anthropic key or model; Anthropic
    // BYOK coverage lives only under test/live/providers/ (non-gating).
    const liveDir = resolve(repoRoot, "apps/user-tests/test/live");
    // (live-skill-tool-staging embeds "ANTHROPIC_API_KEY=" as an illustrative
    // fake-credential string; only actual env READS are forbidden here.)
    for (const name of readdirSync(liveDir).filter((n) => n.endsWith(".ts"))) {
      const source = read(`apps/user-tests/test/live/${name}`);
      expect(source, name).not.toContain('requireEnv("ANTHROPIC_API_KEY")');
      expect(source, name).not.toContain("process.env.ANTHROPIC_API_KEY");
      expect(source, name).not.toContain('process.env["ANTHROPIC_API_KEY"]');
      expect(source, name).not.toContain("AEX_USER_TEST_ANTHROPIC_MODEL");
    }
    const anthropicProviderTest = read(
      "apps/user-tests/test/live/providers/live-sdk-anthropic-managed.test.ts"
    );
    expect(anthropicProviderTest).toContain('requireEnv("ANTHROPIC_API_KEY")');
    expect(anthropicProviderTest).toContain("function liveFailureDiagnostic(result: LiveResult)");
    expect(anthropicProviderTest).toContain("[REDACTED_ANTHROPIC_KEY]");
    expect(anthropicProviderTest).toContain("expect(result.sessionStatus, diagnostic).toBe(\"succeeded\")");

    // The non-gating on-demand workflow is the one place the Anthropic key flows.
    const onDemand = read(".github/workflows/live-on-demand-tests.yml");
    expect(onDemand).toContain("ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}");
    expect(onDemand).toContain("required provider keys hard-fail");
    expect(onDemand).not.toContain("self-skips when its key is unset");
  });

  it("fans out provider and heavy on-demand suites after one artifact preparation", () => {
    const workflow = read(".github/workflows/live-on-demand-tests.yml");

    expect(workflow).toContain("prepare-artifact:");
    expect(workflow).toContain("provider-tests:");
    expect(workflow).toContain("heavy-session:");
    expect(workflow.match(/needs: prepare-artifact/g)).toHaveLength(2);
    expect(workflow).toContain("name: live-on-demand-sdk-tarball");
    expect(workflow).toContain("run: bun run test:user:providers");
    expect(workflow).toContain("run: bun run test:user:heavy");
    expect(workflow).not.toContain("tool-fuzz-tests:");
    expect(workflow).not.toContain("test:user:tool-fuzz");
  });

  it("keeps comprehensive live-test skill names and assertions on one contract", () => {
    const source = read("apps/user-tests/test/live/live-sdk-comprehensive.test.ts");

    expect(source).toContain('function managedSkillName(role: "alpha" | "beta", provider: CaseSpec["provider"]): string');
    expect(source).toContain('name: ${JSON.stringify(managedSkillName("alpha", spec.provider))}');
    expect(source).toContain('name: ${JSON.stringify(managedSkillName("beta", spec.provider))}');
    expect(source).toContain('managedSkillName("alpha", "deepseek")');
    expect(source).toContain('managedSkillName("beta", "deepseek")');
    expect(source).toContain("produced no skill_loaded event");
    expect(source).toContain("dumpComprehensive()");
    expect(source).not.toContain('name: "compose-alpha-${spec.provider}"');
    expect(source).not.toContain('name: "compose-beta-${spec.provider}"');
    expect(source).not.toContain('["compose-alpha-managed-deepseek", "compose-beta-managed-deepseek"]');
  });

  it("keeps heavy live-test skill names and assertions on one contract", () => {
    const source = read("apps/user-tests/test/live/live-sdk-heavy-session.test.ts");
    const fixture = read("apps/user-tests/test/_fixtures/heavy-session-shape.ts");

    expect(source).toContain(
      'import { assertManagedShape, type CaseResult, type Probes } from "../_fixtures/heavy-session-shape.js";'
    );
    expect(source).toContain('import { withPreCreateTransportRetry } from "../_fixtures/pre-create-transport.js";');
    expect(source).toContain('function managedHeavySkillName(role: "alpha" | "beta" | "gamma", provider: CaseSpec["provider"]): string');
    expect(source).toContain('name: ${JSON.stringify(managedHeavySkillName("alpha", spec.provider))}');
    expect(source).toContain('name: ${JSON.stringify(managedHeavySkillName("beta", spec.provider))}');
    expect(source).toContain('name: ${JSON.stringify(managedHeavySkillName("gamma", spec.provider))}');
    expect(source).toContain('managedHeavySkillName("alpha", "deepseek")');
    expect(source).toContain('managedHeavySkillName("beta", "deepseek")');
    expect(source).toContain('managedHeavySkillName("gamma", "deepseek")');
    expect(source).toContain("assertManagedShape(result, [");
    expect(fixture).toContain("produced no skill_loaded event");
    expect(fixture).toContain('const SUCCESS_TERMINAL_KINDS = ["TURN_FINISHED", "aex.session.idle", "aex.session.succeeded"] as const;');
    expect(fixture).toContain('if (result.terminalKind === "TURN_FINISHED")');
    expect(fixture).toContain("legacy TURN_FINISHED stream did not include TURN_STARTED");
    expect(source).toContain("return isSessionIdle(e) ? customName(e) : e.type;");
    expect(fixture).toContain('"aex.session.succeeded"');
    expect(source).toContain("const maxChannelProbeRetries = 2;");
    expect(source).toContain("withPreCreateTransportRetry(`heavy-session ${spec.provider}`");
    expect(source).toContain("recordChannelProbeSources");
    expect(source).toContain('"toolCallStart"');
    expect(source).toContain('"toolCallResult"');
    expect(source).toContain("channelProbeSources");
    expect(source).toContain("channelProbeMisses");
    expect(source).toContain("succeeded but missed channel probes");
    expect(source.indexOf("const SESSION_TERMINAL_NAMES = new Set([")).toBeLessThan(
      source.indexOf("const listedEvents = await session.events().list();")
    );
    expect(source.indexOf("function hasTerminalEvent(list)")).toBeLessThan(
      source.indexOf("const listedEvents = await session.events().list();")
    );
    expect(source).not.toContain('"TURN_FINISHED",\n  "TEXT_MESSAGE_CONTENT"');
    expect(source).not.toContain('name: "heavy-alpha-${spec.provider}"');
    expect(source).not.toContain('name: "heavy-beta-${spec.provider}"');
    expect(source).not.toContain('name: "heavy-gamma-${spec.provider}"');
    expect(source).not.toContain('["heavy-alpha-managed", "heavy-beta-managed", "heavy-gamma-managed"]');
  });

  it("keeps MCP egress fail-closed assertions aware of submit-time API rejections", () => {
    const source = read("apps/user-tests/test/live/edge-mcp-egress.user.test.ts");

    expect(source).toContain("function rejectionText(s: SubmissionCase): string");
    expect(source).toContain('s.reason ?? ""');
    expect(source).toContain('s.threw ?? ""');
    expect(source).toContain("rejectionText(s)");
    expect(source).not.toContain("`${s.reason}`");
  });

  it("keeps live API fuzz region-routing probes from following redirects", () => {
    const source = read("apps/user-tests/test/live/live-api-fuzz.test.ts");

    expect(source).toContain("redirect?: RequestRedirect");
    expect(source).toContain('redirect: opts.redirect ?? "follow"');
    expect(source).toContain('token, redirect: "manual"');
  });

  it("keeps edge file transfer probes live-plane realistic and diagnostic", () => {
    const source = read("apps/user-tests/test/live/edge-files.user.test.ts");

    expect(source).toContain("const LIVE_FILE_TRANSFER_TIMEOUT_MS = 20_000;");
    expect(source).not.toContain("timeoutMs: 5000");
    expect(source).toContain("HTTP_DEBUG_LINES");
    expect(source).toContain("redactedUrlForDebug");
    expect(source).toContain("fetch: tracedFetch");
    expect(source).toContain('debug: (line) => pushHttpDebug("[sdk] " + line)');
    expect(source).toContain("httpDebug: debugTail()");
    expect(source).toContain("const ctxPayload =");
    expect(source).toContain("httpDebug: r.httpDebug");
  });

  it("keeps edge file success probes resilient to transient idempotent GET failures", () => {
    const source = read("apps/user-tests/test/live/edge-files.user.test.ts");

    expect(source).toContain("async function probeIdempotent(label, fn)");
    expect(source).toContain("transient failure");
    expect(source).toContain("isTransientProbeError(result.error)");
    expect(source).toContain("function zipProbeNoTransientManifestErrors(bytes)");
    expect(source).toContain("zip manifest recorded transient per-artifact download errors");
    for (const field of ["causeCode", "attempts", "elapsedMs", "method", "host", "path"]) {
      expect(source).toContain(field);
    }
    for (const label of [
      "read_suffix",
      "read_exact",
      "read_timeout_option",
      "download_selector",
      "download_files_zip",
      "download_all_zip",
      "download_metadata_zip"
    ]) {
      expect(source).toContain(`probeIdempotent("${label}"`);
    }
    expect(source).toContain(
      'probeIdempotent("download_files_zip", async () => zipProbeNoTransientManifestErrors(await outs.download(undefined)))'
    );
    expect(source).toContain(
      'probeIdempotent("download_files_zip_timeout_option", async () => zipProbeNoTransientManifestErrors(await outs.download(undefined, { timeoutMs: LIVE_FILE_TRANSFER_TIMEOUT_MS })))'
    );
    expect(source).toContain(
      'probeIdempotent("download_all_zip", async () => zipProbeNoTransientManifestErrors(await session.download()))'
    );
    for (const label of ["read_missing_path", "download_missing_id", "link_nomatch"]) {
      expect(source).toContain(`probe("${label}"`);
      expect(source).not.toContain(`probeIdempotent("${label}"`);
    }
  });

  it("keeps edge skills/tools live cases resilient only to pre-create transport failures", () => {
    const source = read("apps/user-tests/test/live/edge-skills-tools.user.test.ts");

    expect(source).toContain('import { isPreCreateTransportFailure } from "../_fixtures/pre-create-transport.js";');
    expect(source).toContain("async function runScenarioWithPreCreateRetry(");
    expect(source).toContain("isPreCreateTransportFailure(result.observation)");
    expect(source).toContain("[edge-skills-tools]");
    expect(source).toContain("No sessionId means the submit never created a debuggable live session artifact.");
    expect(source).toContain("runScenarioWithPreCreateRetry(install, \"edge-throw.mjs\", body)");
    expect(source).toContain("runScenarioWithPreCreateRetry(install, \"edge-dup-name.mjs\", body)");
  });

  it("keeps event-stream child diagnostics classifiable before JSON emit", () => {
    const source = read("apps/user-tests/test/live/edge-event-stream.user.test.ts");

    expect(source).toContain('import { isPreCreateTransportMessage } from "../_fixtures/pre-create-transport.js";');
    expect(source).toContain('await emitChildFailure("top-level", error);');
    expect(source).toContain("childFailure: true");
    expect(source).toContain('sessionId: __childSessionId');
    expect(source).toContain("isPreCreateChildFailure(error)");
    expect(source).toContain("childFailureDiagnostic(scriptName, parsed)");
    expect(source).toContain("did not print JSON: ${JSON.stringify({");
    expect(source).toContain("redactChildText(child.stdout)");
  });

  it("keeps event-stream settle consistency aligned with session-park terminals", () => {
    const source = read("apps/user-tests/test/live/edge-event-stream.user.test.ts");

    expect(source).toContain("settleConsistent waits for the post-mirror aex.session.settled barrier");
    expect(source).toContain(
      'r.settleCustomNames.some((name) => name.startsWith("aex.session.")) || r.settleHasBarrier'
    );
    expect(source).toMatch(/expect\(\s*r\.settleEndedNaturally[\s\S]*?\)\.toBe\(true\);/);
  });

  it("waits accepted corrupted-skill sessions to terminal before asserting failure shape", () => {
    const source = read("apps/user-tests/test/live/live-sdk-files-and-failures.test.ts");
    const start = source.indexOf("function buildCorruptedSkillScript");
    const end = source.indexOf("function buildIncompatibleRuntimeScript");
    const corruptedSkillScript = source.slice(start, end);

    expect(corruptedSkillScript).toContain("const accepted = JSON.parse(submitBody);");
    expect(corruptedSkillScript).toContain('"/api/sessions/" + encodeURIComponent(sessionId)');
    expect(corruptedSkillScript).toContain("terminalStatuses.has(sessionStatus)");
    expect(corruptedSkillScript).toContain('"/events?limit=1000"');
    expect(corruptedSkillScript).toContain('event.type === "TURN_ERROR"');
    expect(corruptedSkillScript).toContain('terminalData = terminal && terminal.data');
  });

  it("keeps lineage observability scratch file inside the live-test sandbox", () => {
    const source = read("apps/user-tests/test/live/edge-lineage-observability.user.test.ts");

    expect(source).toContain('writeFileSync(join(install.installDir, "lineage-wave1-out.json")');
    expect(source).not.toContain("C:/Users/");
    expect(source).not.toContain("/tmp/claude/");
    expect(source).not.toContain("scratchpad/lineage-wave1-out.json");
  });
});
