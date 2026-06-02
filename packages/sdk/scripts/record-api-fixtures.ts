/**
 * `pnpm --filter antpath run fixtures:record:anthropic` — LIVE, creds-gated
 * capture of real Anthropic Managed Agents session events into a RAW recording.
 *
 * It runs a minimal simple-turn against the live beta (create agent →
 * environment → session → send a user turn → poll
 * `GET /v1/sessions/{id}/events` to terminal) and writes the WHOLE captured
 * event objects to `test/fixtures/api-recordings/<scenario>.raw.json`. The raw
 * file is GITIGNORED (`*.raw.json`) because it may contain secrets — only the
 * sanitized output (`fixtures:sanitize`) is ever committed.
 *
 * SCOPE: the resulting fixture guards Anthropic response-shape EVOLUTION (an
 * inbound `ProviderEvent` field/type changing under us), NOT this cycle's seam
 * bugs. See surface invariants §Record-replay.
 *
 * Requires `ANTHROPIC_API_KEY` (a real key — this spends tokens). All console
 * output is piped through the shared value-agnostic redactor so a secret can
 * never reach the terminal even though the on-disk raw file is gitignored.
 *
 *   tsx scripts/record-api-fixtures.ts --help
 *   tsx scripts/record-api-fixtures.ts --scenario simple-turn
 *   tsx scripts/record-api-fixtures.ts --dry-run   # print the plan, hit nothing
 *
 * NOTE: this script is intentionally NOT run in CI — capturing a fresh fixture
 * is a deliberate human/creds action; the committed fixtures are the artifact.
 */

import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { redactString } from "@antpath/contracts";
import type { RawRecording, RecordedEvent } from "./lib/fixtures.js";

const FIXTURES_DIR = fileURLToPath(new URL("../test/fixtures/api-recordings/", import.meta.url));
const API_BASE = process.env.ANTHROPIC_API_BASE_URL ?? "https://api.anthropic.com";
const BETA_HEADER = "managed-agents-2026-04-01,files-api-2025-04-14";
const MODEL = process.env.ANTPATH_FIXTURE_MODEL ?? "claude-haiku-4-5";
const PROBE_MARKER = "fixture-marker-7f3a2c";
const POLL_INTERVAL_MS = 1500;
const MAX_POLLS = 80;

/** Redacted stdout — a captured event echoed for progress must never leak. */
function log(line: string): void {
  process.stdout.write(redactString(line) + "\n");
}

interface CliOptions {
  readonly scenario: string;
  readonly dryRun: boolean;
}

function parseArgs(argv: readonly string[]): CliOptions | "help" {
  if (argv.includes("--help") || argv.includes("-h")) return "help";
  let scenario = "simple-turn";
  let dryRun = false;
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--" || arg === undefined) {
      // The standard argv terminator (pnpm injects it after `run <script> --`).
      continue;
    }
    if (arg === "--scenario") {
      const v = argv[++i];
      if (!v) throw new Error("--scenario requires a name");
      scenario = v;
    } else if (arg === "--dry-run") {
      dryRun = true;
    } else if (arg.startsWith("--")) {
      throw new Error(`unknown flag: ${arg}`);
    }
  }
  return { scenario, dryRun };
}

function help(): void {
  log(
    [
      "fixtures:record:anthropic — LIVE capture of Managed Agents session events.",
      "",
      "  --scenario <name>   scenario label + output filename (default: simple-turn)",
      "  --dry-run           print the capture plan; hit no network, write nothing",
      "  --help              this message",
      "",
      "Requires ANTHROPIC_API_KEY (spends tokens). Output (gitignored):",
      `  ${FIXTURES_DIR}<scenario>.raw.json`,
      "Then run `pnpm --filter antpath run fixtures:sanitize` to produce the committed fixture."
    ].join("\n")
  );
}

function anthropicHeaders(apiKey: string): Record<string, string> {
  return {
    "content-type": "application/json",
    // Anthropic uses x-api-key (not Bearer) — memory: anthropic-files-api-headers.
    "x-api-key": apiKey,
    "anthropic-version": "2023-06-01",
    "anthropic-beta": BETA_HEADER
  };
}

async function postJson(path: string, apiKey: string, body: unknown): Promise<Record<string, unknown>> {
  const res = await fetch(`${API_BASE}${path}`, {
    method: "POST",
    headers: anthropicHeaders(apiKey),
    body: JSON.stringify(body)
  });
  if (!res.ok) throw new Error(`POST ${path} -> HTTP ${res.status}`);
  return (await res.json()) as Record<string, unknown>;
}

