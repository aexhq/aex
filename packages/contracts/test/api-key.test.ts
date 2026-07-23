/**
 * WS11: the self-describing API-key codec. parseApiKey round-trips dev/prd ×
 * region, rejects a wrong CRC / part-count / plane / region, and returns null
 * for opaque strings — so the SDK constructor can plane-guard with zero network.
 */
import { describe, expect, it } from "bun:test";
import { parseApiKey, formatApiKey, API_KEY_REGION_TO_CODE, API_KEY_PLANES } from "../src/index.js";

describe("parseApiKey / formatApiKey codec (WS11)", () => {
  const regions = Object.keys(API_KEY_REGION_TO_CODE);

  for (const plane of API_KEY_PLANES) {
    for (const region of regions) {
      it(`round-trips ${plane} / ${region} (dash-normalizing the workspace id)`, () => {
        const key = formatApiKey({ plane, region, workspaceId: "ws-abc-123", secret: "deadbeef".repeat(6) });
        expect(parseApiKey(key)).toEqual({
          plane,
          regionCode: API_KEY_REGION_TO_CODE[region]!,
          region,
          workspaceId: "wsabc123"
        });
      });
    }
  }

  it("embeds public wsp_ workspace ids as the dash-free hex token field", () => {
    const workspaceId = "wsp_5fc4b90e55af46cf9938b70f988e431d";
    const key = formatApiKey({ plane: "prd", region: "eu-west-1", workspaceId, secret: "deadbeef".repeat(6) });
    expect(parseApiKey(key)?.workspaceId).toBe("5fc4b90e55af46cf9938b70f988e431d");
    expect(key.includes("-")).toBe(false);
  });

  it("returns null for opaque / legacy / malformed strings", () => {
    expect(parseApiKey("sk-live-abc123")).toBeNull();
    expect(parseApiKey("")).toBeNull();
    expect(parseApiKey("aex_prd_euw1_ws1_secret")).toBeNull(); // 5 parts
  });

  it("WS6: a tag/secret tamper still PARSES (structure only) — authenticity is server-verified", () => {
    // The trailing tag is an HMAC keyed by the server pepper, which the SDK does not hold, so the
    // SDK parse is STRUCTURAL. A tampered secret is structurally valid → it routes; the server's
    // verifyTokenTag rejects it at auth. Only a broken SHAPE is null here.
    const key = formatApiKey({ plane: "prd", region: "eu-west-1", workspaceId: "ws1", secret: "aaaa" });
    const parts = key.split("_");
    parts[4] = "bbbb";
    expect(parseApiKey(parts.join("_"))).not.toBeNull();
    expect(parseApiKey("aex_prd_euw1_ws1_secret")).toBeNull(); // a broken SHAPE is still rejected
  });

  it("rejects an unknown plane", () => {
    const body = ["aex", "staging", "euw1", "ws1", "secret"].join("_");
    expect(parseApiKey(`${body}_zzz`)).toBeNull();
  });

  it("rejects an unknown region code", () => {
    const key = formatApiKey({ plane: "prd", region: "eu-west-1", workspaceId: "ws1", secret: "aaaa" });
    const parts = key.split("_");
    parts[2] = "zzzz";
    expect(parseApiKey(parts.join("_"))).toBeNull();
  });
});
