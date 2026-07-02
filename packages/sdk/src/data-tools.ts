/**
 * Data-source chat tools — turn the SDK's sessions read surface into LLM tool
 * definitions.
 *
 * `createDataTools(client)` returns a small, PROVIDER-AGNOSTIC tool set (plain
 * JSON-Schema `input_schema`, the Anthropic/OpenAI tool shape) plus an
 * `execute(name, input)` dispatcher. A chat backend wires `tools` into its
 * Messages loop and calls `execute` on each `tool_use`. There is no LLM-vendor
 * dependency here: the executor only ever calls public `Aex` session
 * methods, so a chat built on these tools can reach a workspace's sessions and
 * outputs and NOTHING else — internal/operator data is unreachable by construction.
 *
 * The tools follow search-then-fetch: `list_sessions` / `list_outputs` return
 * lean references and metadata; only `read_output` returns file content, and it
 * is byte-capped so a large deliverable never floods the context window.
 */
import type { Aex } from "./client.js";
import type { OutputFileSelector } from "./client.js";
import type { SessionListQuery } from "@aexhq/contracts";

/** JSON Schema for a tool's input — the subset every major LLM tool API accepts. */
export interface DataChatToolSchema {
  readonly type: "object";
  readonly properties: Record<string, unknown>;
  readonly required?: readonly string[];
  readonly additionalProperties?: boolean;
}

/** One tool definition in the vendor-neutral `{ name, description, input_schema }` shape. */
export interface DataChatTool {
  readonly name: string;
  readonly description: string;
  readonly input_schema: DataChatToolSchema;
}

/** The tool set plus its dispatcher, returned by {@link createDataTools}. */
export interface DataTools {
  /** Tool definitions to pass to the model. */
  readonly tools: readonly DataChatTool[];
  /** A system-prompt fragment that teaches the model to search-then-fetch. */
  readonly instructions: string;
  /**
   * Run one tool by name with the model's arguments. Returns a JSON-serializable
   * result (never raw bytes). Throws `DataToolError` for an unknown tool or
   * invalid arguments — surface that text back to the model as the tool result.
   */
  execute(name: string, input: Record<string, unknown>): Promise<unknown>;
}

export interface CreateDataToolsOptions {
  /** Default `max_bytes` for `read_output` when the model omits it. Default 50_000. */
  readonly defaultReadBytes?: number;
}

/**
 * Scopes a chat to a fixed set of sessions. Either pin explicit `sessionIds`
 * (the primary path — needs only `sessions.get` / `sessions.outputs(id).read`,
 * no `sessions.list`), or supply a `filter`
 * (status/since/limit) resolved to a concrete allow-list via `sessions.list`
 * (owner-gated). Every corpus read tool refuses a session outside the resolved
 * set.
 */
export interface ChatCorpus {
  /** Explicit session-id allow-list. */
  readonly sessionIds?: readonly string[];
  /** Resolve the corpus from a `sessions.list` query (status / since / limit). */
  readonly filter?: SessionListQuery;
}

/** Thrown by `DataTools.execute` for an unknown tool or malformed arguments. */
export class DataToolError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "DataToolError";
  }
}

const DEFAULT_READ_BYTES = 50_000;

export const DATA_TOOLS_INSTRUCTIONS =
  "You can read this workspace's agent sessions and their output files. " +
  "Call `list_sessions` to discover sessions (most recent first; page with `cursor`). " +
  "Call `list_outputs` for a session to see its files. Only call `read_output` for a " +
  "specific file when you actually need its contents — it is byte-capped, so if a " +
  "read comes back `truncated`, narrow it with `grep` or a more specific `path` " +
  "rather than re-reading. Never assume a session or file exists without listing first.";

