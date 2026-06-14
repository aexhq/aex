import { describe, expect, it } from "vitest";
import {
  BLOCKED_MANAGED_KEY_FEATURE_POLICY_V1,
  DEFAULT_CREDENTIAL_MODE,
  ManagedKeyUnavailableError,
  RUNTIME_KINDS,
  RuntimeValidationError,
  collectManagedUnsupportedFeatures,
  DEFAULT_RUN_PROVIDER,
  Models,
  RUN_MODELS,
  RUN_MODELS_BY_PROVIDER,
  RunModels,
  BUILTINS,
  Builtins,
  Providers,
  RUN_PROVIDERS,
  parseRunSubmissionRequest,
  providerForModel,
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
  const model = {
    anthropic: RunModels.CLAUDE_HAIKU_4_5,
    deepseek: RunModels.DEEPSEEK_CHAT,
    openai: RunModels.GPT_4_1,
    gemini: RunModels.GEMINI_2_5_FLASH,
    mistral: RunModels.MISTRAL_LARGE_LATEST,
    openrouter: RunModels.OPENROUTER_GPT_4O_MINI
  }[provider];
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "idem-1",
    provider,
    ...(overrides.runtime !== undefined ? { runtime: overrides.runtime } : {}),
    submission: {
      model,
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
  models: [RunModels.CLAUDE_HAIKU_4_5],
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

  it("rejects unknown model ids with a helpful enumeration", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseRequest(),
        submission: { ...baseRequest().submission, model: "model-x" }
      })
    ).toThrow(/submission\.model must be one of: claude-haiku-4-5, claude-3-5-haiku-latest/);
  });

  it("rejects provider/model mismatches", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseRequest({ provider: "deepseek" }),
        submission: {
          ...baseRequest({ provider: "deepseek" }).submission,
          model: RunModels.CLAUDE_HAIKU_4_5
        }
      })
    ).toThrow(/not supported for provider deepseek/);
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
    expect((captured as RuntimeValidationError).message).toMatch(/provider skill "anthropic\/pdf" \(kind:"provider"\)/);
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
      `provider skill "anthropic/pdf@2024-09" (kind:"provider")`
    ]);
    expect(() => selectRuntime(direct)).toThrowError(RuntimeValidationError);
  });
});

describe("RUNTIME_KINDS / RUN_PROVIDERS exports", () => {
  it("RUN_MODELS is the public model allowlist", () => {
    expect([...RUN_MODELS]).toEqual([
      "claude-haiku-4-5",
      "claude-3-5-haiku-latest",
      "claude-3-5-sonnet-latest",
      "deepseek-v4-flash",
      "deepseek-v4-pro",
      "deepseek-chat",
      "deepseek-reasoner",
      "gpt-4.1",
      "gpt-4o-mini",
      "gemini-2.0-flash",
      "gemini-2.5-flash",
      "mistral-large-latest",
      "mistral-small-latest",
      "openai/gpt-4o-mini",
      "openai/gpt-4o",
      "google/gemini-2.0-flash-001"
    ]);
  });

  it("RUN_PROVIDERS is the v1 set", () => {
    expect([...RUN_PROVIDERS]).toEqual([
      "anthropic",
      "deepseek",
      "openai",
      "gemini",
      "mistral",
      "openrouter"
    ]);
  });

  it("Providers mirrors RUN_PROVIDERS exactly (no drift)", () => {
    expect(Object.values(Providers)).toEqual([...RUN_PROVIDERS]);
  });

  it("BUILTINS is the closed managed-runtime builtin set", () => {
    expect([...BUILTINS]).toEqual([
      "developer",
      "computercontroller",
      "memory",
      "autovisualiser",
      "tutorial"
    ]);
  });

  it("RUNTIME_KINDS exposes only managed", () => {
    expect([...RUNTIME_KINDS]).toEqual(["managed"]);
  });
});

