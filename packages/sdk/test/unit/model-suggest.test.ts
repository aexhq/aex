/** Unknown model failures expose a stable field without leaking input values. */
import { describe, expect, it } from "vitest";
import { Aex, SessionConfigValidationError } from "../../src/index.js";

let networkCalls = 0;
const noNetwork: typeof globalThis.fetch = async () => {
  networkCalls++;
  throw new Error("no network call should be made");
};

describe("SDK unknown model validation", () => {
  it("reports the model field without echoing a typo before any network", async () => {
    networkCalls = 0;
    const invalidModel = "deepseek-v4-flsh";
    const client = new Aex({ apiKey: "tk", baseUrl: "https://x", fetch: noNetwork });
    const err = await client
      .start({ model: invalidModel, message: "hi", apiKeys: { deepseek: "K" } } as never)
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(SessionConfigValidationError);
    expect(err).toMatchObject({ name: "SessionConfigValidationError", code: "SESSION_CONFIG_INVALID" });
    expect((err as SessionConfigValidationError).details).toEqual({ field: "model" });
    expect((err as Error).message.trim().length).toBeGreaterThan(0);
    expect((err as Error).message).not.toContain(invalidModel);
    expect(networkCalls).toBe(0);
  });
});
