import { describe, expect, it } from "vitest";
import {
  BUILTIN_TOOL_NAMES,
  BuiltinTools,
  DEFAULT_BUILTIN_TOOLS,
  resolveBuiltinToolNames,
  DEFAULT_CREDENTIAL_MODE,
  REGIONS,
  RUNTIME_KINDS,
  RuntimeValidationError,
  collectManagedUnsupportedFeatures,
  DEFAULT_RUN_PROVIDER,
  Models,
  RUN_MODELS,
  RUN_MODELS_BY_PROVIDER,
  RunModels,
  Providers,
  Regions,
  RUN_PROVIDERS,
  parseRegion,
  parseRunSubmissionRequest,
  providerForModel,
  providersForModel,
  resolveProviderModelId,
  assertRunModelMatchesProvider,
  MODEL_PROVIDER_IDS,
  selectRuntime,
  type PlatformRunSubmissionRequest,
  type RunProvider,
  type Region,
  type RuntimeKind
} from "../src/index.js";

function assetRef(name: string, seed = 1) {
  const hex = String(seed).padStart(64, "0");
  return { kind: "asset" as const, assetId: `asset_${hex}`, name };
}

function baseRequest(
  overrides: Partial<{ provider: RunProvider; runtime: RuntimeKind; region: Region }> = {}
) {
  const provider = overrides.provider ?? "anthropic";
  const model = {
    anthropic: RunModels.CLAUDE_HAIKU_4_5,
    deepseek: RunModels.DEEPSEEK_CHAT,
    openai: RunModels.GPT_4_1,
    gemini: RunModels.GEMINI_2_5_FLASH,
    mistral: RunModels.MISTRAL_LARGE_LATEST,
    openrouter: RunModels.GPT_4O_MINI,
    doubao: RunModels.DOUBAO_SEED_PRO,
    "doubao-cn": RunModels.DOUBAO_SEED_FLASH
  }[provider];
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "idem-1",
    provider,
    ...(overrides.runtime !== undefined ? { runtime: overrides.runtime } : {}),
    ...(overrides.region !== undefined ? { region: overrides.region } : {}),
    submission: {
      model,
      prompt: ["hello"],
      skills: [],
      agentsMd: [],
      files: [],
      mcpServers: []
    },
    secrets: { apiKeys: { [provider]: `sk-${provider}-test` } }
  };
}

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
      secrets: { apiKeys: { anthropic: "sk-ant-default" } }
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

  it("accepts explicit BYOK credential mode", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseRequest(),
      credentialMode: "byok"
    });
    expect(parsed.credentialMode).toBe("byok");
  });

  it("rejects managed as an unknown credential mode", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseRequest(),
        credentialMode: "managed"
      })
    ).toThrow(/credentialMode must be one of: byok/);
  });

  it.each(["deepseek", "openai", "gemini", "mistral"] as const)(
    "requires a BYOK provider key when provider is %s",
    (provider) => {
      const req = baseRequest({ provider });
      expect(() =>
        parseRunSubmissionRequest({ ...req, secrets: {} })
      ).toThrow(/secrets\.apiKey is required when credentialMode is byok/);
    }
  );

  it("rejects unknown sibling keys inside the secrets bundle", () => {
    const req = baseRequest({ provider: "anthropic" });
    expect(() =>
      parseRunSubmissionRequest({
        ...req,
        secrets: { ...req.secrets, openai: { apiKey: "sk-openai-x" } }
      })
    ).toThrow(
      /secrets\.openai is not an allowed field; permitted: apiKey, apiKeys, mcpServers, proxyEndpointAuth, envSecrets/
    );
  });

  it("rejects an unknown provider key inside apiKeys", () => {
    const req = baseRequest({ provider: "anthropic" });
    expect(() =>
      parseRunSubmissionRequest({ ...req, secrets: { apiKeys: { bogus: "sk-x" } } })
    ).toThrow(/secrets\.apiKeys\["bogus"\] is not a known provider/);
  });

  it("rejects a non-string apiKeys value", () => {
    const req = baseRequest({ provider: "anthropic" });
    expect(() =>
      parseRunSubmissionRequest({ ...req, secrets: { apiKeys: { anthropic: 123 } } })
    ).toThrow(/secrets\.apiKeys\["anthropic"\] must be a non-empty string/);
  });

  it("accepts and preserves multiple provider keys for cross-provider subagents", () => {
    const req = baseRequest({ provider: "deepseek" });
    const parsed = parseRunSubmissionRequest({
      ...req,
      secrets: { apiKeys: { deepseek: "sk-ds", anthropic: "sk-ant" } }
    });
    expect(parsed.secrets.apiKeys).toEqual({ deepseek: "sk-ds", anthropic: "sk-ant" });
  });
});

