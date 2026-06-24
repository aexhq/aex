/**
 * Data-source chat tools — turn the SDK's read surface into LLM tool definitions.
 *
 * `createDataTools(client)` returns a small, PROVIDER-AGNOSTIC tool set (plain
 * JSON-Schema `input_schema`, the Anthropic/OpenAI tool shape) plus an
 * `execute(name, input)` dispatcher. A chat backend wires `tools` into its
 * Messages loop and calls `execute` on each `tool_use`. There is no LLM-vendor
 * dependency here: the executor only ever calls public `AgentExecutor` methods,
 * so a chat built on these tools can reach a workspace's runs and outputs and
 * NOTHING else — internal/operator data is unreachable by construction.
 *
 * The tools follow search-then-fetch: `list_runs` / `list_outputs` return lean
 * references and metadata; only `read_output` returns file content, and it is
 * byte-capped so a large deliverable never floods the context window.
 */
import type { AgentExecutor } from "./client.js";
import type { OutputFileSelector } from "./client.js";

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

/** Thrown by `DataTools.execute` for an unknown tool or malformed arguments. */
export class DataToolError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "DataToolError";
  }
}

const DEFAULT_READ_BYTES = 50_000;

export const DATA_TOOLS_INSTRUCTIONS =
  "You can read this workspace's agent runs and their output files. " +
  "Call `list_runs` to discover runs (most recent first; page with `cursor`). " +
  "Call `list_outputs` for a run to see its files. Only call `read_output` for a " +
  "specific file when you actually need its contents — it is byte-capped, so if a " +
  "read comes back `truncated`, narrow it with `grep` or a more specific `path` " +
  "rather than re-reading. Never assume a run or file exists without listing first.";

/**
 * Build the data-source chat tool set bound to one {@link AgentExecutor}.
 * Everything the tools can reach is scoped to the client's workspace token.
 */
export function createDataTools(client: AgentExecutor, options?: CreateDataToolsOptions): DataTools {
  const defaultReadBytes = options?.defaultReadBytes ?? DEFAULT_READ_BYTES;

  const tools: readonly DataChatTool[] = [
    {
      name: "list_runs",
      description:
        "List agent runs in the workspace, most recent first. Returns id/status/timestamps " +
        "(and cost when settled) plus a `nextCursor` for the next page. Does NOT return prompts or outputs.",
      input_schema: {
        type: "object",
        additionalProperties: false,
        properties: {
          status: { type: "string", description: "Filter by run status, e.g. 'succeeded'." },
          since: { type: "string", description: "ISO-8601 lower bound on createdAt." },
          limit: { type: "integer", description: "Page size, 1-100 (default 25)." },
          cursor: { type: "string", description: "Opaque cursor from a prior page's nextCursor." }
        }
      }
    },
    {
      name: "get_run",
      description: "Get one run's status, timing, and cost summary by id. Does NOT return the submission or outputs.",
      input_schema: {
        type: "object",
        additionalProperties: false,
        required: ["run_id"],
        properties: { run_id: { type: "string", description: "The run id." } }
      }
    },
    {
      name: "list_outputs",
      description: "List a run's captured output files (id, filename, size, content type). Metadata only, no content.",
      input_schema: {
        type: "object",
        additionalProperties: false,
        required: ["run_id"],
        properties: { run_id: { type: "string", description: "The run id." } }
      }
    },
    {
      name: "read_output",
      description:
        "Read one output file of a run as text. Byte-capped: if the file is larger than `max_bytes` the result " +
        "is a prefix with `truncated: true`. Select the file by `path` (e.g. 'report.md') or by `id` from list_outputs.",
      input_schema: {
        type: "object",
        additionalProperties: false,
        required: ["run_id"],
        properties: {
          run_id: { type: "string", description: "The run id." },
          path: { type: "string", description: "Output file path/name (suffix match). Provide this or `id`." },
          id: { type: "string", description: "Output file id from list_outputs. Provide this or `path`." },
          max_bytes: { type: "integer", description: `Cap on bytes read (default ${defaultReadBytes}, max 10000000).` },
          grep: { type: "string", description: "Optional: return only lines matching this substring (case-insensitive)." }
        }
      }
    }
  ];

  async function execute(name: string, input: Record<string, unknown>): Promise<unknown> {
    const args = input ?? {};
    switch (name) {
      case "list_runs": {
        const page = await client.listRuns({
          ...(typeof args.status === "string" ? { status: args.status } : {}),
          ...(typeof args.since === "string" ? { since: args.since } : {}),
          ...(typeof args.limit === "number" ? { limit: args.limit } : {}),
          ...(typeof args.cursor === "string" ? { cursor: args.cursor } : {})
        });
        return page;
      }
      case "get_run": {
        const runId = requireString(args.run_id, "run_id");
        const run = await client.getRun(runId);
        return summarizeRun(run);
      }
      case "list_outputs": {
        const runId = requireString(args.run_id, "run_id");
        const outputs = await client.listOutputs(runId);
        return outputs.map((o) => ({
          id: o.id,
          filename: o.filename,
          sizeBytes: o.sizeBytes,
          contentType: o.contentType
        }));
      }
      case "read_output": {
        const runId = requireString(args.run_id, "run_id");
        const selector = readSelector(args);
        const result = await client.readOutputText(runId, selector, {
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
      default:
        throw new DataToolError(`unknown tool: ${name}`);
    }
  }

  return { tools, instructions: DATA_TOOLS_INSTRUCTIONS, execute };
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

/** Pick the small, useful, non-sensitive subset of a run record for the model. */
function summarizeRun(run: {
  readonly id: string;
  readonly status: string;
  readonly createdAt?: string;
  readonly startedAt?: string;
  readonly terminalAt?: string | null;
  readonly errorMessage?: string | null;
  readonly costTelemetry?: { readonly billedCostUsd?: number };
}): Record<string, unknown> {
  return {
    id: run.id,
    status: run.status,
    createdAt: run.createdAt,
    startedAt: run.startedAt,
    terminalAt: run.terminalAt ?? undefined,
    errorMessage: run.errorMessage ?? undefined,
    costUsd: run.costTelemetry?.billedCostUsd
  };
}
