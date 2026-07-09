/**
 * SDK feature tour: one managed session that uses typed model/runtime constants,
 * inline AGENTS.md guidance, uploaded files, a custom tool bundle, selected
 * built-in tools, runtime env vars/secrets, streamed events, output reads, and
 * a follow-up session turn.
 *
 * SessionRecord from the repository root after building the workspace package:
 *
 *   AEX_API_KEY=... DEEPSEEK_API_KEY=... bun packages/sdk/examples/feature-tour.ts
 *
 * Optional:
 *
 *   AEX_API_URL=https://api.aex.dev
 *   AEX_FEATURE_TOUR_DOWNLOAD=./feature-tour-session.zip
 *   AEX_DEMO_RUNTIME_SECRET=...       # mounted as a runtime secret; never printed
 *   AEX_DEMO_MCP_URL=https://...      # declares an optional remote MCP server
 *   AEX_DEMO_MCP_TOKEN=...            # optional bearer auth for that MCP server
 */
import {
  AgentsMd,
  Aex,
  BuiltinTools,
  File,
  isRateLimited,
  McpServer,
  Models,
  Providers,
  Secret,
  Sizes,
  Tool
} from "@aexhq/sdk";

process.on("uncaughtException", handleFatal);
process.on("unhandledRejection", handleFatal);

const apiKey = required("AEX_API_KEY");
const deepseekKey = required("DEEPSEEK_API_KEY");
const apiUrl = process.env.AEX_API_URL;
const demoMcpUrl = process.env.AEX_DEMO_MCP_URL;
const demoMcpToken = process.env.AEX_DEMO_MCP_TOKEN;
const demoRuntimeSecret = process.env.AEX_DEMO_RUNTIME_SECRET;
const downloadPath = process.env.AEX_FEATURE_TOUR_DOWNLOAD;
const textEncoder = new TextEncoder();

const aex = new Aex({
  apiKey,
  ...(apiUrl ? { baseUrl: apiUrl } : {}),
  retry: {
    maxAttempts: 4,
    initialDelayMs: 500,
    maxDelayMs: 10_000,
    maxElapsedMs: 90_000
  }
});

const metricLookup = await Tool.fromFiles({
  name: "metric_lookup",
  description: "Looks up normalized demo metrics for one product line.",
  inputSchema: {
    type: "object",
    additionalProperties: false,
    properties: {
      product: {
        type: "string",
        enum: ["atlas", "beacon", "cinder"],
        description: "Product line to inspect."
      }
    },
    required: ["product"]
  },
  entry: "index.js",
  files: {
    "index.js": `
const DATA = {
  atlas: { customerCount: 118, activationHealthPct: 72.4, supportTickets: 11 },
  beacon: { customerCount: 74, activationHealthPct: 65.1, supportTickets: 19 },
  cinder: { customerCount: 43, activationHealthPct: 58.8, supportTickets: 7 }
};

export default async function ({ input }) {
  const key = String(input.product ?? "").toLowerCase();
  const row = DATA[key];
  if (!row) {
    return { content: [{ type: "text", text: \`unknown product: \${key}\` }], is_error: true };
  }
  return { content: [{ type: "text", text: JSON.stringify({ product: key, ...row }) }] };
}
`
  }
});

const demoCsv = [
  "product,region,q1_revenue_usd,q2_revenue_usd,activation_rate",
  "atlas,na,120000,139500,0.84",
  "beacon,emea,98000,104300,0.79",
  "cinder,apac,67000,81000,0.91"
].join("\n");

const attachedFile = await File.fromBytes({
  name: "quarterly-metrics.csv",
  bytes: textEncoder.encode(`${demoCsv}\n`),
  mountPath: "/workspace/input"
});

const runRules = await AgentsMd.fromContent(
  [
    "# Feature tour rules",
    "- Use `/workspace/input/quarterly-metrics.csv` as the source table.",
    "- Call `metric_lookup` for atlas, beacon, and cinder before writing conclusions.",
    "- Write final artifacts under `/workspace/outputs`.",
    "- Never print runtime secret values or provider keys."
  ].join("\n"),
  { name: "feature-tour-rules" }
);

const mcpServers = demoMcpUrl
  ? [
      McpServer.remote({
        name: "demo-mcp",
        url: demoMcpUrl,
        ...(demoMcpToken
          ? { headers: { Authorization: `Bearer ${demoMcpToken}` } }
          : {})
      })
    ]
  : [];

const environmentSecrets = demoRuntimeSecret
  ? { DEMO_RUNTIME_SECRET: Secret.value(demoRuntimeSecret) }
  : undefined;

console.log("creating feature-tour session...");
console.log(`optional mcp: ${mcpServers.length > 0 ? "enabled" : "disabled"}`);
console.log(`optional runtime secret: ${environmentSecrets ? "enabled" : "disabled"}`);

