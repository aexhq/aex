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

    expect(workflow).toContain("bun run test:user 2>&1 | tee .suite-diagnostics/raw/live-user-tests.log");
    expect(workflow).toContain("Redact live user test log");
    expect(workflow).toContain("Upload redacted live user test log");
    expect(workflow).toContain("path: .suite-diagnostics/redacted");
    expect(workflow).toContain("retention-days: 14");
    expect(workflow).toContain('["AEX_API_TOKEN", "ANTHROPIC_API_KEY", "DEEPSEEK_API_KEY"]');
    expect(workflow).toContain("text.split(value).join(`[REDACTED:${name}]`)");
    expect(workflow).not.toContain("path: .suite-diagnostics/raw");
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
