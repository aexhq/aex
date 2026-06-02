import { describe, expect, it } from "vitest";
import {
  BLOCKED_MANAGED_KEY_FEATURE_POLICY_V1,
  collectNativeOnlyFeatures,
  collectNativeUnsupportedFeatures,
  DEFAULT_CREDENTIAL_MODE,
  ManagedKeyUnavailableError,
  NATIVE_RUNTIME_PROVIDERS,
  parseRunSubmissionRequest,
  PROVIDER_CAPABILITY,
  providerHasNativeAgent,
  RUN_PROVIDERS,
  RUNTIME_KINDS,
  RuntimeValidationError,
  selectRuntime,
  type ManagedKeyPolicyV1,
  type PlatformRunSubmissionRequest,
  type RunProvider,
  type RuntimeKind
} from "../src/index.js";

// An R2-anchored asset ref (inline skill / file). Mirrors the SDK's
// content-hash upload shape. `seed` keeps the 64-hex path unique within a
// submission (the parser rejects duplicate r2 paths).
function r2Ref(name: string, seed = 1) {
  const hex = String(seed).padStart(64, "0"); // all-digit ⇒ valid [0-9a-f]{64}
  const WS = "11111111-1111-4111-8111-111111111111";
  return { kind: "r2" as const, path: `assets/${WS}/${hex}`, hash: `sha256:${hex}`, sizeBytes: 100, name };
}

// One canonical submission shape — every test starts from this and
// mutates only the fields it cares about. The submission has no
// native-only features by default; tests that want one add a provider
// skill ref explicitly.
function baseRequest(overrides: Partial<{ provider: RunProvider; runtime: RuntimeKind }> = {}) {
  const provider = overrides.provider ?? "anthropic";
  // Pick the secret block that matches the provider — every other
  // provider's block must be absent or `enforceProviderSecretCoupling`
  // rejects the submission.
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
  runtimes: ["native", "managed"],
  models: ["model-x"],
  features: {
    ...BLOCKED_MANAGED_KEY_FEATURE_POLICY_V1
  }
} satisfies ManagedKeyPolicyV1;

describe("submission parser — provider widening", () => {
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
      // Even though provider is omitted on the wire, secrets must match
      // the default ("anthropic") — otherwise enforceProviderSecretCoupling
      // throws. We carry an Anthropic secret in baseRequest already.
      secrets: { anthropic: { apiKey: "sk-ant-default" } }
    });
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
      runtime: "native"
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
          ...baseRequest({ provider: "anthropic", runtime: "native" }),
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

  it("rejects cross-provider secret leakage (openai key shipped with anthropic submission)", () => {
    const req = baseRequest({ provider: "anthropic" });
    expect(() =>
      parseRunSubmissionRequest({
        ...req,
        secrets: { ...req.secrets, openai: { apiKey: "sk-openai-x" } }
      })
    ).toThrow(/secrets\.openai is not allowed when provider is anthropic/);
  });
});

describe("submission parser — runtime field", () => {
  it("accepts an explicit runtime: 'native' on anthropic", () => {
    const parsed = parseRunSubmissionRequest(baseRequest({ provider: "anthropic", runtime: "native" }));
    expect(parsed.runtime).toBe("native");
  });

  it("accepts an explicit runtime: 'managed' on anthropic", () => {
    const parsed = parseRunSubmissionRequest(baseRequest({ provider: "anthropic", runtime: "managed" }));
    expect(parsed.runtime).toBe("managed");
  });

  it.each(["deepseek", "openai", "gemini", "mistral"] as const)(
    "accepts an explicit runtime: 'managed' on %s",
    (provider) => {
      const parsed = parseRunSubmissionRequest(baseRequest({ provider, runtime: "managed" }));
      expect(parsed.runtime).toBe("managed");
    }
  );

  it.each(["deepseek", "openai", "gemini", "mistral"] as const)(
    "rejects runtime: 'native' for non-anthropic provider %s with runtime_native_unsupported",
    (provider) => {
      let captured: unknown;
      try {
        parseRunSubmissionRequest(baseRequest({ provider, runtime: "native" }));
      } catch (err) {
        captured = err;
      }
      expect(captured).toBeInstanceOf(RuntimeValidationError);
      expect((captured as RuntimeValidationError).code).toBe("runtime_native_unsupported");
      expect((captured as RuntimeValidationError).message).toMatch(
        /runtime: "native" is only supported for provider:/
      );
    }
  );

  it("rejects unknown runtime values with a helpful enumeration", () => {
    expect(() =>
      parseRunSubmissionRequest({ ...baseRequest(), runtime: "gpu-managed" })
    ).toThrow(/runtime must be one of: native, managed/);
  });

  it("omits runtime from the parsed request when the wire field is absent", () => {
    const parsed = parseRunSubmissionRequest(baseRequest());
    expect(parsed.runtime).toBeUndefined();
  });
});

