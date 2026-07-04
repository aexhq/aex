import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

function read(path: string): string {
  return readFileSync(resolve(repoRoot, path), "utf8");
}

describe("live user-test release gate", () => {
  it("uploads only a redacted live-test log artifact", () => {
    const workflow = read(".github/workflows/live-user-tests.yml");

    expect(workflow).toContain("name: Live user tests shard ${{ matrix.shard }}/11");
    expect(workflow).toContain("shard: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]");
    expect(workflow).toContain('bun run test:user:files -- $FILES 2>&1 | tee "$RAW_LOG"');
    expect(workflow).toContain("AEX_USER_TEST_MAX_WORKERS: 1");
    expect(workflow).toContain("Redact live user test log");
    expect(workflow).toContain("Upload redacted live user test log");
    expect(workflow).toContain("path: .suite-diagnostics/redacted");
    expect(workflow).toContain("retention-days: 14");
    expect(workflow).toContain('["AEX_API_TOKEN", "DEEPSEEK_API_KEY"]');
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

  it("shards live user tests by recorded duration, not file count", () => {
    // vitest --shard splits by file count (per-file live durations vary
    // ~1s..6.5min, giving 1m42s..12m7s shard walls, and shard 12/12 once
    // collected ZERO tests). Both release-gate workflows must instead ask
    // scripts/shard-files.mjs for an explicit, duration-balanced,
    // guaranteed-non-empty file list BEFORE invoking vitest.
    for (const path of [".github/workflows/live-user-tests.yml", ".github/workflows/release.yml"]) {
      const workflow = read(path);

      expect(workflow, path).toContain(
        'FILES="$(node apps/user-tests/scripts/shard-files.mjs --shard ${{ matrix.shard }}/11)"'
      );
      expect(workflow, path).toContain('bun run test:user:files -- $FILES 2>&1 | tee "$RAW_LOG"');
      expect(workflow, path).not.toContain("--shard=");
      const shardCall = workflow.indexOf("scripts/shard-files.mjs --shard");
      const vitestCall = workflow.indexOf("bun run test:user:files -- $FILES");
      expect(shardCall, path).toBeGreaterThan(-1);
      expect(vitestCall, path).toBeGreaterThan(shardCall);
    }

    // The explicit-file lane must NOT carry the catch-all `test` positional
    // filter — it would match every file and defeat the shard list.
    const packageJson = JSON.parse(read("apps/user-tests/package.json")) as {
      scripts?: Record<string, string>;
    };
    expect(packageJson.scripts?.["test:user:files"]).toBe(
      "bun scripts/run-user-vitest.mjs --config vitest.config.ts"
    );
    expect(packageJson.scripts?.["test:user:admission-gates"]).toBe(
      "bun scripts/run-user-vitest.mjs --config vitest.admission-gates.config.ts"
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
    const wrapper = read("apps/user-tests/scripts/run-user-vitest.mjs");

    for (const [name, script] of Object.entries(packageJson.scripts ?? {})) {
      expect(name.startsWith("pretest:user"), name).toBe(false);
      expect(script, name).not.toContain("pretest:user");
    }
    expect(wrapper).toContain("await buildConformance();");
    expect(wrapper).toContain('["run", "--cwd", repoRoot, "--filter", "@aexhq/conformance", "build"]');
    expect(wrapper).toContain("const vitestArgs = process.argv.slice(2);");
    expect(wrapper).toContain('["run", "vitest", "run", ...vitestArgs]');
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
    // killed 6/11 gating live shards. Gating tests exercise PLATFORM behavior,
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

    // The non-gating on-demand workflow is the one place the Anthropic key flows.
    const onDemand = read(".github/workflows/live-on-demand-tests.yml");
    expect(onDemand).toContain("ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}");
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

    expect(source).toContain('function managedHeavySkillName(role: "alpha" | "beta" | "gamma", provider: CaseSpec["provider"]): string');
    expect(source).toContain('name: ${JSON.stringify(managedHeavySkillName("alpha", spec.provider))}');
    expect(source).toContain('name: ${JSON.stringify(managedHeavySkillName("beta", spec.provider))}');
    expect(source).toContain('name: ${JSON.stringify(managedHeavySkillName("gamma", spec.provider))}');
    expect(source).toContain('managedHeavySkillName("alpha", "deepseek")');
    expect(source).toContain('managedHeavySkillName("beta", "deepseek")');
    expect(source).toContain('managedHeavySkillName("gamma", "deepseek")');
    expect(source).toContain("produced no skill_loaded event");
    expect(source).toContain('return isSessionIdle(e) ? "aex.session.idle" : e.type;');
    expect(source).toContain('expect(["RUN_FINISHED", "aex.session.idle"]).toContain(result.terminalKind);');
    expect(source).not.toContain('"RUN_FINISHED",\n  "TEXT_MESSAGE_CONTENT"');
    expect(source).not.toContain('name: "heavy-alpha-${spec.provider}"');
    expect(source).not.toContain('name: "heavy-beta-${spec.provider}"');
    expect(source).not.toContain('name: "heavy-gamma-${spec.provider}"');
    expect(source).not.toContain('["heavy-alpha-managed", "heavy-beta-managed", "heavy-gamma-managed"]');
  });

  it("keeps live API fuzz region-routing probes from following redirects", () => {
    const source = read("apps/user-tests/test/live/live-api-fuzz.test.ts");

    expect(source).toContain("redirect?: RequestRedirect");
    expect(source).toContain('redirect: opts.redirect ?? "follow"');
    expect(source).toContain('token, redirect: "manual"');
  });

  it("keeps event-stream settle consistency aligned with session-park terminals", () => {
    const source = read("apps/user-tests/test/live/edge-event-stream.user.test.ts");

    expect(source).toContain("stream ends at the session-park terminal");
    expect(source).toContain("expect(r.settleHasBarrier).toBe(false);");
    expect(source).toContain("expect(r.settleEndedNaturally).toBe(true);");
  });

  it("keeps lineage observability scratch output inside the live-test sandbox", () => {
    const source = read("apps/user-tests/test/live/edge-lineage-observability.user.test.ts");

    expect(source).toContain('writeFileSync(join(install.installDir, "lineage-wave1-out.json")');
    expect(source).not.toContain("C:/Users/");
    expect(source).not.toContain("/tmp/claude/");
    expect(source).not.toContain("scratchpad/lineage-wave1-out.json");
  });
});
