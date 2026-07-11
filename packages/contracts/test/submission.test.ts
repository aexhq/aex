import { describe, expect, it } from "vitest";
import {
  BUILTIN_TOOL_NAMES,
  BuiltinTools,
  DEFAULT_BUILTIN_TOOLS,
  resolveBuiltinToolNames,
  DEFAULT_PROVIDER,
  Models,
  SUPPORTED_MODELS,
  SUPPORTED_MODELS_BY_PROVIDER,
  Providers,
  PROVIDERS,
  providerForModel,
  providersForModel,
  resolveProviderModelId,
  assertModelNameMatchesProvider,
  MODEL_PROVIDER_IDS,
  type ProviderName
} from "../src/index.js";
import { parseSessionSubmissionRequest } from "../src/internal.js";

function assetRef(name: string, seed = 1) {
  const hex = String(seed).padStart(64, "0");
  return { kind: "asset" as const, assetId: `asset_${hex}`, name };
}

function baseRequest(
  overrides: Partial<{ provider: ProviderName }> = {}
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
      assets: { files: [], skills: [], tools: [], instructions: [] },
      builtinTools: "default",
      mcpServers: []
    },
    secrets: { apiKeys: { [provider]: `sk-${provider}-test` } }
  };
}

describe("submission parser - providers and secrets", () => {
  it("accepts every provider in PROVIDERS", () => {
    for (const provider of PROVIDERS) {
      const parsed = parseSessionSubmissionRequest(baseRequest({ provider }));
      expect(parsed.provider).toBe(provider);
    }
  });

  it("rejects unknown providers with a helpful enumeration", () => {
    expect(() =>
      parseSessionSubmissionRequest({ ...baseRequest(), provider: "bogus" })
    ).toThrow(/provider must be one of: anthropic, deepseek, openai, gemini, mistral/);
  });

  it("rejects unknown model ids with a helpful enumeration", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        ...baseRequest(),
        submission: { ...baseRequest().submission, model: "model-x" }
      })
    ).toThrow(/submission\.model must be one of: claude-haiku-4-5, claude-3-5-haiku-latest/);
  });

  it("rejects provider/model mismatches", () => {
    expect(() =>
      parseSessionSubmissionRequest({
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
    const parsed = parseSessionSubmissionRequest({
      ...rest,
      secrets: { apiKeys: { anthropic: "sk-ant-default" } }
    });
    expect(DEFAULT_PROVIDER).toBe("anthropic");
    expect(parsed.provider).toBe("anthropic");
  });

  it("rejects explicit credentialMode as a removed choice field", () => {
    expect(() => parseSessionSubmissionRequest({
      ...baseRequest(),
      credentialMode: "byok"
    })).toThrow(/submission\.credentialMode is not an allowed field/);
  });

  it.each(["deepseek", "openai", "gemini", "mistral"] as const)(
    "requires a BYOK provider key when provider is %s",
    (provider) => {
      const req = baseRequest({ provider });
      expect(() =>
        parseSessionSubmissionRequest({ ...req, secrets: {} })
      ).toThrow(new RegExp(`secrets\\.apiKeys\\["${provider}"\\] is required`));
    }
  );

  it("rejects unknown sibling keys inside the secrets bundle", () => {
    const req = baseRequest({ provider: "anthropic" });
    expect(() =>
      parseSessionSubmissionRequest({
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
      parseSessionSubmissionRequest({ ...req, secrets: { apiKeys: { bogus: "sk-x" } } })
    ).toThrow(/secrets\.apiKeys\["bogus"\] is not a known provider/);
  });

  it("rejects a non-string apiKeys value", () => {
    const req = baseRequest({ provider: "anthropic" });
    expect(() =>
      parseSessionSubmissionRequest({ ...req, secrets: { apiKeys: { anthropic: 123 } } })
    ).toThrow(/secrets\.apiKeys\["anthropic"\] must be a non-empty string/);
  });

  it("accepts and preserves multiple provider keys for cross-provider subagents", () => {
    const req = baseRequest({ provider: "deepseek" });
    const parsed = parseSessionSubmissionRequest({
      ...req,
      secrets: { apiKeys: { deepseek: "sk-ds", anthropic: "sk-ant" } }
    });
    expect(parsed.secrets.apiKeys).toEqual({ deepseek: "sk-ds", anthropic: "sk-ant" });
  });
});

describe("submission parser - removed choice fields", () => {
  // `parentSessionId` was the legacy lineage field for API-submitted child sessions.
  // Child sessions are now in-brain threads, so the top-level submit contract no
  // longer accepts it — the strict allow-list rejects it like any unknown field.
  it.each(["runtime", "region", "credentialMode", "parentSessionId"] as const)(
    "rejects top-level %s",
    (field) => {
      expect(() =>
        parseSessionSubmissionRequest({ ...baseRequest(), [field]: "managed" })
      ).toThrow(new RegExp(`submission\\.${field} is not an allowed field`));
    }
  );
});

describe("PROVIDERS exports", () => {
  it("SUPPORTED_MODELS is the public model allowlist", () => {
    expect([...SUPPORTED_MODELS]).toEqual([
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

  it("PROVIDERS is the v1 set", () => {
    expect([...PROVIDERS]).toEqual([
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

  it("Providers mirrors PROVIDERS exactly (no drift)", () => {
    expect(Object.values(Providers)).toEqual([...PROVIDERS]);
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
      "git",
      "ls",
      "stat",
      "wc"
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

  it("resolveBuiltinToolNames handles default, none, explicit selection, dedupe, and rejection", () => {
    expect(resolveBuiltinToolNames()).toEqual([...DEFAULT_BUILTIN_TOOLS]);
    expect(resolveBuiltinToolNames("default")).toEqual([...DEFAULT_BUILTIN_TOOLS]);
    expect(resolveBuiltinToolNames("none")).toEqual([]);
    for (const name of BUILTIN_TOOL_NAMES) {
      expect(resolveBuiltinToolNames([name]), name).toEqual([name]);
    }
    expect(resolveBuiltinToolNames([BuiltinTools.git, BuiltinTools.bash, BuiltinTools.git])).toEqual(
      BUILTIN_TOOL_NAMES.filter((name) => name === BuiltinTools.bash || name === BuiltinTools.git)
    );
    expect(() => resolveBuiltinToolNames(["nope" as never])).toThrow(/is not a builtin tool/);
    expect(() => resolveBuiltinToolNames(["notebook_edit" as never])).toThrow(/is not a builtin tool/);
  });

});

describe("providerForModel / providersForModel", () => {
  it("SUPPORTED_MODELS_BY_PROVIDER and MODEL_PROVIDER_IDS agree", () => {
    for (const [provider, models] of Object.entries(SUPPORTED_MODELS_BY_PROVIDER)) {
      for (const model of models) {
        expect(providersForModel(model), model).toContain(provider);
      }
    }
  });

  it("providerForModel returns the first declared (default) provider", () => {
    for (const model of SUPPORTED_MODELS) {
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

describe("assertModelNameMatchesProvider", () => {
  it("accepts any provider that serves the model", () => {
    expect(() => assertModelNameMatchesProvider("openai", Models.GPT_4O_MINI)).not.toThrow();
    expect(() => assertModelNameMatchesProvider("openrouter", Models.GPT_4O_MINI)).not.toThrow();
  });

  it("rejects a provider that does not serve the model", () => {
    expect(() => assertModelNameMatchesProvider("anthropic", Models.GPT_4O_MINI)).toThrow(/not supported for provider/);
  });
});

describe("submission parser - builtinTools", () => {
  it("normalizes an omitted selection to default", () => {
    const base = baseRequest();
    const { builtinTools: _selection, ...submission } = base.submission;
    const parsed = parseSessionSubmissionRequest({ ...base, submission });
    expect(parsed.submission.builtinTools).toBe("default");
  });

  it("accepts none and explicit selections", () => {
    const base = baseRequest();
    const none = parseSessionSubmissionRequest({
      ...base,
      submission: { ...base.submission, builtinTools: "none" }
    });
    expect(none.submission.builtinTools).toBe("none");
    const selected = parseSessionSubmissionRequest({
      ...base,
      submission: { ...base.submission, builtinTools: [BuiltinTools.wait, BuiltinTools.git] }
    });
    expect(selected.submission.builtinTools).toEqual(["wait", "git"]);
  });

  it("dedupes explicit selections in canonical order", () => {
    const base = baseRequest();
    const parsed = parseSessionSubmissionRequest({
      ...base,
      submission: {
        ...base.submission,
        builtinTools: [BuiltinTools.web_search, BuiltinTools.web_search, BuiltinTools.git]
      }
    });
    expect(parsed.submission.builtinTools).toEqual(["web_search", "git"]);
  });

  it("rejects names outside the closed builtin-tool set", () => {
    const base = baseRequest();
    expect(() =>
      parseSessionSubmissionRequest({
        ...base,
        submission: { ...base.submission, builtinTools: ["not_a_tool"] }
      })
    ).toThrow(/not a builtin tool/);
  });
});

describe("submission parser - outputMode", () => {
  it("accepts 'buffered' and 'stream'", () => {
    for (const mode of ["buffered", "stream"] as const) {
      const req = baseRequest();
      const parsed = parseSessionSubmissionRequest({
        ...req,
        submission: { ...req.submission, outputMode: mode }
      });
      expect(parsed.submission.outputMode).toBe(mode);
    }
  });

  it("omits outputMode when not provided", () => {
    const parsed = parseSessionSubmissionRequest(baseRequest());
    expect("outputMode" in parsed.submission).toBe(false);
  });

  it("rejects an unknown outputMode", () => {
    const req = baseRequest();
    expect(() =>
      parseSessionSubmissionRequest({ ...req, submission: { ...req.submission, outputMode: "fast" } })
    ).toThrow(/outputMode/);
  });
});

describe("submission parser - removed postHook", () => {
  it("rejects postHook on the public submission surface", () => {
    expect(() =>
      parseSessionSubmissionRequest({
        ...baseRequest(),
        postHook: { command: "bun test" }
      })
    ).toThrow(/submission\.postHook is not an allowed field/);
  });
});