describe("submission parser - regions", () => {
  it("exports the public regions and symbol accessors", () => {
    expect([...REGIONS]).toEqual(["eu-west", "us-west", "ap-northeast"]);
    expect(Object.values(Regions)).toEqual([...REGIONS]);
  });

  it("parses explicit regions", () => {
    expect(parseRegion("us-west")).toBe("us-west");
    expect(parseRegion(undefined)).toBeUndefined();
    expect(() => parseRegion("mars")).toThrow(
      /region must be one of: eu-west, us-west, ap-northeast/
    );
  });

  it("preserves an explicit top-level region and omits absent region", () => {
    expect(parseRunSubmissionRequest(baseRequest({ region: "us-west" })).region).toBe("us-west");
    expect(parseRunSubmissionRequest(baseRequest()).region).toBeUndefined();
  });

  it("rejects an invalid explicit region", () => {
    expect(() => parseRunSubmissionRequest({ ...baseRequest(), region: "ams" })).toThrow(
      /region must be one of: eu-west, us-west, ap-northeast/
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
      "claude-sonnet-4-6",
      "deepseek-v4-flash",
      "deepseek-v4-pro",
      "deepseek-chat",
      "deepseek-reasoner",
      "gpt-4.1",
      "gpt-4o-mini",
      "gpt-4o",
      "gemini-2.0-flash",
      "gemini-2.5-flash",
      "mistral-large-latest",
      "mistral-small-latest",
      "doubao-seed-pro",
      "doubao-seed-flash"
    ]);
  });

  it("RUN_PROVIDERS is the v1 set", () => {
    expect([...RUN_PROVIDERS]).toEqual([
      "anthropic",
      "deepseek",
      "openai",
      "gemini",
      "mistral",
      "openrouter",
      "doubao",
      "doubao-cn"
    ]);
  });

  it("Providers mirrors RUN_PROVIDERS exactly (no drift)", () => {
    expect(Object.values(Providers)).toEqual([...RUN_PROVIDERS]);
  });

  it("BUILTIN_TOOL_NAMES is the closed builtin tool-name set (HANDS_TOOLS order)", () => {
    expect([...BUILTIN_TOOL_NAMES]).toEqual([
      "bash",
      "read_file",
      "write_file",
      "edit_file",
      "grep",
      "glob",
      "head",
      "tail",
      "todo_write",
      "subagent",
      "subagent_result",
      "web_fetch",
      "web_search",
      "notebook_edit",
      "bash_output",
      "bash_kill",
      "code_execution",
      "wait",
      "git"
    ]);
  });

  it("BuiltinTools maps each tool name to itself and never drifts from BUILTIN_TOOL_NAMES", () => {
    expect(Object.values(BuiltinTools)).toEqual([...BUILTIN_TOOL_NAMES]);
    for (const name of BUILTIN_TOOL_NAMES) {
      expect(BuiltinTools[name]).toBe(name);
    }
  });

  it("DEFAULT_BUILTIN_TOOLS is every builtin except notebook_edit", () => {
    expect([...DEFAULT_BUILTIN_TOOLS]).toEqual(
      BUILTIN_TOOL_NAMES.filter((n) => n !== "notebook_edit")
    );
    expect(DEFAULT_BUILTIN_TOOLS).not.toContain("notebook_edit");
  });

  it("resolveBuiltinToolNames: default-on, false-off, cherry-pick, dedupe, invalid rejected", () => {
    // Default on ⇒ DEFAULT_BUILTIN_TOOLS.
    expect(resolveBuiltinToolNames(undefined)).toEqual([...DEFAULT_BUILTIN_TOOLS]);
    expect(resolveBuiltinToolNames(true)).toEqual([...DEFAULT_BUILTIN_TOOLS]);
    // false ⇒ none, unless cherry-picked.
    expect(resolveBuiltinToolNames(false)).toEqual([]);
    expect(resolveBuiltinToolNames(false, [BuiltinTools.notebook_edit])).toEqual(["notebook_edit"]);
    // Default + notebook opt-in ⇒ full set, in BUILTIN_TOOL_NAMES order, deduped.
    expect(resolveBuiltinToolNames(undefined, [BuiltinTools.notebook_edit, BuiltinTools.bash])).toEqual([
      ...BUILTIN_TOOL_NAMES
    ]);
    // Invalid builtin name rejected.
    expect(() => resolveBuiltinToolNames(false, ["nope"])).toThrow(/is not a builtin tool/);
  });

  it("RUNTIME_KINDS exposes only managed", () => {
    expect([...RUNTIME_KINDS]).toEqual(["managed"]);
  });
});

