import { inspect } from "node:util";
import { describe, expect, it } from "vitest";
import { Aex, SessionConfigValidationError } from "../../src/index.js";

const COMPLEX_DIAGNOSTIC_FIELDS = new Set([
  "webhook.url",
  "responseFormat",
  "approvalGate",
  "environment.secrets",
  "fileCapture",
  "environment"
]);

interface Case {
  readonly label: string;
  readonly options: () => Record<string, unknown>;
  readonly field: string;
  readonly message: string;
  readonly rejectedTokens?: readonly string[];
  readonly usefulCause?: RegExp;
}

const base = (): Record<string, unknown> => ({
  model: "claude-haiku-4-5",
  apiKeys: { anthropic: "safe-placeholder" }
});

const cases: readonly Case[] = [
  {
    label: "unknown model",
    options: () => ({ ...base(), model: "unknown-model-q7" }),
    field: "model",
    message: "model must be recognized unless provider is supplied explicitly",
    rejectedTokens: ["unknown-model-q7"],
    usefulCause: /known model id|provider explicitly/
  },
  {
    label: "provider mismatch",
    options: () => ({ ...base(), model: "gpt-4.1", provider: "anthropic" }),
    field: "provider",
    message: "provider cannot serve the selected model",
    rejectedTokens: ["gpt-4.1", "anthropic"],
    usefulCause: /available/
  },
  {
    label: "non-streamable output mode",
    options: () => ({
      model: "gemini-2.5-flash",
      provider: "gemini",
      apiKeys: { gemini: "safe-placeholder" },
      outputMode: "stream"
    }),
    field: "outputMode",
    message: "outputMode is not supported for the selected provider",
    rejectedTokens: ["stream", "gemini"],
    usefulCause: /available for/
  },
  {
    label: "runtime size",
    options: () => ({ ...base(), runtime: { size: "runtime-size-q7" } }),
    field: "runtime.size",
    message: "runtime.size must be a supported size preset",
    rejectedTokens: ["runtime-size-q7"],
    usefulCause: /runtimeSize must be one of/
  },
  {
    label: "runtime kind",
    options: () => ({ ...base(), runtime: { kind: "runtime-kind-q7" } }),
    field: "runtime.kind",
    message: "runtime.kind must be one of: container, spot_container, lambda",
    rejectedTokens: ["runtime-kind-q7"],
    usefulCause: /runtimeKind must be one of/
  },
  {
    label: "timeout",
    options: () => ({ ...base(), overrides: { timeout: "10s" } }),
    field: "overrides.timeout",
    message: "overrides.timeout must be a supported duration",
    rejectedTokens: ["10s"],
    usefulCause: /at least 60000ms \(1m\)/
  },
  {
    label: "webhook",
    options: () => ({ ...base(), webhook: { url: "http://user:xy@example.test/path?token=q7" } }),
    field: "webhook.url",
    message: "webhook.url must be a valid HTTPS URL",
    rejectedTokens: ["user", "xy", "token=q7", "example.test"]
  },
  {
    label: "response format",
    options: () => ({
      ...base(),
      responseFormat: { kind: "json_schema", schema: { secret: "schema-secret-q7" }, strict: "yes" }
    }),
    field: "responseFormat",
    message: "responseFormat is invalid",
    rejectedTokens: ["schema-secret-q7"]
  },
  {
    label: "approval gate",
    options: () => ({ ...base(), approvalGate: { tools: ["bash", { secret: "gate-secret-q7" }] } }),
    field: "approvalGate",
    message: "approvalGate is invalid",
    rejectedTokens: ["gate-secret-q7"]
  },
  {
    label: "environment secrets",
    options: () => ({
      ...base(),
      environment: { secrets: { PRIVATE_TOKEN: "env-secret-q7" } }
    }),
    field: "environment.secrets",
    message: "environment.secrets is invalid",
    rejectedTokens: ["PRIVATE_TOKEN", "env-secret-q7"]
  },
  {
    label: "max spend",
    options: () => ({ ...base(), overrides: { maxSpendUsd: -1 } }),
    field: "overrides.maxSpendUsd",
    message: "overrides.maxSpendUsd must be valid",
    usefulCause: /positive finite number/
  },
  {
    label: "max turns",
    options: () => ({ ...base(), overrides: { maxTurns: 0 } }),
    field: "overrides.maxTurns",
    message: "overrides.maxTurns must be valid",
    usefulCause: /positive safe integer/
  },
  {
    label: "file capture",
    options: () => {
      const fileCapture = {} as Record<string, unknown>;
      Object.defineProperty(fileCapture, "allowedDirs", {
        enumerable: true,
        get: () => {
          throw new Error("file path C:/private/file-capture-q7");
        }
      });
      return { ...base(), fileCapture };
    },
    field: "fileCapture",
    message: "fileCapture is invalid",
    rejectedTokens: ["C:/private/file-capture-q7"]
  },
  {
    label: "environment",
    options: () => {
      let reads = 0;
      const environment = { networking: { mode: "none" } } as Record<string, unknown>;
      Object.defineProperty(environment, "variables", {
        enumerable: true,
        get: () => {
          reads += 1;
          if (reads > 1) throw new Error("environment path C:/private/environment-q7");
          return {};
        }
      });
      return { ...base(), environment };
    },
    field: "environment",
    message: "environment is invalid",
    rejectedTokens: ["C:/private/environment-q7"]
  },
  {
    label: "builtin tool",
    options: () => ({ ...base(), builtinTools: ["builtin-tool-q7"] }),
    field: "builtinTools",
    message: "builtinTools contains an unsupported tool name",
    rejectedTokens: ["builtin-tool-q7"],
    usefulCause: /expected one of/
  }
];

