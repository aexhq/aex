import { describe, expect, it } from "bun:test";
import {
  HttpClient,
  SessionConfigValidationError,
  type SessionCreateRequest
} from "../src/index.js";
import { operations } from "../src/internal.js";

const createRequest: SessionCreateRequest = {
  provider: "deepseek",
  submission: {
    model: "deepseek-v4-flash",
    assets: { files: [], skills: [], tools: [], instructions: [] },
    builtinTools: "default",
    mcpServers: []
  },
  secrets: { apiKeys: { deepseek: "sk-test" } }
};

function noNetworkHttp(): { readonly http: HttpClient; readonly calls: { value: number } } {
  const calls = { value: 0 };
  return {
    calls,
    http: new HttpClient({
      apiKey: "test",
      baseUrl: "https://api.example.test",
      fetch: async () => {
        calls.value += 1;
        throw new Error("transport must not be reached");
      }
    })
  };
}

function expectConfigError(error: unknown, field: string): void {
  expect(error).toBeInstanceOf(SessionConfigValidationError);
  expect(error).toMatchObject({
    name: "SessionConfigValidationError",
    code: "SESSION_CONFIG_INVALID"
  });
  expect((error as SessionConfigValidationError).details).toEqual({ field });
  expect((error as Error).message.trim().length).toBeGreaterThan(0);
}

async function rejected(operation: () => Promise<unknown>): Promise<unknown> {
  return operation().then(
    () => undefined,
    (error: unknown) => error
  );
}

describe("operations config errors", () => {
  it.each(["", "   ", "sensitive-idempotency-key".repeat(20)])(
    "uses an exact field-only idempotency error",
    (key) => {
      let error: unknown;
      try {
        operations.resolveIdempotencyKey(key);
      } catch (caught) {
        error = caught;
      }
      expectConfigError(error, "idempotencyKey");
      expect((error as Error).message).not.toContain(key || "__empty__");
    }
  );

  it("rejects an invalid message before transport", async () => {
    const { http, calls } = noNetworkHttp();
    const error = await rejected(() => operations.createSessionWithMessage(http, createRequest, []));
    expectConfigError(error, "input");
    expect(calls.value).toBe(0);
  });

  it.each([
    [{ limit: 0 }, "limit"],
    [{ status: "sensitive-invalid-status" }, "status"],
    [{ since: "sensitive-invalid-date" }, "since"],
    [{ cursor: "" }, "cursor"]
  ] as const)("rejects an invalid sessions.list field before transport", async (query, field) => {
    const { http, calls } = noNetworkHttp();
    const error = await rejected(() => operations.listSessions(http, query as never));
    expectConfigError(error, field);
    expect(calls.value).toBe(0);
  });

  it("rejects an invalid event page size before transport", async () => {
    const { http, calls } = noNetworkHttp();
    const operation = async () => {
      for await (const _event of operations.iterateSessionEvents(http, "sensitive-session-id", { pageSize: 0 })) {
        // Validation must run before the first request.
      }
    };
    const error = await rejected(operation);
    expectConfigError(error, "pageSize");
    expect((error as Error).message).not.toContain("sensitive-session-id");
    expect(calls.value).toBe(0);
  });

  it.each([
    [
      "one file",
      (http: HttpClient) => operations.downloadSessionFile(
        http,
        "session-1",
        { id: "file-1", checkpointId: "cp_1" },
        { timeoutMs: 0 }
      )
    ],
    ["file archive", (http: HttpClient) => operations.downloadSessionFiles(http, "session-1", { timeoutMs: 0 })]
  ] as const)("rejects an invalid %s timeout before transport", async (_label, operation) => {
    const { http, calls } = noNetworkHttp();
    const error = await rejected(() => operation(http));
    expectConfigError(error, "timeoutMs");
    expect(calls.value).toBe(0);
  });

  it("rejects a billing-body idempotency key before transport", async () => {
    const { http, calls } = noNetworkHttp();
    const error = await rejected(() => operations.createBillingCheckout(
      http,
      { planKey: "pro", idempotencyKey: "sensitive-legacy-key" } as never
    ));
    expectConfigError(error, "idempotencyKey");
    expect((error as Error).message).not.toContain("sensitive-legacy-key");
    expect(calls.value).toBe(0);
  });
});
