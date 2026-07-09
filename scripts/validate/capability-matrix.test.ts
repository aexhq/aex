import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  PROVIDERS,
  type ProviderName
} from "../../packages/contracts/src/submission.js";
import {
  PROVIDER_PUBLIC_SUPPORT,
  type SupportPointer
} from "../../packages/contracts/src/provider-support.js";
import {
  CAPABILITY_MATRIX_PATH,
  buildCapabilityMatrixRows,
  checkCapabilityMatrix,
  renderProviderRuntimeCapabilityMarkdown
} from "../generate-capability-matrix.js";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

function liveEvidencePointers(provider: ProviderName): readonly SupportPointer[] {
  return PROVIDER_PUBLIC_SUPPORT[provider].evidence.filter((pointer) =>
    pointer.href.includes("apps/user-tests/test/live/")
  );
}

function evidencePath(pointer: SupportPointer): string {
  return resolve(repoRoot, "packages", "sdk", "docs", pointer.href);
}

describe("provider/runtime capability matrix generation", () => {
  it("renders one deterministic managed-runtime row for every provider", () => {
    const rendered = renderProviderRuntimeCapabilityMarkdown();
    expect(buildCapabilityMatrixRows().map((row) => row.provider)).toEqual([
      "anthropic",
      "deepseek",
      "openai",
      "gemini",
      "mistral",
      "openrouter",
      "doubao",
      "doubao-cn"
    ]);
    expect(rendered).toContain(
      "| [Anthropic](#anthropic) | `anthropic` | `claude-haiku-4-5`, `claude-3-5-haiku-latest`, `claude-3-5-sonnet-latest`, `claude-sonnet-4-6` | [Secrets](secrets.md); [Events](events.md) |"
    );
    expect(rendered).toContain(
      "| `anthropic` | submission parser + managed execution | [Installed-SDK Anthropic live user test](../../../apps/user-tests/test/live/providers/live-sdk-anthropic-managed.test.ts) |"
    );
    expect(rendered).toContain(
      "| `openai` | submission parser + managed execution |"
    );
    expect(rendered).not.toContain("live-unverified");
    expect(rendered).not.toContain("Native feature parity");
  });

  it("keeps public support facts complete and anchor-safe", () => {
    const anchors = new Set<string>();
    for (const provider of PROVIDERS) {
      const support = PROVIDER_PUBLIC_SUPPORT[provider];
      expect(support.docsAnchor).toMatch(/^[a-z0-9-]+$/);
      expect(anchors.has(support.docsAnchor)).toBe(false);
      anchors.add(support.docsAnchor);
      expect(support.docs.length).toBeGreaterThan(0);
      expect(support.evidence.length).toBeGreaterThan(0);
    }
  });

  it("keeps the public registry and generated rows supported-only", () => {
    for (const provider of PROVIDERS) {
      expect(PROVIDER_PUBLIC_SUPPORT[provider]).not.toHaveProperty("status");
    }

    for (const row of buildCapabilityMatrixRows()) {
      expect(row).not.toHaveProperty("publicStatus");
      expect(row.managedExecution).not.toHaveProperty("status");
      expect(row.managedExecution).not.toHaveProperty("ownership");
    }
  });

  it("keeps every managed runtime cell documented with anchor, enforcement, and evidence", () => {
    for (const row of buildCapabilityMatrixRows()) {
      const cell = row.managedExecution;
      expect(cell.docsAnchor).toBe(row.docsAnchor);
      expect(cell.enforcement.length).toBeGreaterThan(0);
      expect(cell.evidence.length).toBeGreaterThan(0);
    }
  });

  it("lists every public provider as a supported managed-runtime row", () => {
    for (const provider of PROVIDERS) {
      const row = buildCapabilityMatrixRows().find((candidate) => candidate.provider === provider);
      expect(row).toBeDefined();
      expect(row?.managedExecution.enforcement).toBe("submission parser + managed execution");
    }
  });

  it("keeps live evidence pointers provider-specific when present", () => {
    const providers = PROVIDERS.map((provider) => ({
      provider,
      support: PROVIDER_PUBLIC_SUPPORT[provider],
      pointers: liveEvidencePointers(provider)
    }));

    for (const { provider, pointers } of providers) {
      for (const pointer of pointers) {
        const path = evidencePath(pointer);
        expect(existsSync(path), `${provider} evidence pointer must resolve: ${pointer.href}`).toBe(true);
        const source = readFileSync(path, "utf8");
        expect(source, `${provider} evidence file must submit that provider`).toContain(`provider: "${provider}"`);
      }
    }
  });

  it("has no public runtime selector fields in generated rows", () => {
    for (const row of buildCapabilityMatrixRows()) {
      expect(row).not.toHaveProperty("autoRoute");
      expect(row).not.toHaveProperty("managedRuntime");
      expect(row.managedExecution.enforcement).toBe("submission parser + managed execution");
    }
  });

  it("keeps deploy and economic caveats out of the generated matrix", () => {
    const rendered = renderProviderRuntimeCapabilityMarkdown();
    expect(rendered).not.toMatch(/\b(billing|bills|cost|costs|margin|margins|topology)\b/i);
  });

  it("fails the check when committed output is stale", () => {
    const expected = renderProviderRuntimeCapabilityMarkdown();
    expect(checkCapabilityMatrix(expected, expected)).toEqual({ ok: true });
    expect(checkCapabilityMatrix("stale\n", expected)).toEqual({
      ok: false,
      message: `${CAPABILITY_MATRIX_PATH} is stale; run bun run capabilities:generate`
    });
  });

  it("keeps the committed markdown in sync", () => {
    const current = readFileSync(resolve(repoRoot, CAPABILITY_MATRIX_PATH), "utf8");
    expect(checkCapabilityMatrix(current, renderProviderRuntimeCapabilityMarkdown())).toEqual({ ok: true });
  });
});
