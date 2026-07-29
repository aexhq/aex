/**
 * Schemas for the two MCP server declarations on the request side: the
 * non-secret `McpServerRef` that enters the hashed submission, and the
 * `SessionConfigMcpServer` a `session.json` may carry with inline `headers`.
 *
 * Host classification and transport narrowing are injected through
 * {@link McpWirePolicy}. This module owns wire shape and issue ordering; the
 * policy owns the accepted values and egress decision.
 */
import * as z from "zod/mini";
import type { McpServerRef, RemoteMcpTransport, SessionConfigMcpServer } from "../session-config.js";
import { parseWire, wireObject } from "./wire.js";

export const MCP_SERVER_NAME_PATTERN = /^[a-z][a-z0-9_-]{0,62}$/;

/**
 * Canonical error string for any attempt to declare a stdio-shaped MCP
 * server (`transport: "stdio"`, or a stdio-only field like `command` /
 * `args` / `env`). Pinned in source so every surface — shared parser,
 * SDK builder, CLI flag parser, dashboard form — surfaces the same
 * message and a user can find it via grep.
 */
export const REMOTE_MCP_STDIO_REJECTED_MESSAGE =
  "stdio MCP servers are not supported by Aex. Aex supports remote MCP servers over HTTP/SSE only.";

/**
 * Stdio-only fields. Used by the shape gate to detect a stdio declaration even
 * when the caller omits `transport: "stdio"` (e.g. `{ url, command }` — the
 * presence of `command` alone identifies it).
 */
const STDIO_ONLY_FIELDS = ["command", "args", "env"] as const;

/**
 * The host and transport rules the MCP schemas defer to.
 *
 * See the module header for the ownership split.
 */
export interface McpWirePolicy {
  /** Reason this URL's host must be refused, or `null` when it is acceptable. */
  readonly denyReasonForHost: (url: URL) => string | null;
  /** Throws when `input` is neither absent nor a supported remote transport. */
  readonly parseTransport: (input: unknown, field: string) => RemoteMcpTransport | undefined;
}

function isStdioShaped(value: unknown): boolean {
  if (value === null || typeof value !== "object") {
    return false;
  }
  const record = value as Record<string, unknown>;
  return (
    record.transport === "stdio" || STDIO_ONLY_FIELDS.some((field) => record[field] !== undefined)
  );
}

/**
 * A remote-only precondition on any MCP declaration, whatever else is wrong
 * with it.
 *
 * Deliberately its own schema, parsed BEFORE the strict object rather than
 * carried on it as a `.check()`. Zod raises `unrecognized_keys` while parsing
 * the object body and skips the object's own checks once the body has issues
 * (measured), and `errorFromZod` ranks a rejected key ahead of every other issue
 * at the same depth — so no rule expressed on the object can outrank an unknown
 * key. `{ command: "local", unknown: true }` must report stdio, not `unknown`,
 * which is exactly the sequencing the hand-written parser had: reject the stdio
 * shape, then apply the allow-list.
 */
export const RemoteMcpShapeSchema = z.unknown().check(
  z.refine((value: unknown) => !isStdioShaped(value), {
    error: REMOTE_MCP_STDIO_REJECTED_MESSAGE,
    abort: true
  })
);

/**
 * Throw the canonical stdio-rejected error if the record carries any
 * stdio-only marker (`transport: "stdio"`, `command`, `args`, `env`).
 * Used by both the shared parser and the SDK `McpServer.remote`
 * builder so every entry point surfaces the same message.
 */
export function rejectStdioMcpShape(record: Record<string, unknown>): void {
  parseWire(RemoteMcpShapeSchema, record);
}

function toUrl(value: unknown): URL | undefined {
  try {
    return new URL(value as string);
  } catch {
    return undefined;
  }
}

function mcpServerName(path: string) {
  const message = `${path}.name must match ${MCP_SERVER_NAME_PATTERN.source}`;
  return z.string({ error: message }).check(z.regex(MCP_SERVER_NAME_PATTERN, { error: message }));
}

/**
 * An http(s) MCP endpoint with no userinfo, on a host the SSRF deny-list allows.
 *
 * Each check aborts so the reported failure is the first one a reader would hit,
 * matching the sequential `new URL()` / protocol / userinfo / host ladder this
 * replaces. Auth belongs in `secrets.mcpServers[].headers`: a
 * `https://user:pass@host` URL would otherwise be persisted in the non-secret
 * session snapshot and hashed into the idempotency key.
 */
function mcpServerUrl(path: string, policy: McpWirePolicy) {
  const nonEmpty = `${path}.url must be a non-empty string`;
  return z.string({ error: nonEmpty }).check(
    z.minLength(1, { error: nonEmpty, abort: true }),
    z.refine((value: string) => toUrl(value) !== undefined, {
      error: (issue) => `${path}.url is not a valid URL: ${String(issue.input)}`,
      abort: true
    }),
    z.refine(
      (value: string) => {
        const protocol = toUrl(value)?.protocol;
        return protocol === "https:" || protocol === "http:";
      },
      {
        error: (issue) => `${path}.url must use http or https (got ${toUrl(issue.input)?.protocol ?? ""})`,
        abort: true
      }
    ),
    z.refine(
      (value: string) => {
        const url = toUrl(value);
        return url !== undefined && url.username === "" && url.password === "";
      },
      {
        error: `${path}.url must not contain userinfo (username/password); use secrets.mcpServers[].headers for auth`,
        abort: true
      }
    ),
    z.refine((value: string) => denyReason(value, policy) === null, {
      error: (issue) => `${path}.url ${denyReason(issue.input, policy) ?? ""}`,
      abort: true
    })
  );
}