describe("providerForModel / providersForModel", () => {
  it("RUN_MODELS_BY_PROVIDER and MODEL_PROVIDER_IDS agree", () => {
    for (const [provider, models] of Object.entries(RUN_MODELS_BY_PROVIDER)) {
      for (const model of models) {
        expect(providersForModel(model), model).toContain(provider);
      }
    }
  });

  it("providerForModel returns the first declared (default) provider", () => {
    for (const model of RUN_MODELS) {
      const declared = Object.keys(MODEL_PROVIDER_IDS[model]);
      expect(providerForModel(model), model).toBe(declared[0]);
    }
  });

  it("exposes every provider that can serve a multi-provider model", () => {
    expect(providersForModel(Models.GPT_4O_MINI)).toEqual(["openai", "openrouter"]);
    expect(providerForModel(Models.GPT_4O_MINI)).toBe("openai");
    expect(providersForModel(Models.GEMINI_2_0_FLASH)).toEqual(["gemini", "openrouter"]);
    expect(providersForModel(Models.GPT_4O)).toEqual(["openrouter"]);
  });

  it("maps the new DeepSeek v4 ids to deepseek", () => {
    expect(providerForModel(Models.DEEPSEEK_V4_FLASH)).toBe("deepseek");
    expect(providerForModel(Models.DEEPSEEK_V4_PRO)).toBe("deepseek");
  });

  it("serves Doubao models from both Ark gateways (international default)", () => {
    expect(providersForModel(Models.DOUBAO_SEED_PRO)).toEqual(["doubao", "doubao-cn"]);
    expect(providersForModel(Models.DOUBAO_SEED_FLASH)).toEqual(["doubao", "doubao-cn"]);
    expect(providerForModel(Models.DOUBAO_SEED_PRO)).toBe("doubao");
  });

  it("returns undefined / empty for an unknown model string", () => {
    expect(providerForModel("not-a-model")).toBeUndefined();
    expect(providersForModel("not-a-model")).toEqual([]);
  });
});

describe("resolveProviderModelId", () => {
  it("translates a canonical id to the provider-native string", () => {
    expect(resolveProviderModelId(Models.GPT_4O_MINI, "openai")).toBe("gpt-4o-mini");
    expect(resolveProviderModelId(Models.GPT_4O_MINI, "openrouter")).toBe("openai/gpt-4o-mini");
    expect(resolveProviderModelId(Models.GEMINI_2_0_FLASH, "openrouter")).toBe("google/gemini-2.0-flash-001");
    expect(resolveProviderModelId(Models.CLAUDE_HAIKU_4_5, "anthropic")).toBe("claude-haiku-4-5");
    expect(resolveProviderModelId(Models.DOUBAO_SEED_PRO, "doubao")).toBe("doubao-seed-1-8-251228");
    expect(resolveProviderModelId(Models.DOUBAO_SEED_FLASH, "doubao-cn")).toBe("doubao-seed-1-6-flash-250828");
  });

  it("throws when the provider does not serve the model", () => {
    expect(() => resolveProviderModelId(Models.GPT_4O_MINI, "anthropic")).toThrow(/not available for provider/);
    expect(() => resolveProviderModelId(Models.CLAUDE_HAIKU_4_5, "openrouter")).toThrow(/not available for provider/);
  });
});

