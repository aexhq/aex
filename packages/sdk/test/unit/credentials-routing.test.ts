import { describe, expect, test } from "bun:test";

import {
  AccountToken,
  AexConfigError,
  WorkspaceApiKey,
  resolveRegionalBaseUrl,
} from "../../src/index.js";

const UUID7_BODY = "0100000000e008000000000000";
const WORKSPACE_BODY = "0100000000e008000000000001";
const SECRET = `${"A".repeat(42)}A`;

describe("credential parsing and routing", () => {
  test.each([
    ["use1", "https://us-east-1.api.aex.dev"],
    ["use2", "https://us-east-2.api.aex.dev"],
    ["usw2", "https://us-west-2.api.aex.dev"],
    ["apne1", "https://ap-northeast-1.api.aex.dev"],
    ["euw1", "https://eu-west-1.api.aex.dev"],
  ] as const)("routes %s to its pinned regional host", (code, host) => {
    const key = WorkspaceApiKey.parse(`aex_wk_${code}_${WORKSPACE_BODY}_${UUID7_BODY}_${SECRET}`);
    expect(key.regionCode()).toBe(code);
    expect(key.workspaceId()).toBe(`wsp_${WORKSPACE_BODY}`);
    expect(key.regionalBaseUrl()).toBe(host);
    expect(key.redacted()).not.toContain(SECRET);
    expect(JSON.stringify(key)).not.toContain(SECRET);
  });

  test("a secret containing a separator is not read as a segment boundary", () => {
    const separated = `${"_".repeat(42)}A`;
    const key = WorkspaceApiKey.parse(
      `aex_wk_euw1_${WORKSPACE_BODY}_${UUID7_BODY}_${separated}`,
    );
    expect(key.workspaceId()).toBe(`wsp_${WORKSPACE_BODY}`);
  });

  test("rejects malformed credentials before any I/O", () => {
    expect(() => WorkspaceApiKey.parse("aex_wk_euw1_bad_secret")).toThrow(AexConfigError);
    expect(() => AccountToken.parse("aex_at_bad_secret")).toThrow(AexConfigError);
    expect(() =>
      WorkspaceApiKey.parse(`aex_wk_euw1_${WORKSPACE_BODY}_${"0".repeat(26)}_${SECRET}`),
    ).toThrow(AexConfigError);
    expect(() =>
      WorkspaceApiKey.parse(`aex_wk_euw1_${"0".repeat(26)}_${UUID7_BODY}_${SECRET}`),
    ).toThrow(AexConfigError);
    // The prelaunch spelling that named no workspace is refused outright.
    expect(() => WorkspaceApiKey.parse(`aex_wk_euw1_${UUID7_BODY}_${SECRET}`)).toThrow(
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
