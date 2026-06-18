import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  checkRuntimeSupported,
  RUN_PROVIDERS,
  RUNTIME_KINDS,
  selectRuntime,
  type PlatformRunSubmissionRequest,
  type RunProvider,
  type RuntimeKind
} from "../../packages/contracts/src/submission.js";
import { RUN_MODELS_BY_PROVIDER } from "../../packages/contracts/src/models.js";
import {
  PROVIDER_PUBLIC_SUPPORT,
  PROVIDER_SUPPORT_STATUSES,
  type SupportPointer,
  type ProviderSupportStatus
} from "../../packages/contracts/src/provider-support.js";
import {
  CAPABILITY_MATRIX_PATH,
  buildCapabilityMatrixRows,
  checkCapabilityMatrix,
  type RuntimeSupportStatus,
  renderProviderRuntimeCapabilityMarkdown
} from "../generate-capability-matrix.js";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

function liveEvidencePointers(provider: RunProvider): readonly SupportPointer[] {
  return PROVIDER_PUBLIC_SUPPORT[provider].evidence.filter((pointer) =>
    pointer.href.includes("apps/user-tests/test/live/")
  );
}

function evidencePath(pointer: SupportPointer): string {
  return resolve(repoRoot, "packages", "sdk", "docs", pointer.href);
}

function dispatcherProbe(provider: RunProvider): PlatformRunSubmissionRequest {
  return {
    workspaceId: "capability-matrix-test",
    idempotencyKey: `capability-matrix-test-${provider}`,
    credentialMode: "byok",
    provider,
    submission: {
      model: RUN_MODELS_BY_PROVIDER[provider][0]!,
      prompt: ["capability matrix runtime probe"],
      skills: [],
      agentsMd: [],
      files: [],
      mcpServers: []
    },
    secrets: {
      apiKey: `sk-${provider}-capability-test`
    }
  };
}

function expectedRuntimeStatus(provider: RunProvider, runtime: RuntimeKind): RuntimeSupportStatus {
  if (!checkRuntimeSupported(provider, runtime).ok) return "rejected";
  return PROVIDER_PUBLIC_SUPPORT[provider].status === "supported" ? "supported" : "rejected";
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
      "| `anthropic` | yes | `managed` | [managed](#anthropic) |"
    );
    expect(rendered).toContain(
      "| `openai` | `managed` | submission parser + managed dispatch |"
    );
    expect(rendered).not.toContain("live-unverified");
    expect(rendered).not.toContain("Native feature parity");
  });

  it("keeps public support facts complete and anchor-safe", () => {
    const anchors = new Set<string>();
    for (const provider of RUN_PROVIDERS) {
      const support = PROVIDER_PUBLIC_SUPPORT[provider];
      expect(PROVIDER_SUPPORT_STATUSES).toContain(support.status);
      expect(support.docsAnchor).toMatch(/^[a-z0-9-]+$/);
      expect(anchors.has(support.docsAnchor)).toBe(false);
      anchors.add(support.docsAnchor);
      expect(support.docs.length).toBeGreaterThan(0);
      expect(support.evidence.length).toBeGreaterThan(0);
    }
  });

  it("keeps every managed runtime cell documented with status, anchor, enforcement, and evidence", () => {
    for (const row of buildCapabilityMatrixRows()) {
      const cell = row.managedRuntime;
      expect(PROVIDER_SUPPORT_STATUSES).toContain(cell.status);
      expect(PROVIDER_SUPPORT_STATUSES).toContain(cell.ownership);
      expect(cell.docsAnchor).toBe(row.docsAnchor);
      expect(cell.enforcement.length).toBeGreaterThan(0);
      expect(cell.evidence.length).toBeGreaterThan(0);
    }
  });

  it("lists every public provider as supported", () => {
    for (const provider of RUN_PROVIDERS) {
      const row = buildCapabilityMatrixRows().find((candidate) => candidate.provider === provider);
      expect(row?.managedRuntime.status).toBe("supported");
      expect(row?.publicStatus).toBe("supported" satisfies ProviderSupportStatus);
    }
  });

  it("keeps live evidence pointers provider-specific when present", () => {
    const providers = RUN_PROVIDERS.map((provider) => ({
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

  it("derives routing cells from checkRuntimeSupported and selectRuntime", () => {
    expect([...RUNTIME_KINDS]).toEqual(["managed"]);
    for (const row of buildCapabilityMatrixRows()) {
      expect(row.managedRuntime.status).toBe(expectedRuntimeStatus(row.provider, "managed"));
      expect(row.autoRoute).toBe(selectRuntime(dispatcherProbe(row.provider)));
      expect(row.autoRoute).toBe("managed");
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
      message: `${CAPABILITY_MATRIX_PATH} is stale; run pnpm capabilities:generate`
    });
  });

  it("keeps the committed markdown in sync", () => {
    const current = readFileSync(resolve(repoRoot, CAPABILITY_MATRIX_PATH), "utf8");
    expect(checkCapabilityMatrix(current, renderProviderRuntimeCapabilityMarkdown())).toEqual({ ok: true });
  });
});
