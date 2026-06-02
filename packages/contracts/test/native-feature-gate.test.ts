// Regression: per AGENTS.md "No feature gates between runtimes" — native
// and managed must serve identical features. The native runtime accepts
// inline skills, files, and MCP servers; auth for MCP attaches at session
// time via an Anthropic vault whose static_bearer credentials are keyed
// on `mcp_server_url` (preflight registers the antpath MCP proxy URL
// with the per-run runner bearer). This test pins that ALL three
// features are accepted on native — an asymmetric rejection is the bug,
// not a feature.

import { describe, expect, it } from "vitest";
import {
  parseRunSubmissionRequest,
  selectRuntime,
  type PlatformRunSubmissionInput
} from "../src/index.js";

const WS = "11111111-1111-4111-8111-111111111111";
const HEX = "a".repeat(64);

function r2Skill() {
  return {
    kind: "r2" as const,
    path: `assets/${WS}/${HEX}`,
    hash: `sha256:${HEX}`,
    sizeBytes: 100,
    name: "rules"
  };
}

function r2File() {
  return {
    kind: "r2" as const,
    path: `assets/${WS}/${"b".repeat(64)}`,
    hash: `sha256:${"b".repeat(64)}`,
    sizeBytes: 200,
    name: "data.csv"
  };
}

function mcpServer() {
  return { name: "deepwiki", url: "https://mcp.deepwiki.com/mcp" };
}

/**
 * Build a native-routed Anthropic submission carrying one inline-feature
 * kind. `runtime: "native"` is explicit so there is no ambiguity about
 * where the run lands; the same gap exists with `runtime: undefined`
 * (anthropic auto-routes to native).
 */
function nativeRequestWith(
  feature: "skills" | "files" | "mcpServers"
): PlatformRunSubmissionInput {
  const submission = {
    model: "claude-sonnet-test",
    prompt: ["hello"],
    skills: feature === "skills" ? [r2Skill()] : [],
    agentsMd: [],
    files: feature === "files" ? [r2File()] : [],
    mcpServers: feature === "mcpServers" ? [mcpServer()] : []
  };
  return {
    workspaceId: WS,
    idempotencyKey: "idem-3-7",
    provider: "anthropic",
    runtime: "native",
    submission,
    secrets: { anthropic: { apiKey: "sk-ant-test" } }
  };
}

/** A native run carrying ONLY fields the native path serves today. */
function nativeRequestPlain(): PlatformRunSubmissionInput {
  return {
    workspaceId: WS,
    idempotencyKey: "idem-plain",
    provider: "anthropic",
    runtime: "native",
    submission: {
      model: "claude-sonnet-test",
      system: "be precise",
      prompt: ["hello"],
      skills: [],
      agentsMd: [],
      files: [],
      mcpServers: []
    },
    secrets: { anthropic: { apiKey: "sk-ant-test" } }
  };
}

/** A managed run carrying the same inline features native rejects. */
function managedRequestWith(
  feature: "skills" | "files" | "mcpServers"
): PlatformRunSubmissionInput {
  return {
    workspaceId: WS,
    idempotencyKey: "idem-managed",
    provider: "anthropic",
    runtime: "managed",
    submission: {
      model: "claude-sonnet-test",
      prompt: ["hello"],
      skills: feature === "skills" ? [r2Skill()] : [],
      agentsMd: [],
      files: feature === "files" ? [r2File()] : [],
      mcpServers: feature === "mcpServers" ? [mcpServer()] : []
    },
    secrets: { anthropic: { apiKey: "sk-ant-test" } }
  };
}

describe("native runtime feature gate", () => {
  // Per AGENTS.md "No feature gates between runtimes" — native and managed
  // serve identical features. Skills/files via Skills API + Files API;
  // MCP via Anthropic session vaults keyed on mcp_server_url.
  it.each(["skills", "files", "mcpServers"] as const)(
    "accepts a native-runtime submission carrying inline %s (native now serves it)",
    (feature) => {
      const parsed = parseRunSubmissionRequest(nativeRequestWith(feature));
      expect(selectRuntime(parsed)).toBe("native");
    }
  );

  it.each(["skills", "files", "mcpServers"] as const)(
    "auto-routed Anthropic (no explicit runtime) also accepts inline %s on native",
    (feature) => {
      const req = nativeRequestWith(feature);
      const { runtime: _drop, ...autoRouted } = req;
      expect(selectRuntime(parseRunSubmissionRequest(autoRouted))).toBe("native");
    }
  );

  it("accepts a native run that uses only system + prompt (no over-rejection)", () => {
    const parsed = parseRunSubmissionRequest(nativeRequestPlain());
    expect(selectRuntime(parsed)).toBe("native");
  });

  it.each(["skills", "files", "mcpServers"] as const)(
    "accepts the same inline %s on the managed runtime (Goose serves them)",
    (feature) => {
      const parsed = parseRunSubmissionRequest(managedRequestWith(feature));
      expect(selectRuntime(parsed)).toBe("managed");
    }
  );
});