describe("aex.sessions.create validator diagnostics", () => {
  it.each(cases)("preserves the top-level $label error and adds only a safe cause", async (row) => {
    let fetchCalls = 0;
    const client = new Aex({
      apiKey: "safe-placeholder",
      baseUrl: "https://example.test",
      fetch: async () => {
        fetchCalls += 1;
        throw new Error("network must not be reached");
      }
    });
    const error = await client.sessions.create(row.options() as never).catch((caught: unknown) => caught);

    expect(error).toBeInstanceOf(SessionConfigValidationError);
    expect(error).toMatchObject({
      name: "SessionConfigValidationError",
      code: "SESSION_CONFIG_INVALID",
      message: `aex.sessions.create: ${row.message}`,
      details: { field: row.field }
    });
    expect(Object.isFrozen((error as SessionConfigValidationError).details)).toBe(false);
    expect(Object.keys(error as object)).toEqual(["code", "details", "name"]);
    expect(JSON.stringify(error)).toBe(JSON.stringify({
      code: "SESSION_CONFIG_INVALID",
      details: { field: row.field },
      name: "SessionConfigValidationError"
    }));
    expect(fetchCalls).toBe(0);

    const cause = (error as Error).cause;
    expect(cause).toBeInstanceOf(Error);
    expect(cause).toMatchObject({ name: "SessionConfigDiagnosticError" });
    expect(Object.keys(cause as object)).toEqual([]);
    expect((cause as Error).message.length).toBeLessThanOrEqual(512);
    if (COMPLEX_DIAGNOSTIC_FIELDS.has(row.field)) {
      expect((cause as Error).message).toBe(
        `underlying validator rejected ${row.field}; diagnostic redacted`
      );
    } else if (row.usefulCause) {
      expect((cause as Error).message).toMatch(row.usefulCause);
    }
    for (const token of row.rejectedTokens ?? []) {
      expect((cause as Error).message).not.toContain(token);
      expect(inspect(cause)).not.toContain(token);
    }
  });

  it.each([
    ["missing options", undefined, "options", "options are required"],
    ["missing model", {}, "model", "model is required"],
    ["unknown key", { ...base(), futureOption: true }, "futureOption", "futureOption is not a supported option"]
  ])("keeps direct $0 validation causeless", async (_label, options, field, message) => {
    const client = new Aex({ apiKey: "safe-placeholder", baseUrl: "https://example.test" });
    const error = await client.sessions.create(options as never).catch((caught: unknown) => caught);
    expect(error).toMatchObject({
      name: "SessionConfigValidationError",
      code: "SESSION_CONFIG_INVALID",
      message: `aex.sessions.create: ${message}`,
      details: { field }
    });
    expect((error as Error).cause).toBeUndefined();
  });
});
