/**
 * `aex chat` — read-only, multi-run chat over a CORPUS of runs (chat-mvp).
 *
 * A thin direct-Claude loop: corpus-scoped read tools (list_runs / get_run /
 * list_outputs / read_output / search_outputs) backed by `@aexhq/contracts`
 * operations, driven by `@anthropic-ai/sdk`. The model answers ONLY from the
 * named runs' outputs; a run outside the corpus is refused by the tool layer.
 *
 * `@anthropic-ai/sdk` is a CLI devDependency, esbuild-inlined into the shipped
 * bundle — it is NOT a runtime dep of the importable `@aexhq/sdk`. (The CLI is
 * bundled *into* `@aexhq/sdk`, so it cannot import `@aexhq/sdk`; the corpus tool
 * dispatch here mirrors the SDK's `createCorpusTools` over `operations`.)
 *
 * BYOK: `--anthropic-api-key` is never stored or logged. One-shot only (`--prompt`);
 * the interactive REPL is deferred (the CLI IO surface has no stdin reader).
 */
import Anthropic from "@anthropic-ai/sdk";
import { HttpClient, operations, type OutputQuery, type Run } from "@aexhq/contracts";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  USAGE_ERR,
  collectRepeated,
  collectRepeatedKv,
  describeApiError,
  emitJsonError,
  makeHttpClient,
  refuseInsideManagedRun,
  resolveCommonHostFlags,
  takeFlagValue
} from "./common.js";

const DEFAULT_MODEL = "claude-opus-4-8";
const DEFAULT_READ_BYTES = 50_000;
const MAX_TURNS = 16;

