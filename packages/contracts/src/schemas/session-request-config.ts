/**
 * Schema for the plain JSON `aex start --config <path>` accepts: the
 * credential-free session parameters the CLI folds into a normal `submit`.
 *
 * Every message this family raises is prefixed with the words
 * `session request config` rather than a dotted path, because that is what a
 * user editing a `session.json` sees. The prefix is the schema's mount path, so
 * `wireObject`'s default `notObject` wording already reads correctly.
 */
import * as z from "zod/mini";
import type { SessionConfigMcpServer, SessionRequestConfig } from "../session-config.js";
import { modelSlugSchema } from "./models.js";
import {
  normalizeSessionConfigMcpServer,
  parseSessionConfigMcpServerWire,
  type McpWirePolicy
} from "./mcp-server.js";
import { parseWire, wireObject } from "./wire.js";

const CONFIG = "session request config";

/**
 * `environment`, `runtimeSize`, `timeout` and `metadata` are carried through
 * unvalidated ON PURPOSE: the BFF revalidates them via
 * `parseSessionSubmissionRequest`, and duplicating those parsers here would be a
 * second source of truth for the same rules. The CLI surfaces their structural
 * errors at submission time instead. They are untyped in the generated document
 * as a result — a known gap, not a described one.
 */
const passthrough = z.optional(z.unknown());

/**
 * The prompt ladder, stated once and reported for every rejected shape.
 *
 * The array case names the offending index, which the union's branch issues
 * cannot, so the wording is derived from the input here rather than attached to
 * the branches.
 */
function promptMessage(value: unknown): string {
  if (typeof value === "string") {
    return `${CONFIG} prompt must be a non-empty string`;
  }
  if (Array.isArray(value)) {
    const index = value.findIndex((item) => typeof item !== "string" || item.length === 0);
    return index === -1
      ? `${CONFIG} prompt must be a non-empty string or array of strings`
      : `${CONFIG} prompt[${index}] must be a non-empty string`;
  }
  return `${CONFIG} prompt must be a string or array of strings`;
}

/**
 * One turn, or an ordered list of them.
 *
 * Every branch check aborts. That is load-bearing: Zod surfaces a single
 * branch's issues when exactly one branch fails non-abortively, and the union's
 * own `error` only when they all abort (measured). Aborting throughout makes
 * {@link promptMessage} the reported wording in every case.
 */
const prompt = z.union(
  [
    z.string().check(z.minLength(1, { abort: true })),
    z.array(z.string().check(z.minLength(1, { abort: true }))).check(z.minLength(1, { abort: true }))
  ],
  { error: (issue) => promptMessage(issue.input) }
);

/**
 * Wire shape of a session request config. Declaration order is both the
 * permitted-key order and the order the field checks report in, matching the
 * sequential model/system/prompt/mcpServers ladder this replaces.
 *
 * `mcpServers` is declared as a bare array here and its entries are validated by
 * {@link parseSessionConfigMcpServers} — see that function for why the entries
 * cannot be an element schema.
 */
export const SessionRequestConfigSchema = wireObject(
  CONFIG,
  {
    model: modelSlugSchema(`${CONFIG} model`),
    system: z.optional(z.string({ error: `${CONFIG} system, when provided, must be a string` })),
    prompt,
    mcpServers: z.optional(z.array(z.unknown(), { error: `${CONFIG} mcpServers must be an array` })),
    environment: passthrough,
    runtimeSize: passthrough,
    timeout: passthrough,
    metadata: passthrough
  },
  { unknownKey: (path, key) => `${path} contains unexpected field: ${key}` }
);

export type SessionRequestConfigWire = z.infer<typeof SessionRequestConfigSchema>;

/**
 * Validate each MCP entry at its own indexed path, in order, and reject a
 * repeated name.
 *
 * A loop rather than `z.array(sessionConfigMcpServerSchema(...))` because each
 * entry's stdio rejection has to outrank that entry's own unknown-key error, and
 * inside a single parse it cannot: Zod raises `unrecognized_keys` during the
 * object body and `errorFromZod` ranks it ahead of everything else at the same
 * depth. Sequencing the stdio gate and the object parse per entry is what the
 * hand-written parser did, and it is the only place that precedence can be
 * expressed — see {@link import("./mcp-server.js").RemoteMcpShapeSchema}.
 */
export function parseSessionConfigMcpServers(
  entries: readonly unknown[] | undefined,
  policy: McpWirePolicy
): readonly SessionConfigMcpServer[] | undefined {
  if (entries === undefined) {
    return undefined;
  }
  const seen = new Set<string>();
  return entries.map((item, index) => {
    const entry = normalizeSessionConfigMcpServer(
      parseSessionConfigMcpServerWire(item, `${CONFIG} mcpServers[${index}]`, policy)
    );
    if (seen.has(entry.name)) {
      throw new Error(`${CONFIG} mcpServers duplicate name: ${entry.name}`);
    }
    seen.add(entry.name);
    return entry;
  });
}

/**
 * Drop the absent optional fields and restore the declared field order.
 *
 * Separate from the schema per D4 — see
 * {@link import("./asset-ref.js").normalizeAssetRef}. Named for the wire type it
 * consumes so it is not confused with the public
 * `normaliseSessionRequestConfig`, which is a different operation: that one
 * splits a parsed config into the non-secret submission and the secret
 * MCP-headers bundle.
 */
export function normalizeSessionRequestConfigWire(
  wire: SessionRequestConfigWire,
  mcpServers: readonly SessionConfigMcpServer[] | undefined
): SessionRequestConfig {
  return {
    model: wire.model,
    ...(wire.system !== undefined ? { system: wire.system } : {}),
    prompt: wire.prompt,
    ...(mcpServers !== undefined ? { mcpServers } : {}),
    ...(wire.environment !== undefined
      ? { environment: wire.environment as NonNullable<SessionRequestConfig["environment"]> }
      : {}),
    ...(wire.runtimeSize !== undefined
      ? { runtimeSize: wire.runtimeSize as NonNullable<SessionRequestConfig["runtimeSize"]> }
      : {}),
    ...(wire.timeout !== undefined
      ? { timeout: wire.timeout as NonNullable<SessionRequestConfig["timeout"]> }
      : {}),
    ...(wire.metadata !== undefined
      ? { metadata: wire.metadata as NonNullable<SessionRequestConfig["metadata"]> }
      : {})
  };
}

/** Validate a session request config end to end, in the family's own words. */
export function parseSessionRequestConfigWire(
  input: unknown,
  policy: McpWirePolicy
): SessionRequestConfig {
  const wire = parseWire(SessionRequestConfigSchema, input);
  return normalizeSessionRequestConfigWire(
    wire,
    parseSessionConfigMcpServers(wire.mcpServers, policy)
  );
}
