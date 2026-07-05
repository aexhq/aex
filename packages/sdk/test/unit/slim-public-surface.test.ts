import { describe, expect, it } from "vitest";

const removedRootExports = [
  "AgentExecutor",
  "createDataTools",
  "createCorpusTools",
  "DataTools",
  "DataToolError",
  "DATA_TOOLS_INSTRUCTIONS",
  "ProxyEndpoint",
  "decodeAssistantText",
  "decodeToolCalls",
  "summarizeRunTrace",
  "summarizeRunUsage",
  "textOf"
] as const;

// The free-function event guards were replaced by `event.is*()` methods on the
// `AexEventView` the SDK yields, so none of them are on the public surface.
const removedEventGuards = [
  "isCustom",
  "isEventChannel",
  "isFromSource",
  "isLog",
  "isRunError",
  "isRunFinished",
  "isRunSettled",
  "isRunStarted",
  "isRunTerminal",
  "isTextMessage",
  "isToolCallResult",
  "isToolCallStart"
] as const;

describe("slim launch root SDK surface", () => {
  it("centers the public runtime surface on Aex", async () => {
    const sdk = await import("../../src/index.js");
    const root = sdk as Record<string, unknown>;

    expect(typeof root["Aex"]).toBe("function");
    for (const name of removedRootExports) {
      expect(root[name], `${name} should not be exported from the root SDK surface`).toBeUndefined();
    }
    for (const name of removedEventGuards) {
      expect(root[name], `${name} should be a method on the event, not a root SDK export`).toBeUndefined();
    }
  });

  it("constructs Aex with an API key string and optional client options", async () => {
    const { Aex } = await import("../../src/index.js");
    const AexCtor = Aex as unknown as {
      new (apiKey: string, options?: { readonly baseUrl?: string; readonly fetch?: typeof fetch }): unknown;
    };
    const fetchFake: typeof fetch = async () =>
      new Response("{}", {
        headers: { "content-type": "application/json" }
      });

    expect(() => new AexCtor("aex_unit_surface")).not.toThrow();
    expect(() => new AexCtor("aex_unit_surface", { baseUrl: "https://example.invalid", fetch: fetchFake })).not.toThrow();
  });
});