describe("selectRuntime — auto routing", () => {
  it("routes provider: 'anthropic' to native by default", () => {
    const parsed = parseRunSubmissionRequest(baseRequest({ provider: "anthropic" }));
    expect(selectRuntime(parsed)).toBe("native");
  });

  it.each(["deepseek", "openai", "gemini", "mistral"] as const)(
    "routes provider: '%s' to managed by default",
    (provider) => {
      const parsed = parseRunSubmissionRequest(baseRequest({ provider }));
      expect(selectRuntime(parsed)).toBe("managed");
    }
  );
});

describe("selectRuntime — explicit choice", () => {
  it("honours runtime: 'native' on anthropic", () => {
    const parsed = parseRunSubmissionRequest(baseRequest({ provider: "anthropic", runtime: "native" }));
    expect(selectRuntime(parsed)).toBe("native");
  });

  it("honours runtime: 'managed' on anthropic (no native-only features)", () => {
    const parsed = parseRunSubmissionRequest(baseRequest({ provider: "anthropic", runtime: "managed" }));
    expect(selectRuntime(parsed)).toBe("managed");
  });

  it("re-rejects runtime: 'native' on non-anthropic providers (defense in depth)", () => {
    // Bypass parser-level validation by constructing the request shape
    // directly. The dispatcher must still reject the mismatch.
    const direct: PlatformRunSubmissionRequest = {
      workspaceId: "ws",
      idempotencyKey: "id",
      credentialMode: "byok",
      provider: "openai",
      runtime: "native",
      submission: {
        model: "m",
        prompt: ["p"],
        skills: [],
        agentsMd: [],
        files: [],
        mcpServers: []
      },
      secrets: { openai: { apiKey: "sk-openai" } }
    };
    expect(() => selectRuntime(direct)).toThrowError(RuntimeValidationError);
    try {
      selectRuntime(direct);
    } catch (err) {
      expect((err as RuntimeValidationError).code).toBe("runtime_native_unsupported");
    }
  });
});