function denyReason(value: unknown, policy: McpWirePolicy): string | null {
  const url = toUrl(value);
  return url === undefined ? null : policy.denyReasonForHost(url);
}

/**
 * The optional remote transport, validated by the policy's parser so the
 * permitted set is stated once — in `session-config.ts`, alongside
 * {@link McpWirePolicy}'s other deferred rule.
 *
 * The value shape is therefore opaque to the generated document (L2); the
 * permitted values live in the description registered on the objects below.
 */
function mcpServerTransport(path: string, policy: McpWirePolicy) {
  const field = `${path}.transport`;
  const failure = (value: unknown): string | null => {
    try {
      policy.parseTransport(value, field);
      return null;
    } catch (error) {
      return (error as Error).message;
    }
  };
  return z.optional(
    z.unknown().check(
      z.refine((value: unknown) => failure(value) === null, {
        error: (issue) => failure(issue.input) ?? "",
        abort: true
      })
    )
  );
}

/** Per-request headers, kept out of the non-secret ref — see {@link mcpServerRefSchema}. */
function mcpHeaders(path: string) {
  return z.optional(
    z.record(
      z.string(),
      z.string({
        error: (issue) => `${path}.headers.${lastSegment(issue.path)} must be a string`
      }),
      { error: `${path}.headers, when provided, must be a string-keyed object` }
    )
  );
}

function lastSegment(issuePath: readonly PropertyKey[] | undefined): string {
  const segment = issuePath?.[issuePath.length - 1];
  return segment === undefined ? "" : String(segment);
}

/**
 * Wire shape of the NON-SECRET half of an MCP server declaration.
 *
 * Headers belong on `SessionConfigMcpServer`, and the wire
 * `submission.mcpServers` must NEVER contain them, so anything other than
 * `{name, url, transport}` is rejected explicitly — a caller accidentally
 * inlining `headers` into the non-secret half fails loudly instead of having the
 * field silently dropped.
 */
export function mcpServerRefSchema(path: string, policy: McpWirePolicy) {
  return wireObject(
    path,
    {
      name: mcpServerName(path),
      url: mcpServerUrl(path, policy),
      transport: mcpServerTransport(path, policy)
    },
    {
      unknownKey: (objectPath, key, permitted) =>
        `${objectPath}.${key} is not an allowed field for McpServerRef; permitted: ${permitted.join(", ")}`
    }
  );
}

/**
 * Wire shape of a session-config MCP entry: the non-secret ref plus the inline
 * `headers` the SDK splits out at submission time, so config loaded from
 * `--config session.json` cannot smuggle unrelated fields past the parser.
 */
export function sessionConfigMcpServerSchema(path: string, policy: McpWirePolicy) {
  return wireObject(
    path,
    {
      name: mcpServerName(path),
      url: mcpServerUrl(path, policy),
      transport: mcpServerTransport(path, policy),
      headers: mcpHeaders(path)
    },
    {
      unknownKey: (objectPath, key, permitted) =>
        `${objectPath}.${key} is not an allowed field for SessionConfigMcpServer; permitted: ${permitted.join(", ")}`
    }
  );
}

export type McpServerRefWire = z.infer<ReturnType<typeof mcpServerRefSchema>>;
export type SessionConfigMcpServerWire = z.infer<ReturnType<typeof sessionConfigMcpServerSchema>>;

/** Reject a stdio shape, then validate the non-secret ref mounted at `path`. */
export function parseMcpServerRefWire(
  input: unknown,
  path: string,
  policy: McpWirePolicy
): McpServerRefWire {
  parseWire(RemoteMcpShapeSchema, input);
  return parseWire(mcpServerRefSchema(path, policy), input);
}

/** Reject a stdio shape, then validate a session-config entry mounted at `path`. */
export function parseSessionConfigMcpServerWire(
  input: unknown,
  path: string,
  policy: McpWirePolicy
): SessionConfigMcpServerWire {
  parseWire(RemoteMcpShapeSchema, input);
  return parseWire(sessionConfigMcpServerSchema(path, policy), input);
}

/**
 * Drop an absent `transport` so an omitted field never lands as an explicit
 * `undefined` on the hashed payload.
 *
 * Separate from the schema per D4 — see
 * {@link import("./asset-ref.js").normalizeAssetRef}.
 */
export function normalizeMcpServerRef(wire: McpServerRefWire): McpServerRef {
  // `transport` is validated by the policy's parser, which is what narrows it;
  // the schema itself only knows the value passed that gate.
  const transport = wire.transport as RemoteMcpTransport | undefined;
  return transport === undefined
    ? { name: wire.name, url: wire.url }
    : { name: wire.name, url: wire.url, transport };
}

/** As {@link normalizeMcpServerRef}, preserving inline `headers` when supplied. */
export function normalizeSessionConfigMcpServer(
  wire: SessionConfigMcpServerWire
): SessionConfigMcpServer {
  const ref = normalizeMcpServerRef(wire);
  return wire.headers === undefined ? ref : { ...ref, headers: wire.headers };
}
