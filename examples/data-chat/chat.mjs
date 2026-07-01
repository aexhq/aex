// Interactive chat over your aex workspace data — built ENTIRELY on the public
// SDK. The only aex import is `@aexhq/sdk`; the LLM is your own Anthropic key
// (BYOK). Because the chat can only call `createDataTools(client)` (which only
// calls public read methods), it can reach your sessions and their outputs and
// nothing else — there is no path to internal/operator data.
//
//   AEX_API_TOKEN   = your workspace token (scopes all data access)
//   ANTHROPIC_API_KEY = your Anthropic key (pays for the chat; never leaves here)
//   AEX_BASE_URL    = optional API plane override (defaults to https://api.aex.dev)
//
//   npm install && node chat.mjs
//
import { createInterface } from "node:readline/promises";
import { stdin, stdout } from "node:process";
import { Aex, createDataTools } from "@aexhq/sdk";
import Anthropic from "@anthropic-ai/sdk";

const apiToken = required("AEX_API_TOKEN");
const anthropicKey = required("ANTHROPIC_API_KEY");
const MODEL = process.env.AEX_CHAT_MODEL ?? "claude-sonnet-4-6";

// 1. The aex client — workspace identity comes from the token, server-side.
const client = new Aex({
  apiToken,
  ...(process.env.AEX_BASE_URL ? { baseUrl: process.env.AEX_BASE_URL } : {})
});

// 2. The unified data interface as model tools. `data.execute` only ever calls
//    public read methods (sessions.list / sessions.get / sessions.outputs /
//    sessions.readOutput).
const data = createDataTools(client);

// 3. Your own LLM, your own key.
const anthropic = new Anthropic({ apiKey: anthropicKey });

// Cache the static system + tool prefix (0.1x on hits). Must be byte-identical
// across turns — keep instructions/tool descriptions free of timestamps.
const system = [{ type: "text", text: data.instructions, cache_control: { type: "ephemeral" } }];

const messages = []; // the chat thread; the Messages API is stateless, so we own it.
const rl = createInterface({ input: stdin, output: stdout });
console.log("aex data chat — ask about your runs and outputs. Ctrl-C to exit.\n");

for (;;) {
  const userText = (await rl.question("you › ")).trim();
  if (!userText) continue;
  messages.push({ role: "user", content: userText });
  await runAgentTurn();
  stdout.write("\n");
}

/** One assistant turn: stream text, run any tool calls, loop until end_turn. */
async function runAgentTurn() {
  for (;;) {
    stdout.write("aex › ");
    const stream = anthropic.messages.stream({
      model: MODEL,
      max_tokens: 1024,
      system,
      tools: data.tools,
      messages
    });
    stream.on("text", (t) => stdout.write(t));
    const message = await stream.finalMessage();
    stdout.write("\n");
    messages.push({ role: "assistant", content: message.content });

    const toolUses = message.content.filter((b) => b.type === "tool_use");
    if (toolUses.length === 0) return; // end_turn

    // Search-then-fetch: each tool returns lean references / capped text, never
    // raw bytes, so large deliverables never flood the context window.
    const toolResults = [];
    for (const call of toolUses) {
      try {
        const result = await data.execute(call.name, call.input ?? {});
        toolResults.push({ type: "tool_result", tool_use_id: call.id, content: JSON.stringify(result) });
      } catch (err) {
        toolResults.push({
          type: "tool_result",
          tool_use_id: call.id,
          content: `error: ${err instanceof Error ? err.message : String(err)}`,
          is_error: true
        });
      }
    }
    messages.push({ role: "user", content: toolResults });
  }
}

function required(name) {
  const value = process.env[name];
  if (!value) {
    console.error(`Missing env var ${name}`);
    process.exit(1);
  }
  return value;
}