describe("providerForModel", () => {
  it("resolves every RUN_MODEL to its declared provider", () => {
    for (const [provider, models] of Object.entries(RUN_MODELS_BY_PROVIDER)) {
      for (const model of models) {
        expect(providerForModel(model), model).toBe(provider);
      }
    }
  });

  it("maps the new DeepSeek v4 ids to deepseek", () => {
    expect(providerForModel(Models.DEEPSEEK_V4_FLASH)).toBe("deepseek");
    expect(providerForModel(Models.DEEPSEEK_V4_PRO)).toBe("deepseek");
  });

  it("returns undefined for an unknown model string", () => {
    expect(providerForModel("not-a-model")).toBeUndefined();
  });
});

describe("submission parser - builtins (closed set)", () => {
  it("accepts every member of BUILTINS", () => {
    const base = baseRequest();
    const parsed = parseRunSubmissionRequest({
      ...base,
      submission: { ...base.submission, builtins: [...BUILTINS] }
    });
    expect(parsed.submission.builtins).toEqual([...BUILTINS]);
  });

  it("accepts an empty array (disable all builtins)", () => {
    const base = baseRequest();
    const parsed = parseRunSubmissionRequest({
      ...base,
      submission: { ...base.submission, builtins: [] }
    });
    expect(parsed.submission.builtins).toEqual([]);
  });

  it("rejects a builtin outside the closed set with an enumeration", () => {
    const base = baseRequest();
    expect(() =>
      parseRunSubmissionRequest({
        ...base,
        submission: { ...base.submission, builtins: ["developr"] }
      })
    ).toThrow(/is not a managed-runtime builtin; expected one of: developer, computercontroller/);
  });

  it("dedupes repeated builtins", () => {
    const base = baseRequest();
    const parsed = parseRunSubmissionRequest({
      ...base,
      submission: {
        ...base.submission,
        builtins: [Builtins.DEVELOPER, Builtins.DEVELOPER, Builtins.MEMORY]
      }
    });
    expect(parsed.submission.builtins).toEqual(["developer", "memory"]);
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

describe("submission parser - postHook", () => {
  it("accepts postHook.command and applies defaults", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseRequest(),
      postHook: { command: "pnpm test" }
    });
    expect(parsed.postHook).toEqual({
      command: "pnpm test",
      timeoutMs: 300_000,
      maxTurns: 10,
      maxChars: null
    });
  });

  it("omits postHook when command is empty", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseRequest(),
      postHook: { command: "   " }
    });
    expect(parsed.postHook).toBeUndefined();
  });

  it("accepts timeout, maxTurns, and maxChars", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseRequest(),
      postHook: {
        command: "npm run verify",
        timeout: "30s",
        maxTurns: 2,
        maxChars: 4096
      }
    });
    expect(parsed.postHook).toEqual({
      command: "npm run verify",
      timeoutMs: 30_000,
      maxTurns: 2,
      maxChars: 4096
    });
  });

  it("accepts maxChars null as an explicit unbounded output budget", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseRequest(),
      postHook: {
        command: "npm test",
        maxChars: null
      }
    });
    expect(parsed.postHook?.maxChars).toBeNull();
  });

  it("rejects unknown nested fields", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseRequest(),
        postHook: { command: "npm test", env: { CI: "1" } }
      })
    ).toThrow(/submission\.postHook\.env is not an allowed field/);
  });

  it("rejects invalid postHook budgets", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseRequest(),
        postHook: { command: "npm test", timeout: "0ms" }
      })
    ).toThrow(/postHook\.timeout must be greater than 0ms/);
    expect(() =>
      parseRunSubmissionRequest({
        ...baseRequest(),
        postHook: { command: "npm test", maxTurns: -1 }
      })
    ).toThrow(/postHook\.maxTurns must be a non-negative integer/);
    expect(() =>
      parseRunSubmissionRequest({
        ...baseRequest(),
        postHook: { command: "npm test", maxChars: 1.5 }
      })
    ).toThrow(/postHook\.maxChars must be a non-negative integer/);
  });
});
