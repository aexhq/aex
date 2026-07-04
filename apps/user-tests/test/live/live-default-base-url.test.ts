/**
 * Guard: the SDK's DEFAULT base URL must actually serve the hosted API.
 *
 * Every other live test injects AEX_API_URL, so the canonical default
 * (`https://api.aex.dev`, AEX_DEFAULT_BASE_URL in @aexhq/contracts) was never
 * exercised over the network — a missing DNS record shipped undetected and
 * every out-of-the-box `new Aex({ apiToken })` failed with ENOTFOUND
 * (root-caused 2026-07-04). This test deliberately IGNORES AEX_API_URL and
 * hits the default hostname directly: it proves DNS + TLS + routing, not auth,
 * so it passes with any plane's credentials.
 *
 * The URL is intentionally hardcoded: it IS the public contract under test.
 * If AEX_DEFAULT_BASE_URL ever changes, update this test in the same commit.
 */
import { describe, expect, it } from "vitest";

const DEFAULT_BASE_URL = "https://api.aex.dev";

describe("SDK default base URL serves the hosted API", () => {
  it("GET /api/whoami on the DEFAULT hostname reaches the API (no DNS/TLS failure)", async () => {
    let response: Response;
    try {
      response = await fetch(`${DEFAULT_BASE_URL}/api/whoami`, {
        redirect: "manual",
        signal: AbortSignal.timeout(30_000)
      });
    } catch (cause) {
      throw new Error(
        `The SDK default base URL ${DEFAULT_BASE_URL} is unreachable at the network layer ` +
          `(DNS/TLS/connect). Out-of-the-box SDK/CLI clients are broken for every user who ` +
          `does not override baseUrl. Cause: ${String(cause)}`,
        { cause }
      );
    }
    // Unauthenticated whoami must be ROUTED (200/401/403), never a 5xx or a
    // gateway placeholder — this asserts the hostname fronts the real API.
    expect([200, 401, 403]).toContain(response.status);
  });
});
