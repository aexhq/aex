/**
 * DX4a: every `submit()` offline validation rejects with a TYPED, code-carrying
 * `AexError` subclass (not a bare `Error`), so callers can `catch` by `err.code`
 * / `instanceof RunConfigValidationError`. The thrown MESSAGES stay byte-identical
 * to the public validation contract asserted alongside the existing submit-test
 * regexes.
 */
import { describe, expect, it, vi } from "vitest";
import {
  AexError,
  AgentExecutor,
  RunConfigValidationError
} from "../../src/index.js";

/** A fetch that NEVER resolves a network call — every assertion below must fail
 *  offline, before any request leaves the SDK. */
function noNetworkFetch(): { fetch: typeof fetch; calls: number } {
  const state = { calls: 0 };
  const stub: typeof fetch = vi.fn(async () => {
    state.calls++;
    throw new Error("no network call should be made for an invalid submission");
  });
  return {
    fetch: stub,
    get calls() {
      return state.calls;
    }
  };
}

function makeClient(fetchImpl: typeof fetch): AgentExecutor {
  return new AgentExecutor({ apiToken: "tkn_test", baseUrl: "https://example.test", fetch: fetchImpl });
}

describe("AgentExecutor.submit — typed RunConfigValidationError (DX4a)", () => {
  it("throws RunConfigValidationError with code RUN_CONFIG_INVALID for a missing options object", async () => {
    const { fetch, calls } = noNetworkFetch();
    const client = makeClient(fetch);
    await expect(
      // deliberately pass an invalid value
      (client.submit as unknown as (o: unknown) => Promise<string>)(undefined)
    ).rejects.toMatchObject({
      name: "RunConfigValidationError",
      code: "RUN_CONFIG_INVALID",
      message: "AgentExecutor.submit: options is required"
    });
    expect(calls).toBe(0);
  });

  it("is both an AexError and a RunConfigValidationError (instanceof reliable)", async () => {
    const { fetch } = noNetworkFetch();
    const client = makeClient(fetch);
    let caught: unknown;
    try {
      await client.submit({ model: "claude-haiku-4-5", prompt: "" });
    } catch (err) {
      caught = err;
    }
    expect(caught).toBeInstanceOf(RunConfigValidationError);
    expect(caught).toBeInstanceOf(AexError);
    expect(caught).toBeInstanceOf(Error);
    expect((caught as AexError).code).toBe("RUN_CONFIG_INVALID");
  });

  it("rejects an empty prompt with the unchanged message + code", async () => {
    const { fetch, calls } = noNetworkFetch();
    const client = makeClient(fetch);
    await expect(
      client.submit({ model: "claude-haiku-4-5", prompt: "" })
    ).rejects.toMatchObject({
      code: "RUN_CONFIG_INVALID",
      message: "AgentExecutor.submit: prompt must be a non-empty string"
    });
    expect(calls).toBe(0);
  });

  it("rejects a missing provider API key with the unchanged message + code", async () => {
    const { fetch } = noNetworkFetch();
    const client = makeClient(fetch);
    await expect(
      client.submit({ model: "claude-haiku-4-5", prompt: "hi" })
    ).rejects.toMatchObject({ code: "RUN_CONFIG_INVALID" });
    // The historic regex (asserted in client.submit.test.ts) still matches.
    await expect(
      client.submit({ model: "claude-haiku-4-5", prompt: "hi" })
    ).rejects.toThrow(/AgentExecutor\.submit: a provider API key is required/);
  });

  it("rejects a provider that does not serve the model with code RUN_CONFIG_INVALID", async () => {
    const { fetch, calls } = noNetworkFetch();
    const client = makeClient(fetch);
    await expect(
      client.submit({
        model: "gpt-4.1",
        prompt: "hi",
        provider: "anthropic",
        secrets: { apiKeys: { anthropic: "sk-x" } }
      })
    ).rejects.toMatchObject({
      name: "RunConfigValidationError",
      code: "RUN_CONFIG_INVALID"
    });
    expect(calls).toBe(0);
  });

  it("rejects a non-Tool / non-builtin tools entry with code RUN_CONFIG_INVALID", async () => {
    const { fetch, calls } = noNetworkFetch();
    const client = makeClient(fetch);
    await expect(
      client.submit({
        model: "claude-haiku-4-5",
        prompt: "hi",
        secrets: { apiKeys: { anthropic: "sk-x" } },
        // not a builtin tool name
        tools: ["definitely_not_a_builtin"] as unknown as never
      })
    ).rejects.toMatchObject({ code: "RUN_CONFIG_INVALID" });
    expect(calls).toBe(0);
  });
});
