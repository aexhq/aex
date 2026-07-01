import { readFileSync } from "node:fs";
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

    expect(workflow).toContain("name: Live user tests shard ${{ matrix.shard }}/4");
    expect(workflow).toContain("shard: [1, 2, 3, 4]");
    expect(workflow).toContain("bun run test:user -- --shard=${{ matrix.shard }}/4 2>&1 | tee \"$RAW_LOG\"");
    expect(workflow).toContain("AEX_USER_TEST_MAX_WORKERS: 2");
    expect(workflow).toContain("Redact live user test log");
    expect(workflow).toContain("Upload redacted live user test log");
    expect(workflow).toContain("path: .suite-diagnostics/redacted");
    expect(workflow).toContain("retention-days: 14");
    expect(workflow).toContain('["AEX_API_TOKEN", "ANTHROPIC_API_KEY", "DEEPSEEK_API_KEY"]');
    expect(workflow).toContain("text.split(value).join(`[REDACTED:${name}]`)");
    expect(workflow).not.toContain("path: .suite-diagnostics/raw");
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
    expect(source).not.toContain('name: "heavy-alpha-${spec.provider}"');
    expect(source).not.toContain('name: "heavy-beta-${spec.provider}"');
    expect(source).not.toContain('name: "heavy-gamma-${spec.provider}"');
    expect(source).not.toContain('["heavy-alpha-managed", "heavy-beta-managed", "heavy-gamma-managed"]');
  });
});
