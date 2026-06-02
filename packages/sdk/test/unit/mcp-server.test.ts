/**
 * SDK shape tests for McpServer.
 *
 *   - McpServer.remote(...)   = inline ref, wire {name, url}
 *   - McpServer.fromId("mcp_...") = workspace ref, wire {kind:"workspace", id}
 *
 * Workspace refs do NOT carry auth from the SDK — the user supplies
 * auth inline in `secrets.mcpServers[<resolved-name>].headers` keyed
 * by the workspace MCP's persisted name. The BFF resolves the ref to
 * `{name, url}` BEFORE the shared parser runs, so the parser only
 * ever sees inline shapes.
 */
import { describe, expect, it } from "vitest";
import { McpServer } from "../../src/mcp-server.js";

describe("McpServer.remote (inline)", () => {
  it("toSubmissionEntry returns the inline {name, url} wire shape", () => {
    const s = McpServer.remote({ name: "ctx7", url: "https://mcp.example.com/mcp" });
    expect(s.kind).toBe("inline");
    expect(s.toSubmissionEntry()).toEqual({ name: "ctx7", url: "https://mcp.example.com/mcp" });
  });

  it("carries optional remote transport on the non-secret wire shape", () => {
    const s = McpServer.remote({
      name: "events",
      transport: "sse",
      url: "https://mcp.example.com/sse"
    });
    expect(s.transport).toBe("sse");
    expect(s.toSubmissionEntry()).toEqual({
      name: "events",
      transport: "sse",
      url: "https://mcp.example.com/sse"
    });
  });

  it("toSecretEntry returns headers when provided", () => {
    const s = McpServer.remote({
      name: "ctx7",
      url: "https://mcp.example.com/mcp",
      headers: { Authorization: "Bearer ant-123" }
    });
    expect(s.toSecretEntry()).toEqual({
      name: "ctx7",
      url: "https://mcp.example.com/mcp",
      headers: { Authorization: "Bearer ant-123" }
    });
  });

  it("toSecretEntry is undefined when no headers were supplied", () => {
    const s = McpServer.remote({ name: "ctx7", url: "https://mcp.example.com" });
    expect(s.toSecretEntry()).toBeUndefined();
  });

  it("rejects stdio-shaped local MCP declarations", () => {
    const expected =
      "stdio MCP servers are not supported by Antpath. Antpath supports remote MCP servers over HTTP/SSE only.";
    expect(() =>
      McpServer.remote({
        name: "local",
        transport: "stdio" as never,
        url: "https://mcp.example.com"
      })
    ).toThrow(expected);
    expect(() =>
      McpServer.remote({
        name: "local",
        url: "https://mcp.example.com",
        command: "node"
      } as never)
    ).toThrow(expected);
  });
});

describe("McpServer.fromId (workspace ref)", () => {
  it("accepts a valid mcp_* id and emits the workspace-ref wire shape", () => {
    const s = McpServer.fromId("mcp_abcdefghijkl");
    expect(s.kind).toBe("workspace");
    expect(s.id).toBe("mcp_abcdefghijkl");
    expect(s.toSubmissionEntry()).toEqual({ kind: "workspace", id: "mcp_abcdefghijkl" });
  });

  it("rejects ids that don't match the mcp_<base64url> pattern", () => {
    expect(() => McpServer.fromId("not-a-valid-id")).toThrow(/must match/);
    expect(() => McpServer.fromId("mcp_short")).toThrow(/must match/);
    expect(() => McpServer.fromId("skl_wrong_prefix_1234567890")).toThrow(/must match/);
  });

  it("never carries auth — toSecretEntry is always undefined for workspace refs", () => {
    const s = McpServer.fromId("mcp_abcdefghijkl");
    expect(s.toSecretEntry()).toBeUndefined();
  });

  it("workspace refs have empty name/url at SDK time (resolved server-side)", () => {
    const s = McpServer.fromId("mcp_abcdefghijkl");
    expect(s.name).toBe("");
    expect(s.url).toBe("");
  });
});