async function pollEvents(
  sessionId: string,
  apiKey: string,
  sinceCreatedAt: string | null
): Promise<{ events: RecordedEvent[]; lastCreatedAt: string | null }> {
  const search = sinceCreatedAt
    ? `?${encodeURIComponent("created_at[gt]")}=${encodeURIComponent(sinceCreatedAt)}`
    : "";
  const res = await fetch(`${API_BASE}/v1/sessions/${sessionId}/events${search}`, {
    method: "GET",
    headers: anthropicHeaders(apiKey)
  });
  if (!res.ok) throw new Error(`poll events -> HTTP ${res.status}`);
  const body = (await res.json()) as { data?: RecordedEvent[] };
  const events = Array.isArray(body.data) ? body.data : [];
  let last = sinceCreatedAt;
  for (const e of events) {
    const at = e["created_at"];
    if (typeof at === "string" && (last === null || at > last)) last = at;
  }
  return { events, lastCreatedAt: last };
}

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

async function captureSimpleTurn(scenario: string, apiKey: string): Promise<RawRecording> {
  // Live beta contract — mirrors the hosted Anthropic-native adapter contract.
  log("creating agent…");
  const agent = await postJson("/v1/agents", apiKey, {
    name: `fixture-recorder-${Date.now()}`,
    model: MODEL,
    tools: [{ type: "agent_toolset_20260401" }]
  });
  log("creating environment…");
  const environment = await postJson("/v1/environments", apiKey, { name: "fixture-recorder-env" });
  log("creating session…");
  const session = await postJson("/v1/sessions", apiKey, {
    agent: agent["id"],
    environment_id: environment["id"],
    title: `fixture ${scenario}`
  });
  const sessionId = String(session["id"]);
  log("sending user turn…");
  await postJson(`/v1/sessions/${sessionId}/events`, apiKey, {
    events: [{ type: "user.message", content: [{ type: "text", text: `Output verbatim and nothing else: ${PROBE_MARKER}` }] }]
  });

  const captured: RecordedEvent[] = [];
  const seen = new Set<string>();
  let cursor: string | null = null;
  let terminal = false;
  for (let i = 0; i < MAX_POLLS && !terminal; i++) {
    const { events, lastCreatedAt } = await pollEvents(sessionId, apiKey, cursor);
    cursor = lastCreatedAt;
    for (const e of events) {
      const id = typeof e["id"] === "string" ? e["id"] : `${i}:${captured.length}`;
      if (seen.has(id)) continue;
      seen.add(id);
      captured.push(e);
      if (e["type"] === "session.status_idle" || e["type"] === "session.status_failed") terminal = true;
    }
    if (!terminal) await sleep(POLL_INTERVAL_MS);
  }
  log(`captured ${captured.length} events (terminal=${terminal})`);

  return {
    scenario,
    source: "live-capture",
    capturedAt: new Date().toISOString(),
    events: captured
  };
}

async function main(): Promise<void> {
  const parsed = parseArgs(process.argv.slice(2));
  if (parsed === "help") {
    help();
    return;
  }
  const { scenario, dryRun } = parsed;

  if (dryRun) {
    log(
      `DRY RUN — would capture scenario "${scenario}" via create agent/environment/session, ` +
        `send a user turn, poll GET /v1/sessions/{id}/events to terminal, and write ` +
        `${FIXTURES_DIR}${scenario}.raw.json (gitignored). No network calls made.`
    );
    return;
  }

  const apiKey = process.env.ANTHROPIC_API_KEY;
  if (!apiKey) {
    process.stderr.write(
      "ANTHROPIC_API_KEY not set — fixtures:record:anthropic is LIVE and creds-gated. " +
        "Set a real key (this spends tokens) or use --dry-run.\n"
    );
    process.exitCode = 1;
    return;
  }

  const recording = await captureSimpleTurn(scenario, apiKey);
  const outPath = `${FIXTURES_DIR}${scenario}.raw.json`;
  writeFileSync(outPath, JSON.stringify(recording, null, 2) + "\n", "utf8");
  log(`wrote raw recording -> ${outPath}`);
  log("next: `pnpm --filter antpath run fixtures:sanitize` to produce the committed fixture.");
}

main().catch((err: unknown) => {
  process.stderr.write(redactString(err instanceof Error ? (err.stack ?? err.message) : String(err)) + "\n");
  process.exitCode = 1;
});