const session = await aex.openSession({
  provider: Providers.DEEPSEEK,
  model: Models.DEEPSEEK_V4_FLASH,
  system: [
    "You are a concise analytics agent.",
    "Prefer exact calculations and write durable files for the caller."
  ].join(" "),
  agentsMd: [runRules],
  files: [attachedFile],
  includeBuiltinTools: false,
  tools: [
    BuiltinTools.read_file,
    BuiltinTools.write_file,
    BuiltinTools.bash,
    BuiltinTools.grep,
    BuiltinTools.code_execution,
    metricLookup
  ],
  mcpServers,
  environment: {
    networking: { mode: "open" },
    variables: {
      FEATURE_TOUR: "true",
      REPORT_DIR: "/workspace/outputs"
    },
    ...(environmentSecrets ? { secrets: environmentSecrets } : {})
  },
  outputs: {
    allowedDirs: ["/workspace/outputs"],
    deniedDirs: ["*.tmp"],
    maxFiles: 10,
    maxFileBytes: 1_000_000
  },
  outputMode: "stream",
  runtime: Sizes.SHARED_0_25X_1GB,
  metadata: {
    example: "sdk-feature-tour",
    sdkSurface: "public"
  },
  overrides: {
    idleTtl: "5m",
    timeout: "10m",
    maxSpendUsd: 2
  },
  apiKeys: { deepseek: deepseekKey }
});

console.log(`session: ${session.id}`);

const prompt = [
  "Analyze the attached quarterly metrics.",
  "Call metric_lookup for atlas, beacon, and cinder.",
  "Create /workspace/outputs/feature-tour-report.md with a short table, a ranking by q2_revenue_usd, and two risks.",
  "Create /workspace/outputs/summary.json with keys topProduct, totalQ2RevenueUsd, highestActivationProduct, and riskCount."
].join(" ");

const firstTurn = session.send(prompt);
const firstTurnIterator = firstTurn[Symbol.asyncIterator]();
let result: Awaited<ReturnType<typeof firstTurn.done>> | undefined;
for (;;) {
  const next = await firstTurnIterator.next();
  if (next.done) {
    result = next.value as Awaited<ReturnType<typeof firstTurn.done>>;
    break;
  }
  const event = next.value;
  if (event.isTextMessage()) {
    process.stdout.write(event.data.text);
  } else if (event.isToolCallStart()) {
    process.stdout.write(`\n[tool:start] ${event.data.name}\n`);
  } else if (event.isToolCallResult()) {
    process.stdout.write("[tool:result]\n");
  }
}

if (!result) {
  throw new Error("first turn stream ended without a result");
}
console.log(`\nfirst turn parked with status: ${result.status}`);

const followUp = await session
  .send("Read summary.json back and answer with one sentence confirming the top product and total Q2 revenue.")
  .done();
console.log(`follow-up status: ${followUp.status}`);
if (followUp.text) {
  console.log(`follow-up text: ${followUp.text.trim()}`);
}

const parked = await session.wait({ timeoutMs: 60_000, intervalMs: 2_000 });
console.log(`settled session status: ${parked.status}`);

const messages = await session.messages().list();
console.log(`assistant messages: ${messages.length}`);

const events = await session.events().list();
console.log(`captured events: ${events.length}`);

const outputs = await session.outputs().list();
console.log("outputs:");
for (const output of outputs) {
  console.log(`- ${output.filename ?? output.id} (${output.contentType ?? "unknown"})`);
}

const summary = await session.outputs().read(
  { path: "summary.json", match: "suffix" },
  { maxBytes: 20_000 }
);
console.log("summary.json:");
console.log(summary.text);

const report = await session.outputs().findOne({
  filename: "feature-tour-report.md"
});
if (report) {
  const reportPreview = await session.outputs().read(report, {
    maxBytes: 4_000,
    grep: "risk"
  });
  console.log("report risk lines:");
  console.log(reportPreview.text || "(no risk lines found)");
}

const reopenedOutputs = await aex.sessions.outputs(session.id).list();
console.log(`outputs via aex.sessions.outputs(...): ${reopenedOutputs.length}`);

if (downloadPath) {
  const bytes = await session.download({ to: downloadPath });
  console.log(`downloaded session archive: ${downloadPath} (${bytes.byteLength} bytes)`);
}

function required(name: string): string {
  const value = process.env[name];
  if (!value) {
    console.error(`Missing env var ${name}`);
    process.exit(1);
  }
  return value;
}

function handleFatal(err: unknown): void {
  if (isRateLimited(err)) {
    console.error(`rate limited after ${err.attempts} attempts; retry after ${err.retryAfterMs ?? "unknown"}ms`);
    process.exit(1);
  }
  console.error(err instanceof Error ? err.stack ?? err.message : String(err));
  process.exit(1);
}
