import { describe, expect, it } from "vitest";
import { isPreCreateTransportFailure, isPreCreateTransportMessage, withPreCreateTransportRetry } from "./pre-create-transport.js";

describe("pre-create transport failure classifier", () => {
  it("matches Bun-style direct upload ConnectionRefused failures before a run is created", () => {
    const threw =
      "uploadAsset: direct upload PUT failed for https://aex-dev-eu-west-2-outputs-522921482290.s3.eu-west-2.amazonaws.com/assets/example?[redacted] after 3 attempts (ConnectionRefused): Unable to connect. Is the computer able to access the url?";

    expect(isPreCreateTransportFailure({ runId: null, threw })).toBe(true);
  });

  it("matches common nested transport spellings but requires no run id", () => {
    expect(isPreCreateTransportMessage("TypeError: fetch failed caused by UND_ERR_CONNECT_TIMEOUT")).toBe(true);
    expect(isPreCreateTransportMessage("socket connection was closed unexpectedly")).toBe(true);
    expect(isPreCreateTransportFailure({ runId: "run_0123456789abcdef0123456789abcdef", threw: "fetch failed" })).toBe(false);
  });

  it("does not classify product or assertion failures as pre-create transport gaps", () => {
    expect(isPreCreateTransportFailure({ runId: null, threw: "AssertionError: expected assistant text to contain token" })).toBe(false);
    expect(isPreCreateTransportFailure({ runId: null, threw: null })).toBe(false);
  });

  it("retries child-script pre-create transport failures only", async () => {
    let attempts = 0;
    const result = await withPreCreateTransportRetry(
      "unit",
      async () => {
        attempts += 1;
        if (attempts === 1) {
          throw new Error(
            "runner exited non-zero:\nerror: uploadAsset: direct upload PUT failed after 3 attempts (ConnectionRefused): Unable to connect"
          );
        }
        return "ok";
      },
      { sleepMs: () => 0 }
    );

    expect(result).toBe("ok");
    expect(attempts).toBe(2);
  });

  it("does not retry product assertions", async () => {
    let attempts = 0;
    await expect(
      withPreCreateTransportRetry(
        "unit",
        async () => {
          attempts += 1;
          throw new Error("AssertionError: expected assistant text to contain planted token");
        },
        { sleepMs: () => 0 }
      )
    ).rejects.toThrow(/planted token/);

    expect(attempts).toBe(1);
  });
});
