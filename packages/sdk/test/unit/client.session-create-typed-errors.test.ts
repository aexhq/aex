/**
 * DX4a: every `sessions.create` / `Aex.start` offline validation rejects with a typed,
 * code-carrying `AexError` subclass (not a bare `Error`), so callers can `catch`
 * by `err.code` / `instanceof SessionConfigValidationError`.
 */
import { describe, expect, it, vi } from "vitest";
import type { ModelName } from "@aexhq/contracts";
import {
  AexError,
  Aex,
  SessionConfigValidationError
} from "../../src/index.js";

/** A fetch that NEVER resolves a network call — every assertion below must fail
 *  offline, before any request leaves the SDK. */
function noNetworkFetch(): { fetch: typeof fetch; calls: number } {
  const state = { calls: 0 };
  const stub: typeof fetch = vi.fn(async () => {
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

function makeClient(fetchImpl: typeof fetch): Aex {
  return new Aex({ apiKey: "tkn_test", baseUrl: "https://example.test", fetch: fetchImpl });
}

const unknownModel = "totally-unknown-model-xyz" as unknown as ModelName;

describe("aex.sessions.create — typed SessionConfigValidationError (DX4a)", () => {
  it("throws SessionConfigValidationError with code SESSION_CONFIG_INVALID for a missing options object", async () => {
    const { fetch, calls } = noNetworkFetch();
    const client = makeClient(fetch);
    await expect(
      // deliberately pass an invalid value
      (client.sessions.create as unknown as (o: unknown) => Promise<unknown>)(undefined)
    ).rejects.toMatchObject({
      name: "SessionConfigValidationError",
      code: "SESSION_CONFIG_INVALID",
      message: "aex.sessions.create: options is required"
    });
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
  });

  it("rejects an empty one-shot message with the unchanged message + code", async () => {
    const { fetch, calls } = noNetworkFetch();
    const client = makeClient(fetch);
    await expect(
      client.start({
        model: "claude-haiku-4-5",
        message: "",
        apiKeys: { anthropic: "sk-x" }
      })
    ).rejects.toMatchObject({
      code: "SESSION_CONFIG_INVALID",
      message: "Aex.start: message must be a non-empty string"
    });
    expect(calls).toBe(0);
  });

  it("rejects a missing provider API key with the unchanged message + code", async () => {
    const { fetch } = noNetworkFetch();
    const client = makeClient(fetch);
    await expect(
      client.sessions.create({ model: "claude-haiku-4-5" })
    ).rejects.toMatchObject({ code: "SESSION_CONFIG_INVALID" });
    await expect(
      client.sessions.create({ model: "claude-haiku-4-5" })
    ).rejects.toThrow(/aex\.sessions\.create: a provider API key is required/);
  });

  it("names the unknown model (not a missing default-provider key) when provider cannot be inferred", async () => {
    const { fetch, calls } = noNetworkFetch();
    const client = makeClient(fetch);
    // Unknown model + no provider + a key for a real provider: the old
    // behavior fell back to the default provider and complained about a
    // missing apiKeys["anthropic"], pointing at the wrong problem.
    await expect(
      client.sessions.create({
        model: unknownModel,
        apiKeys: { deepseek: "sk-x" }
      })
    ).rejects.toMatchObject({
      name: "SessionConfigValidationError",
      code: "SESSION_CONFIG_INVALID",
      message: expect.stringMatching(/"totally-unknown-model-xyz" is not a known model id.*pass provider explicitly/)
    });
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
    await expect(
      client.sessions.create({
        model: "gpt-4.1",
        provider: "anthropic",
        apiKeys: { anthropic: "sk-x" }
      })
    ).rejects.toMatchObject({
      name: "SessionConfigValidationError",
      code: "SESSION_CONFIG_INVALID"
    });
    expect(calls).toBe(0);
  });

  it("rejects a non-Tool / non-builtin tools entry with code SESSION_CONFIG_INVALID", async () => {
    const { fetch, calls } = noNetworkFetch();
    const client = makeClient(fetch);
    await expect(
      client.sessions.create({
        model: "claude-haiku-4-5",
        apiKeys: { anthropic: "sk-x" },
        // not a builtin tool name
        tools: ["definitely_not_a_builtin"] as unknown as never
      } as never)
    ).rejects.toMatchObject({ code: "SESSION_CONFIG_INVALID" });
    expect(calls).toBe(0);
  });
});
