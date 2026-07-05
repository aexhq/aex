/**
 * SPIKE (throwaway): measure the real park-event -> record-idle gap for interactive
 * session turns. This is the extra wall-clock Option B (settle-consistent turn stream)
 * would force onto EVERY turn: the turn stream currently ends on the RUN_FINISHED park
 * event, but the session RECORD flips running->idle later, in the async settle lambda.
 *
 *   AEX_API_KEY=... DEEPSEEK_API_KEY=... AEX_API_URL=... bun packages/sdk/examples/spike-settle-latency.ts
 *
 * Optional: SPIKE_TURNS=8 (samples), SPIKE_POLL_MS=120 (record poll interval).
 *
 * Method per turn:
 *   1) send a tiny prompt, stream events, capture t_runFinished at the RUN_FINISHED event
 *      (this is where the default turn stream ends), then stop consuming.
 *   2) tight-poll session.refresh() until status === "idle"; capture t_idle.
 *   3) settleGapMs = t_idle - t_runFinished  <-- the Option B tax.
 * The next turn is only sent AFTER idle, so each measurement is isolated (no reconcile).
 */
import { Aex, Models, Providers, Sizes } from "@aexhq/sdk";

const apiKey = required("AEX_API_KEY");
const deepseekKey = required("DEEPSEEK_API_KEY");
const apiUrl = process.env.AEX_API_URL;
const turns = Number(process.env.SPIKE_TURNS ?? "8");
const pollMs = Number(process.env.SPIKE_POLL_MS ?? "120");

const aex = new Aex({
  apiKey,
  ...(apiUrl ? { baseUrl: apiUrl } : {}),
  retry: { maxAttempts: 4, initialDelayMs: 500, maxDelayMs: 10_000, maxElapsedMs: 90_000 }
});

console.log(`opening session (dev)...`);
const session = await aex.openSession({
  provider: Providers.DEEPSEEK,
  model: Models.DEEPSEEK_V4_FLASH,
  system: "Reply with a single short sentence. Never use tools or write files.",
  includeBuiltinTools: false,
  tools: [],
  runtime: Sizes.SHARED_0_25X_1GB,
  overrides: { idleTtl: "10m", timeout: "5m", maxSpendUsd: 1 },
  apiKeys: { deepseek: deepseekKey }
});
console.log(`session: ${session.id}`);

type Sample = { turn: number; turnMs: number; settleGapMs: number; polls: number; timedOut: boolean };
const samples: Sample[] = [];

for (let i = 0; i < turns; i++) {
  const stream = session.send(`Say hello #${i + 1} in one short sentence.`);
  const tStart = performance.now();
  let tRunFinished = 0;
  let errored = false;
  for await (const event of stream) {
    if (event.isRunError()) {
      errored = true;
      tRunFinished = performance.now();
      break;
    }
    if (event.isRunFinished()) {
      tRunFinished = performance.now();
      break; // default turn stream would end HERE
    }
  }
  if (tRunFinished === 0) tRunFinished = performance.now();
  const turnMs = tRunFinished - tStart;

  // Tight-poll the authoritative record until it leaves running.
  let polls = 0;
  let tIdle = 0;
  let timedOut = false;
  const deadline = performance.now() + 90_000;
  for (;;) {
    polls++;
    const rec = await session.refresh().catch(() => undefined);
    const status = rec?.status;
    if (status === "idle" || status === "suspended" || status === "error") {
      tIdle = performance.now();
      break;
    }
    if (performance.now() >= deadline) {
      tIdle = performance.now();
      timedOut = true;
      break;
    }
    await sleep(pollMs);
  }
  const settleGapMs = tIdle - tRunFinished;
  samples.push({ turn: i + 1, turnMs, settleGapMs, polls, timedOut });
  console.log(
    `turn ${String(i + 1).padStart(2)}  turn=${fmt(turnMs)}  settleGap=${fmt(settleGapMs)}  polls=${polls}` +
      (errored ? "  (RUN_ERROR)" : "") +
      (timedOut ? "  (TIMEOUT>90s)" : "")
  );
}

const gaps = samples.map((s) => s.settleGapMs).sort((a, b) => a - b);
console.log("\n=== settle-gap distribution (park event -> record idle) ===");
console.log(`n=${gaps.length}`);
console.log(`min    ${fmt(gaps[0])}`);
console.log(`median ${fmt(pct(gaps, 50))}`);
console.log(`p90    ${fmt(pct(gaps, 90))}`);
console.log(`max    ${fmt(gaps[gaps.length - 1])}`);
console.log(`mean   ${fmt(gaps.reduce((a, b) => a + b, 0) / gaps.length)}`);
console.log("\nInterpretation: settleGap is the per-turn latency Option B adds before the");
console.log("caller regains control (the turn stream would block until the record is idle).");

function pct(sorted: number[], p: number): number {
  if (sorted.length === 0) return 0;
  const idx = Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length));
  return sorted[idx];
}
function fmt(ms: number): string {
  return ms >= 1000 ? `${(ms / 1000).toFixed(2)}s` : `${Math.round(ms)}ms`;
}
function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}
function required(name: string): string {
  const v = process.env[name];
  if (!v) {
    console.error(`Missing env var ${name}`);
    process.exit(1);
  }
  return v;
}
