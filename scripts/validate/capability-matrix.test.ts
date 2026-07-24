import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";
import {
  CAPABILITY_MATRIX_PATH,
  checkCapabilityMatrix,
  renderModelCapabilityMarkdown
} from "../generate-capability-matrix.js";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

describe("managed-gateway model-access capability doc", () => {
  it("renders a provider-free, gateway-slug model-access doc", () => {
    const rendered = renderModelCapabilityMarkdown();
    expect(rendered).toContain("# Model access");
    expect(rendered).toContain("managed Vercel AI Gateway");
    expect(rendered).toContain("parseModelSlug");
    expect(rendered).toContain("anthropic/claude-haiku-4-5");
    // No provider AXIS survives: no provider selector field, no closed catalog
    // table, no dropped providers, no BYOK. (The word "provider" may still appear
    // in explanatory prose — "you never supply a provider API key".)
    expect(rendered).not.toContain("openrouter");
    expect(rendered).not.toContain("doubao");
    expect(rendered).not.toContain("| Provider |");
    expect(rendered).not.toContain("BYOK");
    expect(rendered).not.toContain("apiKeys");
    expect(rendered).not.toMatch(/provider:\s*Providers\./);
  });

  it("states streaming is honored for all models (no capability gate)", () => {
    const rendered = renderModelCapabilityMarkdown();
    expect(rendered).toMatch(/honored for ALL models/);
  });

  it("keeps deploy and economic caveats out of the generated doc", () => {
    const rendered = renderModelCapabilityMarkdown();
    expect(rendered).not.toMatch(/\b(billing|bills|cost|costs|margin|margins|topology)\b/i);
  });

  it("fails the check when committed output is stale", () => {
    const expected = renderModelCapabilityMarkdown();
    expect(checkCapabilityMatrix(expected, expected)).toEqual({ ok: true });
    expect(checkCapabilityMatrix("stale\n", expected)).toEqual({
      ok: false,
      message: `${CAPABILITY_MATRIX_PATH} is stale; run bun run capabilities:generate`
    });
  });

  it("keeps the committed markdown in sync", () => {
    const current = readFileSync(resolve(repoRoot, CAPABILITY_MATRIX_PATH), "utf8");
    expect(checkCapabilityMatrix(current, renderModelCapabilityMarkdown())).toEqual({ ok: true });
  });
});
