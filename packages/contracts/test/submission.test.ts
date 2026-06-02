import { describe, expect, it } from "vitest";
import {
  BLOCKED_MANAGED_KEY_FEATURE_POLICY_V1,
  DEFAULT_CREDENTIAL_MODE,
  ManagedKeyUnavailableError,
  RUNTIME_KINDS,
  RuntimeValidationError,
  collectManagedUnsupportedFeatures,
  DEFAULT_RUN_PROVIDER,
  RUN_PROVIDERS,
  parseRunSubmissionRequest,
  selectRuntime,
  type ManagedKeyPolicyV1,
  type PlatformRunSubmissionRequest,
  type RunProvider,
  type RuntimeKind
} from "../src/index.js";

function assetRef(name: string, seed = 1) {
  const hex = String(seed).padStart(64, "0");
  return { kind: "asset" as const, assetId: `asset_${hex}`, name };
}

function baseRequest(overrides: Partial<{ provider: RunProvider; runtime: RuntimeKind }> = {}) {
  const provider = overrides.provider ?? "anthropic";
  const secrets: Record<string, { apiKey: string }> = {};
  secrets[provider] = { apiKey: `sk-${provider}-test` };
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "idem-1",
    provider,
    ...(overrides.runtime !== undefined ? { runtime: overrides.runtime } : {}),
    submission: {
      model: "model-x",
      prompt: ["hello"],
      skills: [],
      agentsMd: [],
      files: [],
      mcpServers: []
    },
    secrets
  };
}

const injectedManagedKeyPolicy = {
  schemaVersion: 1,
  credentialMode: "managed",
  launchStage: "pilot",
  serviceAvailable: true,
  billingRequired: true,
  providers: ["anthropic"],
  runtimes: ["managed"],
  models: ["model-x"],
  features: {
    ...BLOCKED_MANAGED_KEY_FEATURE_POLICY_V1
  }
} satisfies ManagedKeyPolicyV1;

describe("submission parser - providers and secrets", () => {
  it("accepts every provider in RUN_PROVIDERS", () => {
    for (const provider of RUN_PROVIDERS) {
      const parsed = parseRunSubmissionRequest(baseRequest({ provider }));
      expect(parsed.provider).toBe(provider);
    }
  });

  it("rejects unknown providers with a helpful enumeration", () => {
    expect(() =>
      parseRunSubmissionRequest({ ...baseRequest(), provider: "bogus" })
    ).toThrow(/provider must be one of: anthropic, deepseek, openai, gemini, mistral/);
  });

  it("defaults to anthropic when provider is omitted", () => {
    const { provider: _drop, ...rest } = baseRequest();
    const parsed = parseRunSubmissionRequest({
      ...rest,
      secrets: { anthropic: { apiKey: "sk-ant-default" } }
    });
    expect(DEFAULT_RUN_PROVIDER).toBe("anthropic");
    expect(parsed.provider).toBe("anthropic");
  });

  it("normalizes omitted credentialMode to BYOK in the parsed snapshot", () => {
    const parsed = parseRunSubmissionRequest(baseRequest());
    expect(DEFAULT_CREDENTIAL_MODE).toBe("byok");
    expect(parsed.credentialMode).toBe("byok");
    expect(JSON.parse(JSON.stringify(parsed))).toHaveProperty("credentialMode", "byok");
  });

  it("rejects managed-key credential mode until the service implementation exists", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseRequest(),
        credentialMode: "managed"
      })
    ).toThrow(ManagedKeyUnavailableError);
  });

  it("accepts managed-key mode only through an injected policy seam and does not require provider secrets", () => {
    const { secrets: _providerSecret, ...req } = baseRequest({
      provider: "anthropic",
      runtime: "managed"
    });
    const parsed = parseRunSubmissionRequest(
      {
        ...req,
        credentialMode: "managed",
        secrets: {}
      },
      { managedKeyPolicy: injectedManagedKeyPolicy }
    );

    expect(parsed.credentialMode).toBe("managed");
    expect(parsed.secrets).toEqual({});
  });

  it("rejects caller-supplied provider secrets in managed-key mode", () => {
    expect(() =>
      parseRunSubmissionRequest(
        {
          ...baseRequest({ provider: "anthropic", runtime: "managed" }),
          credentialMode: "managed"
        },
        { managedKeyPolicy: injectedManagedKeyPolicy }
      )
    ).toThrow(/secrets\.anthropic is not allowed when credentialMode is managed/);
  });

  it.each(["deepseek", "openai", "gemini", "mistral"] as const)(
    "requires secrets.%s.apiKey when provider is %s",
    (provider) => {
      const req = baseRequest({ provider });
      expect(() =>
        parseRunSubmissionRequest({ ...req, secrets: {} })
      ).toThrow(new RegExp(`secrets\\.${provider}\\.apiKey is required when provider is ${provider}`));
    }
  );

  it("rejects cross-provider secret leakage", () => {
    const req = baseRequest({ provider: "anthropic" });
    expect(() =>
      parseRunSubmissionRequest({
        ...req,
        secrets: { ...req.secrets, openai: { apiKey: "sk-openai-x" } }
      })
    ).toThrow(/secrets\.openai is not allowed when provider is anthropic/);
  });
});

