/**
 * Submission parser — AgentsMd / Files acceptance after the asset-id boundary.
 *
 * Only `kind: "asset"` is valid on the public wire.
 */

import { describe, expect, it } from "vitest";
import { parseRunSubmissionRequest } from "../src/submission.js";

const WS_ID = "11111111-1111-4111-8111-111111111111";
const HASH_HEX = "a".repeat(64);
const VALID_ASSET = {
  kind: "asset" as const,
  assetId: `asset_${HASH_HEX}`,
  name: "rules"
};

function baseRequest(overrides: { agentsMd?: unknown[]; files?: unknown[] } = {}) {
  return {
    workspaceId: WS_ID,
    idempotencyKey: "idem-1",
    provider: "anthropic" as const,
    submission: {
      model: "claude-haiku-4-5",
      prompt: ["hi"],      agentsMd: overrides.agentsMd ?? [],
      files: overrides.files ?? [],
      mcpServers: []
    },
    secrets: { apiKeys: { anthropic: "sk-ant-test" } }
  };
}

describe("parseRunSubmissionRequest — agentsMd[] (asset refs)", () => {
  it("accepts a kind:'asset' ref", () => {
    const parsed = parseRunSubmissionRequest(baseRequest({ agentsMd: [VALID_ASSET] }));
    expect(parsed.submission.agentsMd).toEqual([VALID_ASSET]);
  });

  it("rejects non-asset agentsMd refs", () => {
    expect(() =>
      parseRunSubmissionRequest(baseRequest({ agentsMd: [{ kind: "not_asset", id: "amd_x" }] }))
    ).toThrow(/kind must be 'asset'/);
  });

  it("rejects duplicate asset ids", () => {
    expect(() =>
      parseRunSubmissionRequest(baseRequest({ agentsMd: [VALID_ASSET, VALID_ASSET] }))
    ).toThrow(/duplicate assetId/);
  });
});

describe("parseRunSubmissionRequest — files[] (asset refs)", () => {
  it("accepts a kind:'asset' ref without mountPath", () => {
    const parsed = parseRunSubmissionRequest(baseRequest({ files: [VALID_ASSET] }));
    expect(parsed.submission.files).toEqual([VALID_ASSET]);
  });

  it("accepts a kind:'asset' ref with absolute mountPath", () => {
    const withMount = { ...VALID_ASSET, mountPath: "/aex/files/x/data.csv" };
    const parsed = parseRunSubmissionRequest(baseRequest({ files: [withMount] }));
    expect(parsed.submission.files).toEqual([withMount]);
  });

  it("rejects relative mountPath", () => {
    const bad = { ...VALID_ASSET, mountPath: "relative/path" };
    expect(() =>
      parseRunSubmissionRequest(baseRequest({ files: [bad] }))
    ).toThrow(/mountPath must be an absolute path starting with '\/'/);
  });

  it("rejects mountPath with '..' traversal", () => {
    const bad = { ...VALID_ASSET, mountPath: "/workspace/../etc" };
    expect(() =>
      parseRunSubmissionRequest(baseRequest({ files: [bad] }))
    ).toThrow(/mountPath must not contain '\.\.' traversal/);
  });

  it("rejects any kind other than 'asset'", () => {
    expect(() =>
      parseRunSubmissionRequest(baseRequest({ files: [{ kind: "not_asset", id: "f_x" }] }))
    ).toThrow(/kind must be 'asset'/);
  });

  it("rejects duplicate asset ids", () => {
    expect(() =>
      parseRunSubmissionRequest(baseRequest({ files: [VALID_ASSET, VALID_ASSET] }))
    ).toThrow(/duplicate assetId/);
  });
});
