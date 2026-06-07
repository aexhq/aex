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
    secrets: { apiKey: `sk-${provider}-test` }
  };
}

const injectedManagedKeyPolicy = {
  schemaVersion: 1,
  credentialMode: "managed",
  launchStage: "pilot",
  privateImplementationAvailable: true,
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
      secrets: { apiKey: "sk-ant-default" }
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

  it("rejects managed-key credential mode until the service is available", () => {
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

  it("rejects a caller-supplied apiKey in managed-key mode", () => {
    expect(() =>
      parseRunSubmissionRequest(
        {
          ...baseRequest({ provider: "anthropic", runtime: "managed" }),
          credentialMode: "managed"
        },
        { managedKeyPolicy: injectedManagedKeyPolicy }
      )
    ).toThrow(/secrets\.apiKey is not allowed when credentialMode is managed/);
  });

  it.each(["deepseek", "openai", "gemini", "mistral"] as const)(
    "requires secrets.apiKey when provider is %s",
    (provider) => {
      const req = baseRequest({ provider });
      expect(() =>
        parseRunSubmissionRequest({ ...req, secrets: {} })
      ).toThrow(/secrets\.apiKey is required when credentialMode is byok/);
    }
  );

  it("rejects unknown sibling keys inside the flat secrets bundle", () => {
    const req = baseRequest({ provider: "anthropic" });
    expect(() =>
      parseRunSubmissionRequest({
        ...req,
        secrets: { ...req.secrets, openai: { apiKey: "sk-openai-x" } }
      })
    ).toThrow(
      /secrets\.openai is not an allowed field; permitted: apiKey, mcpServers, proxyEndpointAuth/
    );
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

describe("submission parser - outputMode", () => {
  it("accepts 'buffered' and 'stream'", () => {
    for (const mode of ["buffered", "stream"] as const) {
      const req = baseRequest();
      const parsed = parseRunSubmissionRequest({
        ...req,
        submission: { ...req.submission, outputMode: mode }
      });
      expect(parsed.submission.outputMode).toBe(mode);
    }
  });

  it("omits outputMode when not provided", () => {
    const parsed = parseRunSubmissionRequest(baseRequest());
    expect("outputMode" in parsed.submission).toBe(false);
  });

  it("rejects an unknown outputMode", () => {
    const req = baseRequest();
    expect(() =>
      parseRunSubmissionRequest({ ...req, submission: { ...req.submission, outputMode: "fast" } })
    ).toThrow(/outputMode/);
  });
});

describe("submission parser - bootstrap / direct inputs", () => {
  const HEX = "a".repeat(64);
  function directInput(overrides: Record<string, unknown> = {}) {
    return {
      inputId: `input_skill_0_${HEX}`,
      role: "skill",
      assetId: `asset_${HEX}`,
      name: "rules",
      sha256: `sha256:${HEX}`,
      sizeBytes: 128,
      contentType: "application/zip",
      ...overrides
    };
  }

  it("accepts and round-trips bootstrapMode:direct + directInputs", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseRequest(),
      bootstrapMode: "direct",
      directInputs: [directInput()]
    });
    expect(parsed.bootstrapMode).toBe("direct");
    expect(parsed.directInputs).toEqual([
      {
        inputId: `input_skill_0_${HEX}`,
        role: "skill",
        assetId: `asset_${HEX}`,
        name: "rules",
        sha256: `sha256:${HEX}`,
        sizeBytes: 128,
        contentType: "application/zip"
      }
    ]);
    // JSON round-trip preserves both fields on the wire snapshot.
    const roundTripped = JSON.parse(JSON.stringify(parsed));
    expect(roundTripped.bootstrapMode).toBe("direct");
    expect(roundTripped.directInputs).toHaveLength(1);
  });

  it("preserves an optional mountPath on a file role", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseRequest(),
      bootstrapMode: "direct",
      directInputs: [directInput({ role: "file", mountPath: "/workspace/data.csv" })]
    });
    expect(parsed.directInputs?.[0]).toMatchObject({ role: "file", mountPath: "/workspace/data.csv" });
  });

  it("rejects bootstrapMode without directInputs", () => {
    expect(() =>
      parseRunSubmissionRequest({ ...baseRequest(), bootstrapMode: "direct" })
    ).toThrow(/directInputs must be a non-empty array when bootstrapMode is set/);
  });

  it("rejects directInputs without bootstrapMode", () => {
    expect(() =>
      parseRunSubmissionRequest({ ...baseRequest(), directInputs: [directInput()] })
    ).toThrow(/bootstrapMode is required when directInputs are present/);
  });

  it("rejects an unknown bootstrapMode", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseRequest(),
        bootstrapMode: "bogus",
        directInputs: [directInput()]
      })
    ).toThrow(/bootstrapMode must be one of/);
  });

  it("rejects an assetId that does not match the sha256", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseRequest(),
        bootstrapMode: "direct",
        directInputs: [directInput({ assetId: `asset_${"b".repeat(64)}` })]
      })
    ).toThrow(/assetId must match sha256/);
  });

  it("rejects a non-positive sizeBytes", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseRequest(),
        bootstrapMode: "direct",
        directInputs: [directInput({ sizeBytes: 0 })]
      })
    ).toThrow(/sizeBytes must be a positive safe integer/);
  });

  it("omits both fields when neither is supplied", () => {
    const parsed = parseRunSubmissionRequest(baseRequest());
    expect("bootstrapMode" in parsed).toBe(false);
    expect("directInputs" in parsed).toBe(false);
  });
});
