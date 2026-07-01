/**
 * Reference: a read-only, multi-run chat over a CORPUS of aex runs.
 *
 * Combines the public `@aexhq/sdk` corpus read-tools (`createCorpusTools`) with a
 * direct Claude chat loop (`@anthropic-ai/sdk`). The importable `@aexhq/sdk` stays
 * LLM-vendor-free; the vendor dependency lives only here in the example.
 *
 * Run (Bun): ANTHROPIC_API_KEY=… AEX_TOKEN=… bun examples/chat-corpus.ts <runId> [runId…]
 *
 * The model answers ONLY from the named runs' outputs (read via the corpus
 * tools); a run outside the corpus is refused by the tool layer.
 */
import Anthropic from "@anthropic-ai/sdk";
import { AgentExecutor, createCorpusTools, DataToolError } from "@aexhq/sdk";

const runIds = process.argv.slice(2);
if (runIds.length === 0) {
  console.error("usage: bun examples/chat-corpus.ts <runId> [runId…]");
  process.exit(2);
}

const aex = new AgentExecutor({
  apiToken: process.env.AEX_TOKEN!,
  ...(process.env.AEX_URL ? { baseUrl: process.env.AEX_URL } : {})
});
const tools = createCorpusTools(aex, { runIds });
const anthropic = new Anthropic({ apiKey: process.env.ANTHROPIC_API_KEY! });

const SYSTEM =
  "You answer questions about a fixed set of aex agent runs using the provided tools. " +
  tools.instructions;

// Anthropic wire-shape tool defs: { name, description, input_schema }. The corpus
// tool defs are already in that exact shape. Put one cache_control breakpoint on
// the LAST tool so the (deterministic) tool list + system prefix cache together.
const wireTools = tools.tools.map((t, i) => ({
  name: t.name,
  description: t.description,
  input_schema: t.input_schema,
  ...(i === tools.tools.length - 1 ? { cache_control: { type: "ephemeral" as const } } : {})
}));

const messages: Anthropic.MessageParam[] = [
  {
    role: "user",
    content: "Across these runs, what did they produce and what are the headline findings?"
  }
];

for (let turn = 0; turn < 12; turn++) {
  const stream = anthropic.messages.stream({
    model: process.env.AEX_CHAT_MODEL ?? "claude-opus-4-8",
    max_tokens: 8192,
    thinking: { type: "adaptive" },
    system: [{ type: "text", text: SYSTEM }],
    tools: wireTools,
    messages
  });
  stream.on("text", (delta) => process.stdout.write(delta));
  const response = await stream.finalMessage();
  messages.push({ role: "assistant", content: response.content });

  if (response.stop_reason === "refusal") {
    console.error("\n[refused]", response.stop_details?.category ?? "");
    break;
  }
  if (response.stop_reason === "end_turn" || response.stop_reason === "max_tokens") break;
  if (response.stop_reason === "pause_turn") continue;
  if (response.stop_reason !== "tool_use") break;

  const toolResults: Anthropic.ToolResultBlockParam[] = [];
  for (const block of response.content) {
    if (block.type !== "tool_use") continue;
    try {
      const result = await tools.execute(block.name, block.input as Record<string, unknown>);
      toolResults.push({ type: "tool_result", tool_use_id: block.id, content: JSON.stringify(result) });
    } catch (err) {
      const message = err instanceof DataToolError ? err.message : String(err);
      toolResults.push({ type: "tool_result", tool_use_id: block.id, content: message, is_error: true });
    }
  }
  messages.push({ role: "user", content: toolResults });
}
console.log();
