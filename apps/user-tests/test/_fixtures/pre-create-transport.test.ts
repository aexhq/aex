import { describe, expect, it } from "vitest";
import { isPreCreateTransportFailure, isPreCreateTransportMessage } from "./pre-create-transport.js";

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
});