describe("feature_runtime_mismatch (managed rejects native-only features)", () => {
  // The parser fails closed on (runtime: 'managed' + native-only feature)
  // so the validation happens at parseRunSubmissionRequest time — no
  // caller can skip it the way the dashboard BFF used to. selectRuntime
  // still works on a parsed request, but the parser is now the single
  // point of validation.
  it("rejects Skill.provider('anthropic', 'pdf') on runtime: 'managed' at parse time", () => {
    const req = baseRequest({ provider: "anthropic", runtime: "managed" });
    const withSkill = {
      ...req,
      submission: {
        ...req.submission,
        skills: [{ kind: "provider", vendor: "anthropic", skillId: "pdf" } as const]
      }
    };
    let captured: unknown;
    try {
      parseRunSubmissionRequest(withSkill);
    } catch (err) {
      captured = err;
    }
    expect(captured).toBeInstanceOf(RuntimeValidationError);
    expect((captured as RuntimeValidationError).code).toBe("feature_runtime_mismatch");
    expect((captured as RuntimeValidationError).message).toMatch(/Skill\.provider\("anthropic", "pdf"\)/);
    expect((captured as RuntimeValidationError).message).toMatch(/Remove them or switch to runtime: "native"/);
  });

  it("rejects custom-vendor provider skill refs on managed too", () => {
    const req = baseRequest({ provider: "anthropic", runtime: "managed" });
    expect(() =>
      parseRunSubmissionRequest({
        ...req,
        submission: {
          ...req.submission,
          skills: [
            { kind: "provider", vendor: "custom", skillId: "research", version: "v2" } as const
          ]
        }
      })
    ).toThrowError(/Skill\.provider\("custom", "research", "v2"\)/);
  });

  it("accepts Skill.provider('anthropic', ...) on runtime: 'native' (default for anthropic)", () => {
    const req = baseRequest({ provider: "anthropic", runtime: "native" });
    const parsed = parseRunSubmissionRequest({
      ...req,
      submission: {
        ...req.submission,
        skills: [{ kind: "provider", vendor: "anthropic", skillId: "pdf" } as const]
      }
    });
    expect(selectRuntime(parsed)).toBe("native");
  });

  it("accepts Skill.provider('anthropic', ...) under auto-routing (defaults to native)", () => {
    const req = baseRequest({ provider: "anthropic" });
    const parsed = parseRunSubmissionRequest({
      ...req,
      submission: {
        ...req.submission,
        skills: [{ kind: "provider", vendor: "anthropic", skillId: "pdf" } as const]
      }
    });
    expect(selectRuntime(parsed)).toBe("native");
  });

  it("lists every offending feature, not just the first", () => {
    const req = baseRequest({ provider: "anthropic", runtime: "managed" });
    let captured: RuntimeValidationError | undefined;
    try {
      parseRunSubmissionRequest({
        ...req,
        submission: {
          ...req.submission,
          skills: [
            { kind: "provider", vendor: "anthropic", skillId: "pdf" } as const,
            { kind: "provider", vendor: "anthropic", skillId: "xlsx" } as const
          ]
        }
      });
    } catch (err) {
      captured = err as RuntimeValidationError;
    }
    expect(captured?.message).toMatch(/Skill\.provider\("anthropic", "pdf"\)/);
    expect(captured?.message).toMatch(/Skill\.provider\("anthropic", "xlsx"\)/);
  });
});

describe("collectNativeOnlyFeatures", () => {
  it("returns an empty list when there are no provider skill refs", () => {
    const parsed = parseRunSubmissionRequest(baseRequest());
    expect(collectNativeOnlyFeatures(parsed)).toEqual([]);
  });

  it("does not flag R2-anchored skill refs", () => {
    const req = baseRequest();
    const WS = "11111111-1111-4111-8111-111111111111";
    const hex = "a".repeat(64);
    const parsed = parseRunSubmissionRequest({
      ...req,
      submission: {
        ...req.submission,
        skills: [
          {
            kind: "r2",
            path: `assets/${WS}/${hex}`,
            hash: `sha256:${hex}`,
            sizeBytes: 100,
            name: "rules"
          } as const
        ]
      }
    });
    expect(collectNativeOnlyFeatures(parsed)).toEqual([]);
  });

  it("formats version suffix when ProviderSkillRef carries one", () => {
    const req = baseRequest({ provider: "anthropic", runtime: "native" });
    const parsed = parseRunSubmissionRequest({
      ...req,
      submission: {
        ...req.submission,
        skills: [
          { kind: "provider", vendor: "anthropic", skillId: "pdf", version: "2024-09" } as const
        ]
      }
    });
    expect(collectNativeOnlyFeatures(parsed)).toEqual([
      `Skill.provider("anthropic", "pdf", "2024-09")`
    ]);
  });
});