describe("submission parser - managed-only runtime field", () => {
  it("accepts explicit runtime: 'managed' for every provider", () => {
    for (const provider of RUN_PROVIDERS) {
      const parsed = parseRunSubmissionRequest(baseRequest({ provider, runtime: "managed" }));
      expect(parsed.runtime).toBe("managed");
      expect(selectRuntime(parsed)).toBe("managed");
    }
  });

  it("omits runtime from the parsed request when the wire field is absent, then selects managed", () => {
    const parsed = parseRunSubmissionRequest(baseRequest());
    expect(parsed.runtime).toBeUndefined();
    expect(selectRuntime(parsed)).toBe("managed");
  });

  it("rejects runtime: 'native' as an invalid enum value", () => {
    expect(() =>
      parseRunSubmissionRequest({ ...baseRequest(), runtime: "native" })
    ).toThrow(/runtime must be one of: managed \(got "native"\)/);
  });

  it("rejects unknown runtime values with a helpful enumeration", () => {
    expect(() =>
      parseRunSubmissionRequest({ ...baseRequest(), runtime: "gpu-managed" })
    ).toThrow(/runtime must be one of: managed/);
  });
});

describe("managed runtime unsupported features", () => {
  it("rejects provider-hosted skill refs at parse time with feature_runtime_mismatch", () => {
    const req = baseRequest({ provider: "anthropic", runtime: "managed" });
    let captured: unknown;
    try {
      parseRunSubmissionRequest({
        ...req,
        submission: {
          ...req.submission,
          skills: [{ kind: "provider", vendor: "anthropic", skillId: "pdf" } as const]
        }
      });
    } catch (err) {
      captured = err;
    }
    expect(captured).toBeInstanceOf(RuntimeValidationError);
    expect((captured as RuntimeValidationError).code).toBe("feature_runtime_mismatch");
    expect((captured as RuntimeValidationError).message).toMatch(/Skill\.provider\("anthropic", "pdf"\)/);
    expect((captured as RuntimeValidationError).message).not.toMatch(/switch to runtime/);
  });

  it("rejects provider-hosted skill refs when runtime is omitted", () => {
    const req = baseRequest({ provider: "anthropic" });
    expect(() =>
      parseRunSubmissionRequest({
        ...req,
        submission: {
          ...req.submission,
          skills: [{ kind: "provider", vendor: "custom", skillId: "research", version: "v2" } as const]
        }
      })
    ).toThrowError(RuntimeValidationError);
  });

  it("collectManagedUnsupportedFeatures lists provider skill refs and ignores asset skill refs", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseRequest(),
      submission: {
        ...baseRequest().submission,
        skills: [assetRef("rules", 1)]
      }
    });
    expect(collectManagedUnsupportedFeatures(parsed)).toEqual([]);

    const direct: PlatformRunSubmissionRequest = {
      ...parsed,
      submission: {
        ...parsed.submission,
        skills: [{ kind: "provider", vendor: "anthropic", skillId: "pdf", version: "2024-09" }]
      }
    };
    expect(collectManagedUnsupportedFeatures(direct)).toEqual([
      `Skill.provider("anthropic", "pdf", "2024-09")`
    ]);
    expect(() => selectRuntime(direct)).toThrowError(RuntimeValidationError);
  });
});

describe("RUNTIME_KINDS / RUN_PROVIDERS exports", () => {
  it("RUN_PROVIDERS is the v1 set", () => {
    expect([...RUN_PROVIDERS]).toEqual(["anthropic", "deepseek", "openai", "gemini", "mistral"]);
  });

  it("RUNTIME_KINDS exposes only managed", () => {
    expect([...RUNTIME_KINDS]).toEqual(["managed"]);
  });
});
