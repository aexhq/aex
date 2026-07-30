import { describe, expect, it } from "bun:test";
import {
  WORKSPACE_API_KEY_FIELD_COUNT,
  WORKSPACE_API_KEY_REGION_CODES,
  WORKSPACE_API_KEY_REGIONS,
  WorkspaceApiKeyValueSchema,
  newId,
  parseApiKey
} from "../src/index.js";

const SECRET = Buffer.alloc(32, 0xa5).toString("base64url");

function workspaceKey(
  regionCode: (typeof WORKSPACE_API_KEY_REGION_CODES)[number],
  keyId = newId("apiKey"),
  secret = SECRET
): string {
  return `aex_wk_${regionCode}_${keyId.slice("key_".length)}_${secret}`;
}

describe("workspace API-key codec", () => {
  it("parses exactly the five accepted regions and reconstructs the indexed key id", () => {
    expect(WORKSPACE_API_KEY_REGION_CODES).toEqual([
      "use1",
      "use2",
      "usw2",
      "apne1",
      "euw1"
    ]);
    for (const regionCode of WORKSPACE_API_KEY_REGION_CODES) {
      const keyId = newId("apiKey");
      const value = workspaceKey(regionCode, keyId);
      expect(value.split("_")).toHaveLength(WORKSPACE_API_KEY_FIELD_COUNT);
      expect(parseApiKey(value)).toEqual({
        regionCode,
        region: WORKSPACE_API_KEY_REGIONS[regionCode],
        keyId
      });
      expect(WorkspaceApiKeyValueSchema.safeParse(value).success).toBe(true);
    }
  });

  it("accepts exactly 32 canonical unpadded base64url secret bytes", () => {
    expect(SECRET).toHaveLength(43);
    expect(parseApiKey(workspaceKey("euw1"))).not.toBeNull();
    for (const secret of [
      "A".repeat(42),
      "A".repeat(44),
      `${"A".repeat(42)}=`,
      `${"A".repeat(42)}B`,
      `${"A".repeat(42)}+`,
      `${"A".repeat(42)}/`
    ]) {
      expect(parseApiKey(workspaceKey("euw1", newId("apiKey"), secret))).toBeNull();
    }
  });

  it("rejects legacy plane, workspace, tag, checksum, and removed-region layouts", () => {
    const keyId = newId("apiKey");
    const suffix = keyId.slice("key_".length);
    for (const value of [
      `aex_prd_euw1_wsp_deadbeef_${SECRET}_tag`,
      `aex_dev_euw1_${suffix}_${SECRET}_tag`,
      `aex_wk_usw1_${suffix}_${SECRET}`,
      `aex_wk_euw1_wsp_deadbeef_${SECRET}`,
      `aex_wk_euw1_${suffix}_${SECRET}_crc`,
      `aex_wk_euw1_${"0".repeat(26)}_${SECRET}`,
      "sk-live-abc123",
      ""
    ]) {
      expect(parseApiKey(value)).toBeNull();
      expect(WorkspaceApiKeyValueSchema.safeParse(value).success).toBe(false);
    }
  });
});
