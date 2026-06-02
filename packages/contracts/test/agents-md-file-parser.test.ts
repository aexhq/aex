/**
 * Submission parser — AgentsMd / Files acceptance after Phase D.
 *
 * Only `kind: "r2"` is valid on the wire (no workspace/inline/transient
 * kinds anymore — R2 is the canonical asset store). The richer
 * end-to-end coverage lives in server-side submission tests
 * which also exercises hosted asset validation.
 */

import { describe, expect, it } from "vitest";
import { parseRunSubmissionRequest } from "../src/submission.js";

const WS_ID = "11111111-1111-4111-8111-111111111111";
const HASH_HEX = "a".repeat(64);
const VALID_R2 = {
  kind: "r2" as const,
  path: `assets/${WS_ID}/${HASH_HEX}`,
  hash: `sha256:${HASH_HEX}`,
  sizeBytes: 100,
  name: "rules"
};

function baseRequest(overrides: { agentsMd?: unknown[]; files?: unknown[] } = {}) {
  return {
    workspaceId: WS_ID,
    idempotencyKey: "idem-1",
    provider: "anthropic" as const,
    submission: {
      model: "claude-haiku-4-5",
      prompt: ["hi"],
      skills: [],
      agentsMd: overrides.agentsMd ?? [],
      files: overrides.files ?? [],
      mcpServers: []
    },
    secrets: { anthropic: { apiKey: "sk-ant-test" } }
  };
}

describe("parseRunSubmissionRequest — agentsMd[] (R2-only)", () => {
  it("accepts a kind:'r2' ref", () => {
    const parsed = parseRunSubmissionRequest(baseRequest({ agentsMd: [VALID_R2] }));
    expect(parsed.submission.agentsMd).toEqual([VALID_R2]);
  });

  it("rejects any kind other than 'r2'", () => {
    expect(() =>
      parseRunSubmissionRequest(baseRequest({ agentsMd: [{ kind: "workspace_agentsmd", id: "amd_x" }] }))
    ).toThrow(/kind must be 'r2'/);
    expect(() =>
      parseRunSubmissionRequest(baseRequest({ agentsMd: [{ kind: "transient_agentsmd", slot: "x", name: "n", contentHash: `sha256:${HASH_HEX}` }] }))
    ).toThrow(/kind must be 'r2'/);
  });

  it("rejects duplicate r2 paths", () => {
    expect(() =>
      parseRunSubmissionRequest(baseRequest({ agentsMd: [VALID_R2, VALID_R2] }))
    ).toThrow(/duplicate r2 path/);
  });
});

describe("parseRunSubmissionRequest — files[] (R2-only)", () => {
  it("accepts a kind:'r2' ref without mountPath", () => {
    const parsed = parseRunSubmissionRequest(baseRequest({ files: [VALID_R2] }));
    expect(parsed.submission.files).toEqual([VALID_R2]);
  });

  it("accepts a kind:'r2' ref with absolute mountPath", () => {
    const withMount = { ...VALID_R2, mountPath: "/antpath/files/x/data.csv" };
    const parsed = parseRunSubmissionRequest(baseRequest({ files: [withMount] }));
    expect(parsed.submission.files).toEqual([withMount]);
  });

  it("rejects relative mountPath", () => {
    const bad = { ...VALID_R2, mountPath: "relative/path" };
    expect(() =>
      parseRunSubmissionRequest(baseRequest({ files: [bad] }))
    ).toThrow(/mountPath must start with '\/'/);
  });

  it("rejects any kind other than 'r2'", () => {
    expect(() =>
      parseRunSubmissionRequest(baseRequest({ files: [{ kind: "workspace_file", id: "f_x" }] }))
    ).toThrow(/kind must be 'r2'/);
  });

  it("rejects duplicate r2 paths", () => {
    expect(() =>
      parseRunSubmissionRequest(baseRequest({ files: [VALID_R2, VALID_R2] }))
    ).toThrow(/duplicate r2 path/);
  });
});