describe("provider × runtime × feature matrix", () => {
  // Build the full matrix of (provider, runtime, has provider skill)
  // and assert the outcome the dispatcher should produce. The matrix
  // doubles as documentation for the customer-surface invariant —
  // any change here must be intentional.
  type Outcome =
    | { kind: "ok"; runtime: RuntimeKind }
    | { kind: "throws"; code: "runtime_native_unsupported" | "feature_runtime_mismatch" };

  const cases: ReadonlyArray<{
    provider: RunProvider;
    runtime: RuntimeKind | undefined;
    nativeOnlyFeature: boolean;
    expected: Outcome;
  }> = [
    // Anthropic, no native-only feature
    { provider: "anthropic", runtime: undefined, nativeOnlyFeature: false, expected: { kind: "ok", runtime: "native" } },
    { provider: "anthropic", runtime: "native", nativeOnlyFeature: false, expected: { kind: "ok", runtime: "native" } },
    { provider: "anthropic", runtime: "managed", nativeOnlyFeature: false, expected: { kind: "ok", runtime: "managed" } },

    // Anthropic, with Skill.provider(anthropic, pdf)
    { provider: "anthropic", runtime: undefined, nativeOnlyFeature: true, expected: { kind: "ok", runtime: "native" } },
    { provider: "anthropic", runtime: "native", nativeOnlyFeature: true, expected: { kind: "ok", runtime: "native" } },
    { provider: "anthropic", runtime: "managed", nativeOnlyFeature: true, expected: { kind: "throws", code: "feature_runtime_mismatch" } },

    // Non-anthropic providers — `runtime: "native"` is a parse-time
    // failure, so we only enumerate the runtime: undefined | "managed"
    // branches here. The native rejection is covered in the parser
    // tests above.
    { provider: "deepseek", runtime: undefined, nativeOnlyFeature: false, expected: { kind: "ok", runtime: "managed" } },
    { provider: "deepseek", runtime: "managed", nativeOnlyFeature: false, expected: { kind: "ok", runtime: "managed" } },
    { provider: "openai", runtime: undefined, nativeOnlyFeature: false, expected: { kind: "ok", runtime: "managed" } },
    { provider: "openai", runtime: "managed", nativeOnlyFeature: false, expected: { kind: "ok", runtime: "managed" } },
    { provider: "gemini", runtime: undefined, nativeOnlyFeature: false, expected: { kind: "ok", runtime: "managed" } },
    { provider: "gemini", runtime: "managed", nativeOnlyFeature: false, expected: { kind: "ok", runtime: "managed" } },
    { provider: "mistral", runtime: undefined, nativeOnlyFeature: false, expected: { kind: "ok", runtime: "managed" } },
    { provider: "mistral", runtime: "managed", nativeOnlyFeature: false, expected: { kind: "ok", runtime: "managed" } },

    // Non-anthropic with a provider skill — feature_runtime_mismatch
    { provider: "openai", runtime: "managed", nativeOnlyFeature: true, expected: { kind: "throws", code: "feature_runtime_mismatch" } }
  ];

  it.each(cases)(
    "provider=$provider runtime=$runtime nativeOnly=$nativeOnlyFeature → $expected",
    ({ provider, runtime, nativeOnlyFeature, expected }) => {
      const req = baseRequest({ provider, ...(runtime !== undefined ? { runtime } : {}) });
      const withFeatures = nativeOnlyFeature
        ? {
            ...req,
            submission: {
              ...req.submission,
              skills: [{ kind: "provider" as const, vendor: "anthropic" as const, skillId: "pdf" }]
            }
          }
        : req;
      if (expected.kind === "ok") {
        const parsed = parseRunSubmissionRequest(withFeatures);
        expect(selectRuntime(parsed)).toBe(expected.runtime);
      } else {
        // feature_runtime_mismatch is now caught at parse time so the
        // validation cannot be skipped by callers that forget to invoke
        // selectRuntime. The matrix matches the parser's contract.
        let caught: unknown;
        try {
          parseRunSubmissionRequest(withFeatures);
        } catch (err) {
          caught = err;
        }
        expect(caught).toBeInstanceOf(RuntimeValidationError);
        expect((caught as RuntimeValidationError).code).toBe(expected.code);
      }
    }
  );
});

describe("RUNTIME_KINDS / RUN_PROVIDERS exports", () => {
  it("RUN_PROVIDERS is the v1 set", () => {
    expect([...RUN_PROVIDERS]).toEqual(["anthropic", "deepseek", "openai", "gemini", "mistral"]);
  });

  it("RUNTIME_KINDS exposes native + managed", () => {
    expect([...RUNTIME_KINDS]).toEqual(["native", "managed"]);
  });
});

