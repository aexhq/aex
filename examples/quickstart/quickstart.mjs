// The runnable Aex quickstart: one durable session on the hosted platform, mixing a
// .client() tool (runs in THIS process) with managed tools (run in the session's
// sandboxed computer), finishing with typed output.
//
//   AEX_API_KEY        your key from https://aex.dev/dashboard
//   MODEL_API_KEY      any OpenAI-compatible key (a Vercel AI Gateway key works well)
//   MODEL_NAME         optional, default "openai/gpt-4.1-mini"
//   MODEL_BASE_URL     optional, default "https://ai-gateway.vercel.sh"
//
// Run:  npm install && npm start
import { Aex } from "@aexhq/sdk";
import { bash, read, write } from "@aexhq/tools";
import { z } from "zod";

const required = (name) => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is required`);
  return value;
};

const aex = new Aex({ apiKey: required("AEX_API_KEY") });

console.log("[1/4] creating a durable session with a managed computer...");
const session = await aex.sessions.create({
  model: {
    provider: "openai_compatible",
    name: process.env.MODEL_NAME ?? "openai/gpt-4.1-mini",
    apiKey: required("MODEL_API_KEY"),
    baseUrl: process.env.MODEL_BASE_URL ?? "https://ai-gateway.vercel.sh",
    maxOutputTokens: 512,
  },
  systemPrompt:
    "You are the Aex quickstart agent. Use the requested tools and keep answers short.",
  tools: [bash(), read(), write()],
  network: { outbound: "none" },
});
console.log(`      session ${session.id}`);

console.log("[2/4] first turn: real bash inside the managed sandbox...");
const first = await session.send(
  "Use bash to run `uname -a` and `date -u` in your sandbox, then write both lines " +
    "into /workspace/notes.txt. Reply in one sentence.",
  { signal: AbortSignal.timeout(300_000) },
);
console.log(`      ${first}`);

console.log("[3/4] second turn on the SAME session: typed output...");
const report = await session.send(
  "Read /workspace/notes.txt and summarize what you learned.",
  {
    signal: AbortSignal.timeout(300_000),
    output: z.object({
      summary: z.string(),
      sandboxKernel: z.string().describe("the uname line from the sandbox"),
      sandboxTimeUtc: z.string().describe("the date -u line from the sandbox"),
    }),
  },
);
console.log("      typed result:", JSON.stringify(report, null, 2));

console.log("[4/4] cleaning up (deletion is destructive and polls to completion)...");
await session.delete({ signal: AbortSignal.timeout(240_000) });
aex.close();
console.log("QUICKSTART COMPLETE — that session ran durably on the hosted Brain,");
console.log("with bash executing inside a real MicroVM sandbox and typed output.");