/** The vendor-neutral tool definitions, shared by createDataTools/createCorpusTools. */
function buildToolDefs(defaultReadBytes: number): readonly DataChatTool[] {
  return [
    {
      name: "list_sessions",
      description:
        "List agent sessions in the workspace, most recent first. Returns id/status/timestamps " +
        "(and cost when settled) plus a `nextCursor` for the next page. Does NOT return prompts or outputs.",
      input_schema: {
        type: "object",
        additionalProperties: false,
        properties: {
          status: { type: "string", description: "Filter by session status, e.g. 'idle'." },
          since: { type: "string", description: "ISO-8601 lower bound on createdAt." },
          limit: { type: "integer", description: "Page size, 1-100 (default 25)." },
          cursor: { type: "string", description: "Opaque cursor from a prior page's nextCursor." }
        }
      }
    },
    {
      name: "get_session",
      description: "Get one session's status, timing, and cost summary by id. Does NOT return the submission or outputs.",
      input_schema: {
        type: "object",
        additionalProperties: false,
        required: ["session_id"],
        properties: { session_id: { type: "string", description: "The session id." } }
      }
    },
    {
      name: "list_outputs",
      description: "List a session's captured output files (id, filename, size, content type). Metadata only, no content.",
      input_schema: {
        type: "object",
        additionalProperties: false,
        required: ["session_id"],
        properties: { session_id: { type: "string", description: "The session id." } }
      }
    },
    {
      name: "read_output",
      description:
        "Read one output file of a session as text. Byte-capped: if the file is larger than `max_bytes` the result " +
        "is a prefix with `truncated: true`. Select the file by `path` (e.g. 'report.md') or by `id` from list_outputs.",
      input_schema: {
        type: "object",
        additionalProperties: false,
        required: ["session_id"],
        properties: {
          session_id: { type: "string", description: "The session id." },
          path: { type: "string", description: "Output file path/name (suffix match). Provide this or `id`." },
          id: { type: "string", description: "Output file id from list_outputs. Provide this or `path`." },
          max_bytes: { type: "integer", description: `Cap on bytes read (default ${defaultReadBytes}, max 10000000).` },
          grep: { type: "string", description: "Optional: return only lines matching this substring (case-insensitive)." }
        }
      }
    },
    {
      name: "search_outputs",
      description:
        "Find output files across sessions by filename/extension/content type. Returns references " +
        "(session_id, outputId, filename, size) — call read_output to fetch content.",
      input_schema: {
        type: "object",
        additionalProperties: false,
        properties: {
          filename: { type: "string", description: "Case-insensitive substring match on the filename." },
          extension: { type: "string", description: "File extension, e.g. 'md', 'json' (no dot)." },
          content_type: { type: "string", description: "Exact content type or a prefix wildcard like 'image/*'." },
          limit: { type: "integer", description: "Max hits (default 100)." }
        }
      }
    }
  ];
}

/** Shared executor over the read surface; `corpus` (when present) enforces the allow-list. */
function makeExecute(
  client: Aex,
  defaultReadBytes: number,
  corpus?: CorpusGuard
): (name: string, input: Record<string, unknown>) => Promise<unknown> {
  return async function execute(name: string, input: Record<string, unknown>): Promise<unknown> {
    const args = input ?? {};
    switch (name) {
      case "list_sessions": {
        if (corpus) return { sessions: await corpus.listSessionSummaries() };
        const page = await client.sessions.list({
          ...(typeof args.status === "string" ? { status: args.status } : {}),
          ...(typeof args.since === "string" ? { since: args.since } : {}),
          ...(typeof args.limit === "number" ? { limit: args.limit } : {}),
          ...(typeof args.cursor === "string" ? { cursor: args.cursor } : {})
        });
        return {
          sessions: page.sessions.map(summarizeSession),
          ...(page.nextCursor !== undefined ? { nextCursor: page.nextCursor } : {})
        };
      }
      case "get_session": {
        const sessionId = requireString(args.session_id, "session_id");
        if (corpus) await corpus.ensure(sessionId);
        return summarizeSession(await client.sessions.get(sessionId));
      }
      case "list_outputs": {
        const sessionId = requireString(args.session_id, "session_id");
        if (corpus) await corpus.ensure(sessionId);
        const outputs = await client.sessions.outputs(sessionId).list();
        return outputs.map((o) => ({
          id: o.id,
          filename: o.filename,
          sizeBytes: o.sizeBytes,
          contentType: o.contentType
        }));
      }
      case "read_output": {
        const sessionId = requireString(args.session_id, "session_id");
        if (corpus) await corpus.ensure(sessionId);
        const selector = readSelector(args);
        const result = await client.sessions.outputs(sessionId).read(selector, {
          maxBytes: typeof args.max_bytes === "number" ? args.max_bytes : defaultReadBytes,
          ...(typeof args.grep === "string" && args.grep.length > 0 ? { grep: args.grep } : {})
        });
        return {
          path: result.output.filename ?? result.output.id,
          text: result.text,
          truncated: result.truncated,
          totalBytes: result.totalBytes
        };
      }
      case "search_outputs": {
        const sessionIds = corpus ? await corpus.ids() : undefined;
        const page = await client.sessions.searchOutputs({
          ...(sessionIds ? { runIds: sessionIds } : {}),
          ...(typeof args.filename === "string" ? { filename: args.filename } : {}),
          ...(typeof args.extension === "string" ? { extension: args.extension } : {}),
          ...(typeof args.content_type === "string" ? { contentType: args.content_type } : {}),
          ...(typeof args.limit === "number" ? { limit: args.limit } : {})
        });
        // The wire hit carries `runId`; expose it to the model as `session_id`.
        return {
          hits: page.hits.map((hit) => ({
            session_id: hit.runId,
            outputId: hit.outputId,
            ...(hit.filename !== undefined ? { filename: hit.filename } : {}),
            ...(hit.sizeBytes !== undefined ? { sizeBytes: hit.sizeBytes } : {}),
            ...(hit.contentType !== undefined ? { contentType: hit.contentType } : {})
          }))
        };
      }
      default:
        throw new DataToolError(`unknown tool: ${name}`);
    }
  };
}

