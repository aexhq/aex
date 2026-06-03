import { readFileSync } from "node:fs";
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
import {
  PROVIDER_PUBLIC_SUPPORT,
  PROVIDER_SUPPORT_STATUSES,
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

function dispatcherProbe(provider: RunProvider): PlatformRunSubmissionRequest {
  return {
    workspaceId: "capability-matrix-test",
    idempotencyKey: `capability-matrix-test-${provider}`,
    credentialMode: "byok",
    provider,
    submission: {
      model: "capability-matrix-model",
      prompt: ["capability matrix runtime probe"],
      skills: [],
      agentsMd: [],
      files: [],
      mcpServers: []
    },
    secrets: {
      [provider]: { apiKey: `sk-${provider}-capability-test` }
    } as PlatformRunSubmissionRequest["secrets"]
  };
}

function expectedRuntimeStatus(provider: RunProvider, runtime: RuntimeKind): RuntimeSupportStatus {
  if (!checkRuntimeSupported(provider, runtime).ok) return "rejected";
  return PROVIDER_PUBLIC_SUPPORT[provider].status === "supported" ? "supported" : "live-unverified";
}

describe("provider/runtime capability matrix generation", () => {
  it("renders one deterministic managed-runtime row for every provider", () => {
    const rendered = renderProviderRuntimeCapabilityMarkdown();
    expect(buildCapabilityMatrixRows().map((row) => row.provider)).toEqual([
      "anthropic",
      "deepseek",
      "openai",
      "gemini",
      "mistral"
    ]);
    expect(rendered).toContain(
      "| [Anthropic](#anthropic) | `anthropic` | supported | [Credentials](credentials.md); [Events](events.md) |"
    );
    expect(rendered).toContain(
      "| `anthropic` | yes | `managed` | [supported](#anthropic) |"
    );
    expect(rendered).toContain(
      "| `openai` | `managed` | live-unverified | live-unverified | submission parser + managed dispatch |"
    );
    expect(rendered).not.toContain("Native feature parity");
    expect(rendered).not.toContain("anthropic-managed");
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

  it("does not promote accepted-but-not-live-proven managed providers to supported", () => {
    for (const provider of ["openai", "gemini", "mistral"] as const) {
      const row = buildCapabilityMatrixRows().find((candidate) => candidate.provider === provider);
      expect(row?.managedRuntime.status).toBe("live-unverified");
      expect(row?.publicStatus).toBe("live-unverified" satisfies ProviderSupportStatus);
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
    expect(rendered).not.toMatch(/\b(Fly|billing|bills|cost|costs|margin|margins|topology)\b/i);
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
