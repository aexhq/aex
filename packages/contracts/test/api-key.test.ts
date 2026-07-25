/**
 * WS11: the self-describing API-key codec. parseApiKey round-trips dev/prd ×
 * region, rejects a wrong CRC / part-count / plane / region, and returns null
 * for opaque strings — so the SDK constructor can plane-guard with zero network.
 *
 * Every workspace-id fixture is minted with `newId("workspace")`. A hand-written
 * id keeps passing after the format changes and hides the break.
 */
import { describe, expect, it } from "bun:test";
import {
  API_KEY_FIELD_COUNT,
  API_KEY_PLANES,
  API_KEY_REGION_TO_CODE,
  formatApiKey,
  idPatternSource,
  newId,
  parseApiKey
} from "../src/index.js";

const SECRET = "deadbeef".repeat(6);

describe("parseApiKey / formatApiKey codec (WS11)", () => {
  const regions = Object.keys(API_KEY_REGION_TO_CODE);

  it("covers all three launch regions", () => {
    expect(regions.sort()).toEqual(["ap-northeast-1", "eu-west-1", "us-west-1"]);
  });

  for (const plane of API_KEY_PLANES) {
    for (const region of regions) {
      it(`round-trips ${plane} / ${region} with the workspace id byte-identical`, () => {
        const workspaceId = newId("workspace");
        const key = formatApiKey({ plane, region, workspaceId, secret: SECRET });
        expect(key.split("_")).toHaveLength(API_KEY_FIELD_COUNT);
        expect(key).toContain(workspaceId);
        expect(parseApiKey(key)).toEqual({
          plane,
          regionCode: API_KEY_REGION_TO_CODE[region]!,
          region,
          workspaceId
        });
      });
    }
  }

  it("embeds the whole `wsp_<hex>` id — the key is 7 fields, not 6", () => {
    const workspaceId = newId("workspace");
    const key = formatApiKey({ plane: "prd", region: "eu-west-1", workspaceId, secret: SECRET });
    const parts = key.split("_");
    expect(parts.slice(3, 5).join("_")).toBe(workspaceId);
    expect(key.includes("-")).toBe(false);
  });

  it("refuses to normalize a non-canonical workspace id — no silent coercion", () => {
    for (const dead of [
      "ws-abc-123",
      "5fc4b90e-55af-46cf-9938-b70f988e431d",
      "5fc4b90e55af46cf9938b70f988e431d",
      "wsp_example",
      "ws1"
    ]) {
      expect(() => formatApiKey({ plane: "prd", region: "eu-west-1", workspaceId: dead, secret: SECRET })).toThrow(
        `API key workspaceId must match ${idPatternSource("workspace")}`
      );
    }
  });

  it("returns null for opaque / legacy / malformed strings", () => {
    expect(parseApiKey("sk-live-abc123")).toBeNull();
    expect(parseApiKey("")).toBeNull();
    expect(parseApiKey("aex_prd_euw1_ws1_secret")).toBeNull(); // 5 fields
    // The PRE-cut 6-field layout, with the prefix stripped off the workspace id.
    expect(parseApiKey(`aex_prd_euw1_5fc4b90e55af46cf9938b70f988e431d_${SECRET}_tag`)).toBeNull();
    // 7 fields, but the embedded workspace id is not a canonical id.
    expect(parseApiKey(`aex_prd_euw1_wsp_nothex_${SECRET}_tag`)).toBeNull();
  });

  it("WS6: a tag/secret tamper still PARSES (structure only) — authenticity is server-verified", () => {
    // The trailing tag is an HMAC keyed by the server pepper, which the SDK does not hold, so the
    // SDK parse is STRUCTURAL. A tampered secret is structurally valid → it routes; the server's
    // verifyTokenTag rejects it at auth. Only a broken SHAPE is null here.
    const key = formatApiKey({ plane: "prd", region: "eu-west-1", workspaceId: newId("workspace"), secret: "aaaa" });
    const parts = key.split("_");
    parts[5] = "bbbb";
    expect(parseApiKey(parts.join("_"))).not.toBeNull();
    expect(parseApiKey("aex_prd_euw1_ws1_secret")).toBeNull(); // a broken SHAPE is still rejected
  });

  it("rejects an unknown plane", () => {
    const body = ["aex", "staging", "euw1", newId("workspace"), "secret"].join("_");
    expect(parseApiKey(`${body}_zzz`)).toBeNull();
  });

  it("rejects an unknown region code", () => {
    const key = formatApiKey({ plane: "prd", region: "eu-west-1", workspaceId: newId("workspace"), secret: "aaaa" });
    const parts = key.split("_");
    parts[2] = "zzzz";
    expect(parseApiKey(parts.join("_"))).toBeNull();
  });
});
