/**
 * SDK shape tests for McpServer.
 *
 *   - McpServer.remote(...)   = inline ref, wire {name, url}
 *   - McpServer.fromId("mcp_...") = workspace ref, wire {kind:"workspace", id}
 *
 * Workspace refs do NOT carry auth from the SDK — the user supplies
 * auth inline in `secrets.mcpServers[<resolved-name>].headers` keyed
 * by the workspace MCP's persisted name. The BFF resolves the ref to
 * `{name, url}` BEFORE the shared parser sessions, so the parser only
 * ever sees inline shapes.
 */
import { describe, expect, it } from "bun:test";
import { newId } from "@aexhq/contracts";
import { McpServer } from "../../src/mcp-server.js";

// Minted from the id owner, never hand-written: a fixture like
// `mcp_abcdefghijkl` passed the SDK's old local pattern and no other layer.
const MCP_ID = newId("mcp");

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
      "stdio MCP servers are not supported by Aex. Aex supports remote MCP servers over HTTP/SSE only.";
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
    const s = McpServer.fromId(MCP_ID);
    expect(s.kind).toBe("workspace");
    expect(s.id).toBe(MCP_ID);
    expect(s.toSubmissionEntry()).toEqual({ kind: "workspace", id: MCP_ID });
  });

  it("rejects every id the owner does not mint", () => {
    expect(() => McpServer.fromId("not-a-valid-id")).toThrow(/must match/);
    expect(() => McpServer.fromId("mcp_short")).toThrow(/must match/);
    expect(() => McpServer.fromId("skl_wrong_prefix_1234567890")).toThrow(/must match/);
    // These three passed the retired local pattern: a wrong-length body, a
    // non-hex body, and uppercase hex (`newId` mints lowercase, so an
    // uppercase id is a different string, not another encoding).
    expect(() => McpServer.fromId("mcp_abcdefghijkl")).toThrow(/must match/);
    expect(() => McpServer.fromId(`mcp_${"z".repeat(32)}`)).toThrow(/must match/);
    expect(() => McpServer.fromId(MCP_ID.toUpperCase())).toThrow(/must match/);
    // The owner's own id is accepted, and the message names the owner's shape.
    expect(McpServer.fromId(MCP_ID).id).toBe(MCP_ID);
  });

  it("never carries auth — toSecretEntry is always undefined for workspace refs", () => {
    const s = McpServer.fromId(MCP_ID);
    expect(s.toSecretEntry()).toBeUndefined();
  });

  it("workspace refs have empty name/url at SDK time (resolved server-side)", () => {
    const s = McpServer.fromId(MCP_ID);
    expect(s.name).toBe("");
    expect(s.url).toBe("");
  });
});