describe("assertRunModelMatchesProvider", () => {
  it("accepts any provider that serves the model", () => {
    expect(() => assertRunModelMatchesProvider("openai", Models.GPT_4O_MINI)).not.toThrow();
    expect(() => assertRunModelMatchesProvider("openrouter", Models.GPT_4O_MINI)).not.toThrow();
  });

  it("rejects a provider that does not serve the model", () => {
    expect(() => assertRunModelMatchesProvider("anthropic", Models.GPT_4O_MINI)).toThrow(/not supported for provider/);
  });
});

describe("submission parser - includeBuiltinTools + builtin tool refs", () => {
  it("defaults includeBuiltinTools to absent (⇒ standard set ON downstream)", () => {
    const base = baseRequest();
    const parsed = parseRunSubmissionRequest(base);
    expect(parsed.submission.includeBuiltinTools).toBeUndefined();
    expect(parsed.submission.builtinTools).toBeUndefined();
  });

  it("accepts includeBuiltinTools: false (disable all builtins)", () => {
    const base = baseRequest();
    const parsed = parseRunSubmissionRequest({
      ...base,
      submission: { ...base.submission, includeBuiltinTools: false }
    });
    expect(parsed.submission.includeBuiltinTools).toBe(false);
  });

  it("rejects a non-boolean includeBuiltinTools", () => {
    const base = baseRequest();
    expect(() =>
      parseRunSubmissionRequest({
        ...base,
        submission: { ...base.submission, includeBuiltinTools: [] as unknown }
      })
    ).toThrow(/includeBuiltinTools must be a boolean/);
  });

  it("extracts bare-string builtin refs from the tools union into builtinTools", () => {
    const base = baseRequest();
    const parsed = parseRunSubmissionRequest({
      ...base,
      submission: {
        ...base.submission,
        tools: [BuiltinTools.notebook_edit, BuiltinTools.git]
      }
    });
    // builtinTools is in BUILTIN_TOOL_NAMES order, custom tools stays empty.
    expect(parsed.submission.builtinTools).toEqual(["notebook_edit", "git"]);
    expect(parsed.submission.tools).toEqual([]);
  });

  it("dedupes repeated builtin refs in tools (BUILTIN_TOOL_NAMES order)", () => {
    const base = baseRequest();
    const parsed = parseRunSubmissionRequest({
      ...base,
      submission: {
        ...base.submission,
        tools: [BuiltinTools.web_search, BuiltinTools.web_search, BuiltinTools.notebook_edit]
      }
    });
    expect(parsed.submission.builtinTools).toEqual(["web_search", "notebook_edit"]);
  });

  it("rejects a tools string outside the closed builtin-tool set", () => {
    const base = baseRequest();
    expect(() =>
      parseRunSubmissionRequest({
        ...base,
        submission: { ...base.submission, tools: ["not_a_tool"] }
      })
    ).toThrow(/is not a builtin tool name; expected one of: bash, read_file/);
  });

  it("parses a mix of custom tool bundles and builtin refs", () => {
    const base = baseRequest();
    const customTool = {
      kind: "asset" as const,
      assetId: `asset_${"a".repeat(64)}`,
      name: "calendar_lookup",
      description: "Looks up calendar availability.",
      input_schema: { type: "object", properties: {}, required: [] },
      entry: "index.js"
    };
    const parsed = parseRunSubmissionRequest({
      ...base,
      submission: {
        ...base.submission,
        tools: [BuiltinTools.notebook_edit, customTool]
      }
    });
    expect(parsed.submission.builtinTools).toEqual(["notebook_edit"]);
    expect(parsed.submission.tools?.map((t) => t.name)).toEqual(["calendar_lookup"]);
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
      postHook: { command: "bun test" }
    });
    expect(parsed.postHook).toEqual({
      command: "bun test",
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
