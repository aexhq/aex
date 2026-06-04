/**
 * `aex wait <run-id> [--timeout <dur>] [--interval <dur>]`
 *
 * Block until the run reaches a terminal status (the host-side mirror of
 * the SDK's `client.waitForRun` / `client.wait`), then print the final
 * `Run` record as JSON. Exits 0 when the run `succeeded`, RUNTIME_ERR
 * when it reached a non-succeeded terminal status, and TIMEOUT_ERR when
 * the `--timeout` deadline elapsed first.
 *
 * Where `events --follow` streams the event log, `wait` is the quiet
 * "tell me when it's done and what the outcome was" verb — one final
 * line of JSON, script-friendly exit code.
 */
import { operations, TERMINAL_RUN_STATUSES } from "@aexhq/contracts";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  RUNTIME_ERR,
  TIMEOUT_ERR,
  USAGE_ERR,
  emitJsonError,
  makeHttpClient,
  parseCommonHostFlags,
  parseDuration,
  refuseInsideManagedRun,
  takeOptionFlag
} from "./common.js";

// Membership-tested against the loose `string` run status from the BFF, so we
// back it with the canonical terminal set rather than a drift-prone local list.
const TERMINAL_STATUSES = new Set<string>(TERMINAL_RUN_STATUSES);

const DEFAULT_INTERVAL_MS = 2_000;

export async function runWaitCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "wait")) return USAGE_ERR;

  const common = parseCommonHostFlags(argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }

  const timeoutFlag = takeOptionFlag(common.rest, "--timeout");
  const intervalFlag = takeOptionFlag(timeoutFlag.remaining, "--interval");

  let timeoutMs: number | null = null;
  if (timeoutFlag.value !== undefined) {
    const parsed = parseDuration(timeoutFlag.value);
    if (parsed.error) {
      io.stderr(`--timeout: ${parsed.error}\n`);
      return USAGE_ERR;
    }
    timeoutMs = parsed.ms;
  }

  let intervalMs = DEFAULT_INTERVAL_MS;
  if (intervalFlag.value !== undefined) {
    const parsed = parseDuration(intervalFlag.value);
    if (parsed.error) {
      io.stderr(`--interval: ${parsed.error}\n`);
      return USAGE_ERR;
    }
    intervalMs = parsed.ms!;
  }

  const positional = intervalFlag.remaining.filter((arg) => !arg.startsWith("--"));
  if (positional.length !== 1) {
    io.stderr("usage: aex wait <run-id> [--timeout <dur>] [--interval <dur>] [common flags]\n");
    return USAGE_ERR;
  }
  const runId = positional[0]!;

  const http = makeHttpClient(io, common.flags);
  const deadline = timeoutMs === null ? Number.POSITIVE_INFINITY : Date.now() + timeoutMs;

  // Emit the timeout JSON error body (via emitJsonError's side effect)
  // but return the dedicated TIMEOUT_ERR code rather than its RUNTIME_ERR.
  const timeout = (extra: Record<string, unknown>): CliExitCode => {
    emitJsonError(io, "wait_timeout", `timed out after ${timeoutMs}ms waiting for run to finish`, { runId, ...extra });
    return TIMEOUT_ERR;
  };

  while (true) {
    let run;
    try {
      run = await operations.getRun(http, runId);
    } catch (err) {
      // Transient read failures are non-fatal until the deadline — a slow
      // BFF or a brief network blip shouldn't abort a multi-minute wait.
      io.stderr(`(transient) status poll failed: ${(err as Error).message}\n`);
      if (Date.now() >= deadline) return timeout({});
      await sleep(intervalMs);
      continue;
    }

    if (TERMINAL_STATUSES.has(run.status)) {
      io.stdout(JSON.stringify(run) + "\n");
      return run.status === "succeeded" ? SUCCESS : RUNTIME_ERR;
    }

    if (Date.now() >= deadline) return timeout({ lastStatus: run.status });
    await sleep(intervalMs);
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
