/**
 * Public SDK feature tour: publish reusable resources, run a resumable session,
 * stream one run, read its committed checkpoint, and continue the thread.
 */
import {
  Aex,
  BuiltinTools,
  File,
  Instructions,
  Models,
  Providers,
  Sizes,
  Tool,
  isRateLimited
} from "@aexhq/sdk";

process.on("uncaughtException", handleFatal);
process.on("unhandledRejection", handleFatal);

const aex = new Aex({
  apiKey: required("AEX_API_KEY"),
  ...(process.env.AEX_API_URL ? { baseUrl: process.env.AEX_API_URL } : {})
});
const deepseekKey = required("DEEPSEEK_API_KEY");

const csv = [
  "product,q2_revenue_usd",
  "atlas,139500",
  "beacon,104300",
  "cinder,81000"
].join("\n");

const fileDraft = await File.fromBytes({
  name: "quarterly-metrics.csv",
  bytes: new TextEncoder().encode(`${csv}\n`),
  mountPath: "/workspace/input"
});
const instructionDraft = await Instructions.fromContent(
  "Read the supplied data, use exact arithmetic, and write final files under /workspace/files.",
  { name: "feature-tour-rules" }
);
const toolDraft = await Tool.fromFiles({
  name: "normalize_product",
  description: "Normalizes a product name.",
  inputSchema: {
    type: "object",
    properties: { product: { type: "string" } },
    required: ["product"]
  },
  entry: "index.js",
  files: {
    "index.js": "export default async ({ input }) => ({ content: [{ type: 'text', text: String(input.product).trim().toLowerCase() }] });"
  }
});

const [input, rules, normalizeProduct] = await Promise.all([
  aex.workspace.files.publish(fileDraft),
  aex.workspace.instructions.publish(instructionDraft),
  aex.workspace.tools.publish(toolDraft)
]);

const session = await aex.sessions.create({
  provider: Providers.DEEPSEEK,
  model: Models.DEEPSEEK_V4_FLASH,
  system: "You are a concise analytics agent.",
  assets: {
    files: [input],
    instructions: [rules],
    tools: [normalizeProduct]
  },
  builtinTools: [BuiltinTools.read_file, BuiltinTools.write_file, BuiltinTools.code_execution],
  fileCapture: { allowedDirs: ["/workspace/files"], maxFiles: 10 },
  runtime: Sizes.SHARED_0_25X_1GB,
  overrides: { idleTtl: "5m", timeout: "10m", maxSpendUsd: 2 },
  apiKeys: { deepseek: deepseekKey }
});

console.log(`session: ${session.id}`);
const run = session.messages.send(
  "Read /workspace/input/quarterly-metrics.csv, rank products, and write /workspace/files/report.md."
);

for await (const event of run) {
  if (event.isTextMessage()) process.stdout.write(event.data.text);
  if (event.isToolCallStart()) process.stdout.write(`\n[tool] ${event.data.name}\n`);
}

const first = await run.finished();
console.log(`\noutcome: ${first.status}; cost: $${first.costUsd}`);
console.log(`checkpoint: ${first.checkpoint?.checkpointId ?? "none"}`);

const snapshot = await session.files.list();
console.log(`captured files: ${snapshot.files.length}`);
const report = await session.files.read({ path: "report.md", match: "suffix" });
console.log(report.text);

const followUp = await session.messages
  .send("Confirm the top product in one sentence.")
  .finished();
console.log(`follow-up: ${followUp.status} ${followUp.text.trim()}`);

const reopened = await aex.sessions.open(session.id);
const reopenedSnapshot = await reopened.files.list();
console.log(`reopened checkpoint: ${reopenedSnapshot.revision.checkpointId}`);

if (process.env.AEX_FEATURE_TOUR_DOWNLOAD) {
  const bytes = await reopened.download({ to: process.env.AEX_FEATURE_TOUR_DOWNLOAD });
  console.log(`archive bytes: ${bytes.byteLength}`);
}

function required(name: string): string {
  const value = process.env[name];
  if (!value) throw new Error(`Missing env var ${name}`);
  return value;
}

function handleFatal(error: unknown): void {
  if (isRateLimited(error)) {
    console.error(`rate limited; retry after ${error.retryAfterMs ?? "unknown"}ms`);
  } else {
    console.error(error instanceof Error ? error.stack ?? error.message : String(error));
  }
  process.exit(1);
}
