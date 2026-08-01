import { describe, expect, test } from "bun:test";

import {
  AccountToken,
  AexConfigError,
  WorkspaceApiKey,
  resolveRegionalBaseUrl,
} from "../../src/index.js";

const UUID7_BODY = "0100000000e008000000000000";
const SECRET = `${"A".repeat(42)}A`;

describe("credential parsing and routing", () => {
  test.each([
    ["use1", "https://us-east-1.api.aex.dev"],
    ["use2", "https://us-east-2.api.aex.dev"],
    ["usw2", "https://us-west-2.api.aex.dev"],
    ["apne1", "https://ap-northeast-1.api.aex.dev"],
    ["euw1", "https://eu-west-1.api.aex.dev"],
  ] as const)("routes %s to its pinned regional host", (code, host) => {
    const key = WorkspaceApiKey.parse(`aex_wk_${code}_${UUID7_BODY}_${SECRET}`);
    expect(key.regionCode()).toBe(code);
    expect(key.regionalBaseUrl()).toBe(host);
    expect(key.redacted()).not.toContain(SECRET);
    expect(JSON.stringify(key)).not.toContain(SECRET);
  });

  test("rejects malformed credentials before any I/O", () => {
    expect(() => WorkspaceApiKey.parse("aex_wk_euw1_bad_secret")).toThrow(AexConfigError);
    expect(() => AccountToken.parse("aex_at_bad_secret")).toThrow(AexConfigError);
    expect(() => WorkspaceApiKey.parse(`aex_wk_euw1_${"0".repeat(26)}_${SECRET}`)).toThrow(
      AexConfigError,
    );
  });

  test("an account token requires an explicit regional binding", () => {
    const token = AccountToken.parse(`aex_at_${UUID7_BODY}_${SECRET}`);
    expect(() => resolveRegionalBaseUrl(token, {})).toThrow(AexConfigError);
    expect(resolveRegionalBaseUrl(token, { regionalBaseUrl: "https://regional.example" })).toBe(
      "https://regional.example",
    );
    expect(() => resolveRegionalBaseUrl(token, { regionalBaseUrl: "http://regional.example" })).toThrow(
      AexConfigError,
    );
  });
});