export async function runChatCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "chat")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  let rest = common.rest;

  const runs = collectRepeated(rest, "--run");
  if (runs.error) { io.stderr(`${runs.error}\n`); return USAGE_ERR; }
  rest = runs.remaining;

  const keyFlag = takeFlagValue(rest, "--anthropic-api-key");
  if (keyFlag.error) { io.stderr(`${keyFlag.error}\n`); return USAGE_ERR; }
  rest = keyFlag.remaining;

  const modelFlag = takeFlagValue(rest, "--model");
  if (modelFlag.error) { io.stderr(`${modelFlag.error}\n`); return USAGE_ERR; }
  rest = modelFlag.remaining;

  const promptFlag = takeFlagValue(rest, "--prompt");
  if (promptFlag.error) { io.stderr(`${promptFlag.error}\n`); return USAGE_ERR; }
  rest = promptFlag.remaining;

  const mcp = collectRepeatedKv(rest, "--mcp");
  if (mcp.error) { io.stderr(`${mcp.error}\n`); return USAGE_ERR; }
  rest = mcp.remaining;

  const mcpToken = collectRepeatedKv(rest, "--mcp-token");
  if (mcpToken.error) { io.stderr(`${mcpToken.error}\n`); return USAGE_ERR; }
  rest = mcpToken.remaining;

  const stray = rest.filter((a) => a.startsWith("--"));
  if (stray.length > 0) { io.stderr(`unknown flag: ${stray[0]}\n`); return USAGE_ERR; }

  const corpus = runs.values;
  if (corpus.length === 0) {
    io.stderr("aex chat requires at least one --run <runId> (the corpus to chat over)\n");
    return USAGE_ERR;
  }
  if (!keyFlag.value) {
    io.stderr("--anthropic-api-key is required (BYOK; never stored)\n");
    return USAGE_ERR;
  }
  if (!promptFlag.value) {
    io.stderr("--prompt <text> is required (one-shot; interactive REPL not yet supported)\n");
    return USAGE_ERR;
  }

  const debug = common.flags.debug;
  const log = (obj: Record<string, unknown>): void => {
    if (debug) io.stderr(JSON.stringify(obj) + "\n");
  };

  const http = makeHttpClient(io, common.flags);
  const allow = new Set(corpus);
  log({ event: "corpus.resolved", source: "runIds", runCount: corpus.length, sampleIds: corpus.slice(0, 5) });

  const tools = corpusTools();
  const execute = makeExecutor(io, http, allow, log);

  const anthropic = new Anthropic({ apiKey: keyFlag.value });
  const model = modelFlag.value ?? DEFAULT_MODEL;

  // Remote MCP (optional): every server must be referenced by exactly one toolset.
  const useMcp = mcp.entries && Object.keys(mcp.entries).length > 0;
  const mcpServers = Object.entries(mcp.entries).map(([name, url]) => ({
    type: "url" as const,
    name,
    url,
    ...(mcpToken.entries[name] ? { authorization_token: mcpToken.entries[name] } : {})
  }));
  for (const s of mcpServers) log({ event: "mcp.attach", name: s.name, url: safeHost(s.url) });

  const system =
    "You answer questions about a fixed set of aex agent runs using the provided tools. " +
    "Search-then-fetch: list outputs, then read only the files you need. " +
    "Never assume a run or file exists without listing first.";

  const messages: Anthropic.MessageParam[] = [{ role: "user", content: promptFlag.value }];

  try {
    for (let turn = 0; turn < MAX_TURNS; turn++) {
      log({ event: "chat.request", turn, model, messageCount: messages.length, toolCount: tools.length, mcpServers: mcpServers.length });
      const response = await runOneTurn(anthropic, {
        model,
        system,
        tools,
        messages,
        ...(useMcp ? { mcpServers } : {})
      });
      messages.push({ role: "assistant", content: response.content });
      const usage = response.usage as { input_tokens?: number; output_tokens?: number; cache_read_input_tokens?: number } | undefined;
      log({ event: "chat.response", turn, stopReason: response.stop_reason, usage });

      if (response.stop_reason === "refusal") {
        io.stderr(`\n(refused) ${(response.stop_details as { category?: string } | null)?.category ?? ""}\n`);
        return SUCCESS;
      }
      if (response.stop_reason === "end_turn" || response.stop_reason === "max_tokens") {
        io.stdout("\n");
        return SUCCESS;
      }
      if (response.stop_reason === "pause_turn") continue;
      if (response.stop_reason !== "tool_use") {
        io.stdout("\n");
        return SUCCESS;
      }

      const toolResults: Anthropic.ToolResultBlockParam[] = [];
      for (const block of response.content) {
        if (block.type !== "tool_use") continue;
        const started = Date.now();
        try {
          const result = await execute(block.name, (block.input ?? {}) as Record<string, unknown>);
          log({ event: "tool.result", name: block.name, ok: true, ms: Date.now() - started });
          toolResults.push({ type: "tool_result", tool_use_id: block.id, content: JSON.stringify(result) });
        } catch (err) {
          const message = err instanceof Error ? err.message : String(err);
          log({ event: "tool.error", name: block.name, message });
          toolResults.push({ type: "tool_result", tool_use_id: block.id, content: message, is_error: true });
        }
      }
      messages.push({ role: "user", content: toolResults });
    }
    io.stderr("\n(chat reached the max turn limit)\n");
    return SUCCESS;
  } catch (err) {
    const d = describeApiError(err);
    return emitJsonError(io, "chat_failed", d.message, {
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }
}

interface TurnParams {
  readonly model: string;
  readonly system: string;
  readonly tools: readonly Anthropic.Tool[];
  readonly messages: readonly Anthropic.MessageParam[];
  readonly mcpServers?: ReadonlyArray<{ type: "url"; name: string; url: string; authorization_token?: string }>;
}

/** One Claude turn. Streams assistant text to stdout, returns the final message. */
async function runOneTurn(anthropic: Anthropic, p: TurnParams): Promise<Anthropic.Message> {
  // One cache_control breakpoint on the last tool so tools + system cache together.
  const tools = p.tools.map((t, i) =>
    i === p.tools.length - 1 ? { ...t, cache_control: { type: "ephemeral" as const } } : t
  );
  const base = {
    model: p.model,
    max_tokens: 8192,
    thinking: { type: "adaptive" as const },
    system: [{ type: "text" as const, text: p.system }],
    tools,
    messages: p.messages as Anthropic.MessageParam[]
  };
  if (p.mcpServers && p.mcpServers.length > 0) {
    // Remote MCP connector: declare servers + a matching mcp_toolset per server.
    const beta = anthropic.beta.messages.stream({
      ...base,
      betas: ["mcp-client-2025-11-20"],
      mcp_servers: p.mcpServers.map((s) => ({
        type: "url",
        name: s.name,
        url: s.url,
        ...(s.authorization_token ? { authorization_token: s.authorization_token } : {})
      })),
      tools: [
        ...tools,
        ...p.mcpServers.map((s) => ({ type: "mcp_toolset" as const, mcp_server_name: s.name }))
      ]
    } as Anthropic.Beta.Messages.MessageCreateParamsStreaming);
    beta.on("text", (delta) => process.stdout.write(delta));
    return (await beta.finalMessage()) as unknown as Anthropic.Message;
  }
  const stream = anthropic.messages.stream(base);
  stream.on("text", (delta) => process.stdout.write(delta));
  return stream.finalMessage();
}

function corpusTools(): readonly Anthropic.Tool[] {
  return [
    {
      name: "list_runs",
      description: "List the runs in this chat's corpus (id/status/timestamps + cost when settled). No prompts or outputs.",
      input_schema: { type: "object", additionalProperties: false, properties: {} }
    },
    {
      name: "get_run",
      description: "Get one corpus run's status, timing, and cost summary by id.",
      input_schema: { type: "object", additionalProperties: false, required: ["run_id"], properties: { run_id: { type: "string" } } }
    },
    {
      name: "list_outputs",
      description: "List a corpus run's captured output files (id, filename, size, content type). Metadata only.",
      input_schema: { type: "object", additionalProperties: false, required: ["run_id"], properties: { run_id: { type: "string" } } }
    },
    {
      name: "read_output",
      description:
        "Read one output file of a corpus run as text. Byte-capped (truncated:true when larger). " +
        "Select by `path` (suffix) or `id`.",
      input_schema: {
        type: "object",
        additionalProperties: false,
        required: ["run_id"],
        properties: {
          run_id: { type: "string" },
          path: { type: "string" },
          id: { type: "string" },
          max_bytes: { type: "integer" },
          grep: { type: "string" }
        }
      }
    },
    {
      name: "search_outputs",
      description: "Find output files across the corpus by filename/extension/content type. Returns references — read_output to fetch.",
      input_schema: {
        type: "object",
        additionalProperties: false,
        properties: {
          filename: { type: "string" },
          extension: { type: "string" },
          content_type: { type: "string" },
          limit: { type: "integer" }
        }
      }
    }
  ];
}

function makeExecutor(
  io: CliIO,
  http: HttpClient,
  allow: ReadonlySet<string>,
  log: (obj: Record<string, unknown>) => void
): (name: string, input: Record<string, unknown>) => Promise<unknown> {
  const ensure = (runId: string): void => {
    if (!allow.has(runId)) {
      log({ event: "corpus.reject", runId });
      throw new Error(`run ${runId} is not in this chat's corpus`);
    }
  };
  void io;
  return async (name, input) => {
    switch (name) {
      case "list_runs":
        return { runs: await Promise.all([...allow].map(async (id) => summarizeRun(await operations.getRun(http, id)))) };
      case "get_run": {
        const runId = requireString(input.run_id, "run_id");
        ensure(runId);
        return summarizeRun(await operations.getRun(http, runId));
      }
      case "list_outputs": {
        const runId = requireString(input.run_id, "run_id");
        ensure(runId);
        const outputs = await operations.listOutputs(http, runId);
        return outputs.map((o) => ({ id: o.id, filename: o.filename, sizeBytes: o.sizeBytes, contentType: o.contentType }));
      }
      case "read_output": {
        const runId = requireString(input.run_id, "run_id");
        ensure(runId);
        const selector =
          typeof input.path === "string" && input.path.length > 0
            ? { path: input.path, match: "suffix" as const }
            : typeof input.id === "string" && input.id.length > 0
              ? { id: input.id }
              : null;
        if (!selector) throw new Error("read_output requires either `path` or `id`");
        const result = await operations.readOutputText(http, runId, selector, {
          maxBytes: typeof input.max_bytes === "number" ? input.max_bytes : DEFAULT_READ_BYTES,
          ...(typeof input.grep === "string" && input.grep.length > 0 ? { grep: input.grep } : {})
        });
        return { path: result.output.filename ?? result.output.id, text: result.text, truncated: result.truncated, totalBytes: result.totalBytes };
      }
      case "search_outputs": {
        const limit = typeof input.limit === "number" ? input.limit : 100;
        const query: OutputQuery = {
          ...(typeof input.filename === "string" ? { filename: new RegExp(escapeRegExp(input.filename), "i") } : {}),
          ...(typeof input.extension === "string" ? { extension: input.extension } : {}),
          ...(typeof input.content_type === "string" ? { contentType: input.content_type } : {})
        };
        const hasFilter = Object.keys(query).length > 0;
        const hits: Array<Record<string, unknown>> = [];
        for (const runId of allow) {
          const outputs = hasFilter ? await operations.listOutputs(http, runId, query) : await operations.listOutputs(http, runId);
          for (const o of outputs) {
            hits.push({ runId, outputId: o.id, filename: o.filename, sizeBytes: o.sizeBytes, contentType: o.contentType });
            if (hits.length >= limit) return { hits };
          }
        }
        return { hits };
      }
      default:
        throw new Error(`unknown tool: ${name}`);
    }
  };
}

function summarizeRun(run: Run): Record<string, unknown> {
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

function requireString(value: unknown, field: string): string {
  if (typeof value !== "string" || value.length === 0) throw new Error(`missing required string argument: ${field}`);
  return value;
}

function escapeRegExp(input: string): string {
  return input.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function safeHost(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return "(invalid url)";
  }
}