/** Internal corpus allow-list, resolved lazily on first tool dispatch. */
interface CorpusGuard {
  ensure(sessionId: string): Promise<void>;
  ids(): Promise<readonly string[]>;
  listSessionSummaries(): Promise<unknown[]>;
}

/**
 * Build the data-source chat tool set bound to one {@link Aex}.
 * Everything the tools can reach is scoped to the client's workspace token.
 */
export function createDataTools(client: Aex, options?: CreateDataToolsOptions): DataTools {
  const defaultReadBytes = options?.defaultReadBytes ?? DEFAULT_READ_BYTES;
  return {
    tools: buildToolDefs(defaultReadBytes),
    instructions: DATA_TOOLS_INSTRUCTIONS,
    execute: makeExecute(client, defaultReadBytes)
  };
}

/**
 * Build a corpus-scoped variant of {@link createDataTools}: identical tools, but
 * every read tool is fenced to the sessions in `corpus`. A `get_session`/
 * `list_outputs`/`read_output` for a session outside the corpus throws a
 * {@link DataToolError} ("session <id> is not in this chat's corpus");
 * `list_sessions` returns only corpus sessions; `search_outputs` is auto-scoped
 * to the corpus. This is a client-side guard on top of the server-side
 * workspace-token data scope.
 */
export function createCorpusTools(
  client: Aex,
  corpus: ChatCorpus,
  options?: CreateDataToolsOptions
): DataTools {
  const defaultReadBytes = options?.defaultReadBytes ?? DEFAULT_READ_BYTES;

  let resolved: Set<string> | null = corpus.sessionIds ? new Set(corpus.sessionIds) : null;
  async function resolve(): Promise<Set<string>> {
    if (resolved) return resolved;
    // No explicit sessionIds → page sessions.list with the filter to a concrete set.
    const ids: string[] = [];
    const base: SessionListQuery = corpus.filter ?? {};
    let cursor: string | undefined = base.cursor;
    do {
      const page = await client.sessions.list({ ...base, ...(cursor ? { cursor } : {}) });
      for (const s of page.sessions) ids.push(s.id);
      cursor = page.nextCursor;
    } while (cursor);
    resolved = new Set(ids);
    return resolved;
  }

  const guard: CorpusGuard = {
    ensure: async (sessionId) => {
      const set = await resolve();
      if (!set.has(sessionId)) throw new DataToolError(`session ${sessionId} is not in this chat's corpus`);
    },
    ids: async () => [...(await resolve())],
    // Build list_sessions rows from sessions.get so the sessionIds corpus never
    // needs the owner-gated sessions.list; bounded by the corpus size.
    listSessionSummaries: async () => {
      const set = await resolve();
      return Promise.all([...set].map(async (id) => summarizeSession(await client.sessions.get(id))));
    }
  };

  return {
    tools: buildToolDefs(defaultReadBytes),
    instructions: DATA_TOOLS_INSTRUCTIONS,
    execute: makeExecute(client, defaultReadBytes, guard)
  };
}

function requireString(value: unknown, field: string): string {
  if (typeof value !== "string" || value.length === 0) {
    throw new DataToolError(`missing required string argument: ${field}`);
  }
  return value;
}

function readSelector(args: Record<string, unknown>): OutputFileSelector {
  if (typeof args.path === "string" && args.path.length > 0) {
    return { path: args.path, match: "suffix" };
  }
  if (typeof args.id === "string" && args.id.length > 0) {
    return { id: args.id };
  }
  throw new DataToolError("read_output requires either `path` or `id`");
}

/** Pick the small, useful, non-sensitive subset of a session record for the model. */
function summarizeSession(session: {
  readonly id: string;
  readonly status: string;
  readonly createdAt?: string;
  readonly updatedAt?: string;
  readonly costUsd?: number;
  readonly errorMessage?: string | null;
}): Record<string, unknown> {
  return {
    id: session.id,
    status: session.status,
    createdAt: session.createdAt,
    updatedAt: session.updatedAt,
    costUsd: session.costUsd,
    errorMessage: session.errorMessage ?? undefined
  };
}
