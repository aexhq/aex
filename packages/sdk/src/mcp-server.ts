import {
  rejectStdioMcpShape,
  type McpServerRef,
  type PlatformMcpServerSecret,
  type RemoteMcpTransport
} from "@aexhq/contracts";

const WORKSPACE_MCP_ID_PATTERN = /^mcp_[A-Za-z0-9_-]{8,128}$/;

/**
 * Wire shape for a workspace MCP server ref in `submission.mcpServers[]`.
 * The BFF resolves this to an inline `{name, url}` before the shared
 * parser runs. Auth still arrives inline in
 * `secrets.mcpServers[<resolved-name>].headers` — workspace refs do
 * not carry auth, by design.
 */
export interface WorkspaceMcpServerSubmissionEntry {
  readonly kind: "workspace";
  readonly id: string;
}

/**
 * Inline MCP server reference OR a workspace-persistent ref via
 * `McpServer.fromId("mcp_...")`.
 *
 * For inline refs, headers (typically `Authorization`) are vaulted
 * server-side under `secrets.mcpServers` and excluded from the
 * idempotency hash; the non-secret `{name, url}` part is hashed.
 *
 * The SDK splits the header bag from the public ref BEFORE the wire
 * payload is built, so a single inline `McpServer` instance becomes:
 *
 *   - a `submission.mcpServers` entry of `{ name, url }`, and
 *   - (when `headers` is present) a `secrets.mcpServers` entry of
 *     `{ name, headers }`.
 *
 * For workspace refs (`McpServer.fromId(id)`), the submission entry
 * is `{kind:"workspace", id}`; the BFF resolves it to inline when
 * the session is created. The user still supplies auth separately via
 * `secrets.mcpServers[<workspace-name>]` keyed by the workspace
 * MCP's persisted `name`.
 */
export class McpServer {
  readonly kind: "inline" | "workspace";
  readonly name: string;
  readonly url: string;
  readonly transport: RemoteMcpTransport | undefined;
  readonly id: string | undefined;
  readonly headers: Readonly<Record<string, string>> | undefined;

  /** Internal constructor. Use `McpServer.remote(...)` or `McpServer.fromId(...)`. */
  constructor(args: {
    readonly kind?: "inline" | "workspace";
    readonly id?: string;
    readonly name?: string;
    readonly url?: string;
    readonly transport?: RemoteMcpTransport;
    readonly headers?: Readonly<Record<string, string>>;
  }) {
    if (!args || typeof args !== "object") {
      throw new Error("McpServer: args is required");
    }
    // Defence in depth: any stdio-shaped input that slips past the
    // builder type (callers will reach in with `as never` to feed stdio
    // shapes) is rejected with the canonical error.
    rejectStdioMcpShape(args as unknown as Record<string, unknown>);
    if (args.kind === "workspace") {
      if (typeof args.id !== "string" || !WORKSPACE_MCP_ID_PATTERN.test(args.id)) {
        throw new Error(`McpServer.fromId: id must match ${WORKSPACE_MCP_ID_PATTERN.source}`);
      }
      this.kind = "workspace";
      this.id = args.id;
      // `name` and `url` are resolved server-side; populate placeholders so
      // type-level access never returns undefined, but mark the workspace
      // ref so callers can branch.
      this.name = "";
      this.url = "";
      this.transport = undefined;
      this.headers = undefined;
      return;
    }
    if (typeof args.name !== "string" || !args.name) {
      throw new Error("McpServer: name is required");
    }
    if (typeof args.url !== "string" || !args.url) {
      throw new Error("McpServer: url is required");
    }
    this.kind = "inline";
    this.id = undefined;
    this.name = args.name;
    this.url = args.url;
    this.transport = args.transport;
    this.headers = args.headers && Object.keys(args.headers).length > 0 ? { ...args.headers } : undefined;
  }

  /**
   * Inline remote MCP server reachable over HTTP(S) by URL. `transport`
   * defaults to the runtime's choice when omitted; pass it explicitly
   * to pin `"http"` (streamable) or `"sse"` (event-stream). Stdio is
   * not supported — see {@link rejectStdioMcpShape}.
   */
  static remote(args: {
    readonly name: string;
    readonly url: string;
    readonly transport?: RemoteMcpTransport;
    readonly headers?: Readonly<Record<string, string>>;
  }): McpServer {
    return new McpServer({ kind: "inline", ...args });
  }

  /**
   * Reference a workspace-persistent MCP server by id. The BFF
   * resolves the ref to inline `{name, url}` when the session is created using
   * the row from `workspace_mcp_servers`. Auth still arrives inline at
   * the call site — pass it in `secrets.mcpServers[<workspace-name>]`.
   */
  static fromId(id: string): McpServer {
    return new McpServer({ kind: "workspace", id });
  }

  /** Wire shape for the non-secret `submission.mcpServers` entry. */
  toSubmissionEntry(): McpServerRef | WorkspaceMcpServerSubmissionEntry {
    if (this.kind === "workspace") {
      return { kind: "workspace", id: this.id! };
    }
    return this.transport
      ? { name: this.name, transport: this.transport, url: this.url }
      : { name: this.name, url: this.url };
  }

  /**
   * Wire shape for the `secrets.mcpServers` entry, or `undefined`
   * when no headers were provided. Returned headers are a defensive
   * copy.
   *
   * Workspace refs never emit a secret entry from the SDK — the
   * caller knows the resolved `name` (from the dashboard or from a
   * prior list call) and supplies the headers under that name
   * directly on the `secrets.mcpServers[]` array. Mixing SDK-side
   * auth into a workspace ref's `name` would create a name/auth
   * mismatch the BFF can't validate.
   */
  toSecretEntry(): PlatformMcpServerSecret | undefined {
    if (this.kind === "workspace" || !this.headers) {
      return undefined;
    }
    return { name: this.name, url: this.url, headers: { ...this.headers } };
  }
}
