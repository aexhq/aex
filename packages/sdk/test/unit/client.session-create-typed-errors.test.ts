/**
 * DX4a: every `sessions.create` / `Aex.start` offline validation rejects with a typed,
 * code-carrying `AexError` subclass (not a bare `Error`), so callers can `catch`
 * by `err.code` / `instanceof SessionConfigValidationError`.
 */
import { describe, expect, it, mock } from "bun:test";
import type { FetchLike } from "@aexhq/contracts";
import type { ModelName } from "@aexhq/contracts";
import { unvalidatedCreateOptions } from "../helpers/unvalidated.js";
import {
  AexError,
  Aex,
  SessionConfigValidationError
} from "../../src/index.js";

/** A fetch that NEVER resolves a network call — every assertion below must fail
 *  offline, before any request leaves the SDK. */
function noNetworkFetch(): { fetch: FetchLike; calls: number } {
  const state = { calls: 0 };
  const stub: FetchLike = mock(async () => {
    state.calls++;
    throw new Error("no network call should be made for an invalid session");
  });
  return {
    fetch: stub,
    get calls() {
      return state.calls;
    }
  };
}

function makeClient(fetchImpl: FetchLike): Aex {
  return new Aex({ apiKey: "tkn_test", baseUrl: "https://example.test", fetch: fetchImpl });
}

async function captureRejected(operation: () => Promise<unknown>): Promise<unknown> {
  return operation().then(
    () => undefined,
    (error: unknown) => error
  );
}

function expectConfigError(error: unknown, field: string): void {
  expect(error).toBeInstanceOf(SessionConfigValidationError);
  expect(error).toBeInstanceOf(AexError);
  expect(error).toMatchObject({
    name: "SessionConfigValidationError",
    code: "SESSION_CONFIG_INVALID"
  });
  expect((error as SessionConfigValidationError).details).toEqual({ field });
  expect((error as Error).message.trim().length).toBeGreaterThan(0);
}

const unknownModel = "totally-unknown-model-xyz" as unknown as ModelName;

describe("aex.sessions.create — typed SessionConfigValidationError (DX4a)", () => {
  it("throws SessionConfigValidationError with code SESSION_CONFIG_INVALID for a missing options object", async () => {
    const { fetch, calls } = noNetworkFetch();
    const client = makeClient(fetch);
    const error = await captureRejected(() =>
      // deliberately pass an invalid value
      (client.sessions.create as unknown as (o: unknown) => Promise<unknown>)(undefined)
    );
    expectConfigError(error, "options");
    expect(calls).toBe(0);
  });

  it("is both an AexError and a SessionConfigValidationError (instanceof reliable)", async () => {
    const { fetch } = noNetworkFetch();
    const client = makeClient(fetch);
    let caught: unknown;
    try {
      await client.sessions.create({
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "" }
      });
    } catch (err) {
      caught = err;
    }
    expect(caught).toBeInstanceOf(SessionConfigValidationError);
    expect(caught).toBeInstanceOf(AexError);
    expect(caught).toBeInstanceOf(Error);
    expect((caught as AexError).code).toBe("SESSION_CONFIG_INVALID");
    expect((caught as SessionConfigValidationError).details).toEqual({ field: "apiKeys.anthropic" });
  });

  it("rejects an empty one-shot message with stable error metadata", async () => {
    const { fetch, calls } = noNetworkFetch();
    const client = makeClient(fetch);
    const error = await captureRejected(() =>
      client.start({
        model: "claude-haiku-4-5",
        message: "",
        apiKeys: { anthropic: "sk-x" }
      })
    );
    expectConfigError(error, "message");
    expect(calls).toBe(0);
  });

  it("rejects a missing provider API key with its provider-specific field", async () => {
    const { fetch } = noNetworkFetch();
    const client = makeClient(fetch);
    const error = await captureRejected(() => client.sessions.create({ model: "claude-haiku-4-5" }));
    expectConfigError(error, "apiKeys.anthropic");
  });

  it("identifies an unknown model without echoing its value", async () => {
    const { fetch, calls } = noNetworkFetch();
    const client = makeClient(fetch);
    // Unknown model + no provider + a key for a real provider: the old
    // behavior fell back to the default provider and complained about a
    // missing apiKeys["anthropic"], pointing at the wrong problem.
    const error = await captureRejected(() => client.sessions.create({
        model: unknownModel,
        apiKeys: { deepseek: "sk-x" }
      }));
    expectConfigError(error, "model");
    expect((error as Error).message).not.toContain(unknownModel);
    expect(calls).toBe(0);
  });

  it("still forwards an unknown model when the caller names the provider explicitly (forward-compat)", async () => {
    const { fetch } = noNetworkFetch();
    const client = makeClient(fetch);
    // Explicit provider + key: client-side validation must NOT hard-reject the
    // unknown model (server owns that) — the request reaches the fetch stub.
    await expect(
      client.sessions.create({
        model: unknownModel,
        provider: "deepseek",
        apiKeys: { deepseek: "sk-x" }
      })
    ).rejects.toThrow(/no network call should be made/);
  });

  it("rejects a provider that does not serve the model with code SESSION_CONFIG_INVALID", async () => {
    const { fetch, calls } = noNetworkFetch();
    const client = makeClient(fetch);
    const error = await captureRejected(() => client.sessions.create({
        model: "gpt-4.1",
        provider: "anthropic",
        apiKeys: { anthropic: "sk-x" }
      }));
    expectConfigError(error, "provider");
    expect(calls).toBe(0);
  });

  it("rejects a non-Tool / non-builtin tools entry with code SESSION_CONFIG_INVALID", async () => {
    const { fetch, calls } = noNetworkFetch();
    const client = makeClient(fetch);
    const error = await captureRejected(() => client.sessions.create(unvalidatedCreateOptions({
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "sk-x" },
        // not a builtin tool name
        tools: ["definitely_not_a_builtin"]
      }))
    );
    expectConfigError(error, "tools");
    expect(calls).toBe(0);
  });
});
