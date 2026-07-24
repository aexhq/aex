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

// A malformed model id: not a `creator/model` gateway slug (no creator prefix).
const malformedModel = "totally-unknown-model-xyz";
// A well-formed but unknown-to-this-client gateway slug — forward-compat: the
// SDK does not gate against a catalog, so this reaches the server.
const unknownButValidSlug = "newvendor/brand-new-model-2030" as unknown as ModelName;

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
        model: malformedModel as unknown as ModelName,
      });
    } catch (err) {
      caught = err;
    }
    expect(caught).toBeInstanceOf(SessionConfigValidationError);
    expect(caught).toBeInstanceOf(AexError);
    expect(caught).toBeInstanceOf(Error);
    expect((caught as AexError).code).toBe("SESSION_CONFIG_INVALID");
    expect((caught as SessionConfigValidationError).details).toEqual({ field: "model" });
  });

  it("rejects an empty one-shot message with stable error metadata", async () => {
    const { fetch, calls } = noNetworkFetch();
    const client = makeClient(fetch);
    const error = await captureRejected(() =>
      client.start({
        model: "anthropic/claude-haiku-4-5",
        message: "",
      })
    );
    expectConfigError(error, "message");
    expect(calls).toBe(0);
  });

  it("identifies a malformed model slug without echoing its value", async () => {
    const { fetch, calls } = noNetworkFetch();
    const client = makeClient(fetch);
    // A model id that is not a `creator/model` slug is rejected at the boundary,
    // and the rejected value is redacted out of the diagnostic.
    const error = await captureRejected(() => client.sessions.create({
        model: malformedModel as unknown as ModelName,
      }));
    expectConfigError(error, "model");
    expect((error as Error).message).not.toContain(malformedModel);
    expect(calls).toBe(0);
  });

  it("forwards a well-formed but unknown model slug to the server (forward-compat)", async () => {
    const { fetch } = noNetworkFetch();
    const client = makeClient(fetch);
    // The SDK validates only the slug SHAPE, not a catalog — a well-formed slug
    // this client doesn't recognize must NOT be hard-rejected (the gateway owns
    // that), so the request reaches the fetch stub.
    await expect(
      client.sessions.create({
        model: unknownButValidSlug,
      })
    ).rejects.toThrow(/no network call should be made/);
  });

  it("rejects a non-Tool / non-builtin tools entry with code SESSION_CONFIG_INVALID", async () => {
    const { fetch, calls } = noNetworkFetch();
    const client = makeClient(fetch);
    const error = await captureRejected(() => client.sessions.create(unvalidatedCreateOptions({
        model: "anthropic/claude-haiku-4-5",
        // not a builtin tool name
        tools: ["definitely_not_a_builtin"]
      }))
    );
    expectConfigError(error, "tools");
    expect(calls).toBe(0);
  });
});
