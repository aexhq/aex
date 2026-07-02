import { describe, expect, it } from "vitest";
import {
  BUILTIN_TOOL_NAMES,
  BuiltinTools,
  DEFAULT_BUILTIN_TOOLS,
  resolveBuiltinToolNames,
  DEFAULT_RUN_PROVIDER,
  Models,
  RUN_MODELS,
  RUN_MODELS_BY_PROVIDER,
  Providers,
  RUN_PROVIDERS,
  parseRunSubmissionRequest,
  providerForModel,
  providersForModel,
  resolveProviderModelId,
  assertRunModelMatchesProvider,
  MODEL_PROVIDER_IDS,
  type RunProvider
} from "../src/index.js";

function assetRef(name: string, seed = 1) {
  const hex = String(seed).padStart(64, "0");
  return { kind: "asset" as const, assetId: `asset_${hex}`, name };
}

function baseRequest(
  overrides: Partial<{ provider: RunProvider }> = {}
) {
  const provider = overrides.provider ?? "anthropic";
  const model = {
    anthropic: Models.CLAUDE_HAIKU_4_5,
    deepseek: Models.DEEPSEEK_V4_FLASH,
    openai: Models.GPT_4_1,
    gemini: Models.GEMINI_2_5_FLASH,
    mistral: Models.MISTRAL_LARGE_LATEST,
    openrouter: Models.GPT_4O_MINI,
    doubao: Models.DOUBAO_SEED_PRO,
    "doubao-cn": Models.DOUBAO_SEED_FLASH
  }[provider];
  return {
    workspaceId: "workspace-1",
    idempotencyKey: "idem-1",
    provider,
    submission: {
      model,
      prompt: ["hello"],
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
          model: Models.CLAUDE_HAIKU_4_5
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

  it("rejects explicit credentialMode as a removed choice field", () => {
    expect(() => parseRunSubmissionRequest({
      ...baseRequest(),
      credentialMode: "byok"
    })).toThrow(/submission\.credentialMode is not an allowed field/);
  });

  it.each(["deepseek", "openai", "gemini", "mistral"] as const)(
    "requires a BYOK provider key when provider is %s",
    (provider) => {
      const req = baseRequest({ provider });
      expect(() =>
        parseRunSubmissionRequest({ ...req, secrets: {} })
      ).toThrow(new RegExp(`secrets\\.apiKeys\\["${provider}"\\] is required`));
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
      /secrets\.openai is not an allowed field; permitted: apiKeys, mcpServers, envSecrets/
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

describe("submission parser - removed choice fields", () => {
  // `parentRunId` was the legacy lineage field for API-submitted child runs.
  // Child runs are now in-brain threads, so the top-level submit contract no
  // longer accepts it — the strict allow-list rejects it like any unknown field.
  it.each(["runtime", "region", "credentialMode", "parentRunId"] as const)(
    "rejects top-level %s",
    (field) => {
      expect(() =>
        parseRunSubmissionRequest({ ...baseRequest(), [field]: "managed" })
      ).toThrow(new RegExp(`submission\\.${field} is not an allowed field`));
    }
  );
});

describe("submission parser - skill tools", () => {
  const skillTool = {
    kind: "skill" as const,
    assetId: `asset_${"a".repeat(64)}`,
    name: "report-writer",
    description: "Load the report-writer skill."
  };

  it("splits a skill-tool out of the tools union into submission.skillTools", () => {
    const req = baseRequest({ provider: "anthropic" });
    const parsed = parseRunSubmissionRequest({
      ...req,
      submission: { ...req.submission, tools: [skillTool] }
    });
    expect(parsed.submission.skillTools).toEqual([skillTool]);
    // A skill-tool is not a custom ToolRef, so the `tools` bundle list is empty.
    expect(parsed.submission.tools).toEqual([]);
  });

  it("rejects a skill-tool name containing the reserved '__' separator", () => {
    const req = baseRequest({ provider: "anthropic" });
    expect(() =>
      parseRunSubmissionRequest({
        ...req,
        submission: { ...req.submission, tools: [{ ...skillTool, name: "bad__name" }] }
      })
    ).toThrow(/"__"/);
  });

  it("rejects a skill-tool with an unknown field", () => {
    const req = baseRequest({ provider: "anthropic" });
    expect(() =>
      parseRunSubmissionRequest({
        ...req,
        submission: { ...req.submission, tools: [{ ...skillTool, entry: "SKILL.md" }] }
      })
    ).toThrow(/not an allowed field for a skill tool/);
  });
});

describe("RUN_PROVIDERS exports", () => {
  it("RUN_MODELS is the public model allowlist", () => {
    expect([...RUN_MODELS]).toEqual([
      "claude-haiku-4-5",
      "claude-3-5-haiku-latest",
      "claude-3-5-sonnet-latest",
      "claude-sonnet-4-6",
      "deepseek-v4-flash",
      "deepseek-v4-pro",
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

  it("DEFAULT_BUILTIN_TOOLS is the complete closed builtin set", () => {
    expect([...DEFAULT_BUILTIN_TOOLS]).toEqual([...BUILTIN_TOOL_NAMES]);
  });

  it("resolveBuiltinToolNames: default-on, false-off, every builtin cherry-pickable, dedupe, invalid rejected", () => {
    // Default on ⇒ DEFAULT_BUILTIN_TOOLS.
    expect(resolveBuiltinToolNames(undefined)).toEqual([...DEFAULT_BUILTIN_TOOLS]);
    expect(resolveBuiltinToolNames(true)).toEqual([...DEFAULT_BUILTIN_TOOLS]);
    // false ⇒ none, unless a valid builtin is cherry-picked.
    expect(resolveBuiltinToolNames(false)).toEqual([]);
    for (const name of BUILTIN_TOOL_NAMES) {
      expect(resolveBuiltinToolNames(false, [name]), name).toEqual([name]);
    }
    // Default + repeated refs stays the full set, in BUILTIN_TOOL_NAMES order.
    expect(resolveBuiltinToolNames(undefined, [BuiltinTools.git, BuiltinTools.bash, BuiltinTools.git])).toEqual(
      [...BUILTIN_TOOL_NAMES]
    );
    // Invalid builtin name rejected.
    expect(() => resolveBuiltinToolNames(false, ["nope"])).toThrow(/is not a builtin tool/);
    expect(() => resolveBuiltinToolNames(false, ["notebook_edit"])).toThrow(/is not a builtin tool/);
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
        tools: [BuiltinTools.wait, BuiltinTools.git]
      }
    });
    // builtinTools is in BUILTIN_TOOL_NAMES order, custom tools stays empty.
    expect(parsed.submission.builtinTools).toEqual(["wait", "git"]);
    expect(parsed.submission.tools).toEqual([]);
  });

  it.each(BUILTIN_TOOL_NAMES)(
    "accepts %s as an individual builtin reference",
    (name) => {
      const base = baseRequest();
      const parsed = parseRunSubmissionRequest({
        ...base,
        submission: { ...base.submission, includeBuiltinTools: false, tools: [name] }
      });
      expect(parsed.submission.includeBuiltinTools).toBe(false);
      expect(parsed.submission.builtinTools).toEqual([name]);
      expect(parsed.submission.tools).toEqual([]);
    }
  );

  it("dedupes repeated builtin refs in tools (BUILTIN_TOOL_NAMES order)", () => {
    const base = baseRequest();
    const parsed = parseRunSubmissionRequest({
      ...base,
      submission: {
        ...base.submission,
        tools: [BuiltinTools.web_search, BuiltinTools.web_search, BuiltinTools.git]
      }
    });
    expect(parsed.submission.builtinTools).toEqual(["web_search", "git"]);
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

  it("rejects the removed notebook_edit builtin", () => {
    const base = baseRequest();
    expect(() =>
      parseRunSubmissionRequest({
        ...base,
        submission: { ...base.submission, tools: ["notebook_edit"] }
      })
    ).toThrow(/is not a builtin tool name/);
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
        tools: [BuiltinTools.git, customTool]
      }
    });
    expect(parsed.submission.builtinTools).toEqual(["git"]);
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

describe("submission parser - removed postHook", () => {
  it("rejects postHook on the public submission surface", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseRequest(),
        postHook: { command: "bun test" }
      })
    ).toThrow(/submission\.postHook is not an allowed field/);
  });
});
