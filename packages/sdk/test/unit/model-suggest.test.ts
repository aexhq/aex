/**
 * WS5/T6g — the SDK's unknown-model error carries the shared `did you mean?`
 * suggestion (via `resolveModelProvider`), zero-network.
 */
import { describe, expect, it } from "vitest";
import { Aex, SessionConfigValidationError } from "../../src/index.js";

const noNetwork: typeof globalThis.fetch = async () => {
  throw new Error("no network call should be made");
};

describe("SDK model did-you-mean (WS5)", () => {
  it("a typo'd known model throws with a 'did you mean' hint before any network", async () => {
    const client = new Aex({ apiKey: "tk", baseUrl: "https://x", fetch: noNetwork });
    const err = await client
      .start({ model: "deepseek-v4-flsh", message: "hi", apiKeys: { deepseek: "K" } } as never)
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(SessionConfigValidationError);
    expect((err as Error).message).toMatch(/did you mean "deepseek-v4-flash"/);
    expect((err as SessionConfigValidationError).details).toMatchObject({ field: "model" });
  });
});