describe("provider capability registry (native-first source of truth)", () => {
  it("covers every provider in RUN_PROVIDERS", () => {
    for (const provider of RUN_PROVIDERS) {
      expect(PROVIDER_CAPABILITY[provider]).toBeDefined();
    }
  });

  it("NATIVE_RUNTIME_PROVIDERS mirrors the registry (no drift)", () => {
    const fromRegistry = RUN_PROVIDERS.filter((p) => PROVIDER_CAPABILITY[p].nativeAgent !== null);
    expect([...NATIVE_RUNTIME_PROVIDERS].sort()).toEqual([...fromRegistry].sort());
  });

  it("only anthropic has a native agent runtime today; the rest fall back to goose", () => {
    expect(providerHasNativeAgent("anthropic")).toBe(true);
    for (const provider of ["deepseek", "openai", "gemini", "mistral"] as const) {
      expect(providerHasNativeAgent(provider)).toBe(false);
      expect(PROVIDER_CAPABILITY[provider].nativeAgent).toBeNull();
    }
  });

  it("anthropic's native serves inlineSkills + files + mcpServers (parity with managed)", () => {
    // Per AGENTS.md "No feature gates between runtimes" — native and
    // managed serve identical features. Skills/files via the Skills + Files
    // APIs; MCP via Anthropic session vaults whose static_bearer credentials
    // are keyed on mcp_server_url. This test is the tripwire that any
    // capability flip back to false is intentional and accompanied by an
    // AGENTS.md rule change.
    const cap = PROVIDER_CAPABILITY.anthropic.nativeAgent;
    expect(cap).not.toBeNull();
    expect(cap?.executor).toBe("anthropic-managed");
    expect(cap?.serves).toEqual({ inlineSkills: true, files: true, mcpServers: true });
  });
});

describe("native feature gating is capability-driven", () => {
  // Per AGENTS.md "No feature gates between runtimes" — native serves
  // inline skills, files, AND mcp servers. Skills/files via the
  // Skills + Files APIs; MCP via session vaults keyed on mcp_server_url.
  const agenticSubmissions: ReadonlyArray<{ label: string; mutate: (s: Record<string, unknown>) => Record<string, unknown> }> = [
    { label: "inline skill", mutate: (s) => ({ ...s, skills: [r2Ref("rules")] }) },
    { label: "file", mutate: (s) => ({ ...s, files: [r2Ref("data")] }) },
    {
      label: "mcp server",
      mutate: (s) => ({ ...s, mcpServers: [{ name: "gh", url: "https://example.com/mcp" }] })
    }
  ];

  it.each(agenticSubmissions)(
    "accepts $label on runtime: 'native' (native serves it)",
    ({ mutate }) => {
      const req = baseRequest({ provider: "anthropic", runtime: "native" });
      const parsed = parseRunSubmissionRequest({ ...req, submission: mutate(req.submission) });
      expect(selectRuntime(parsed)).toBe("native");
    }
  );

  it.each(agenticSubmissions)(
    "accepts $label under auto-routing (resolves to native)",
    ({ mutate }) => {
      const req = baseRequest({ provider: "anthropic" });
      const parsed = parseRunSubmissionRequest({ ...req, submission: mutate(req.submission) });
      expect(selectRuntime(parsed)).toBe("native");
    }
  );

  it.each(agenticSubmissions)("runs $label on runtime: 'managed' (Goose) without complaint", ({ mutate }) => {
    const req = baseRequest({ provider: "anthropic", runtime: "managed" });
    const parsed = parseRunSubmissionRequest({ ...req, submission: mutate(req.submission) });
    expect(selectRuntime(parsed)).toBe("managed");
  });

  it("collectNativeUnsupportedFeatures returns [] for a provider with no native runtime", () => {
    // deepseek has nativeAgent: null — the native gate never applies.
    const req = baseRequest({ provider: "deepseek", runtime: "managed" });
    const parsed = parseRunSubmissionRequest({ ...req, submission: { ...req.submission, files: [r2Ref("d")] } });
    expect(collectNativeUnsupportedFeatures(parsed)).toEqual([]);
  });

  it("collectNativeUnsupportedFeatures returns [] for anthropic (all features served on native)", () => {
    // Per AGENTS.md "No feature gates between runtimes" — none of
    // {inlineSkills, files, mcpServers} are flagged for anthropic; all
    // three are served by the native executor.
    const req = baseRequest({ provider: "anthropic" });
    const parsed = parseRunSubmissionRequest({
      ...req,
      submission: {
        ...req.submission,
        skills: [r2Ref("rules", 1)],
        files: [r2Ref("data", 2)],
        mcpServers: [{ name: "gh", url: "https://example.com/mcp" }]
      }
    });
    const flagged = collectNativeUnsupportedFeatures(parsed);
    expect(flagged).toEqual([]);
  });
});
