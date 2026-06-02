/**
 * `antpath events <run-id> [--follow] [--transport=auto|sse|polling]`
 *
 * Without `--follow`: lists events recorded so far and exits.
 *
 * With `--follow`: polls the coordinator-backed `/events` endpoint and
 * prints new events as NDJSON until the run reaches a terminal status.
 */
import { operations, TERMINAL_RUN_STATUSES } from "@antpath/contracts";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  TIMEOUT_ERR,
  USAGE_ERR,
  emitJsonError,
  makeHttpClient,
  parseCommonHostFlags,
  parseDuration,
  refuseInsideManagedRun,
  takeBooleanFlag,
  takeOptionFlag
} from "./common.js";

// Membership-tested against the loose `string` run status from the BFF, so we
// back it with the canonical terminal set rather than a drift-prone local list.
const TERMINAL_STATUSES = new Set<string>(TERMINAL_RUN_STATUSES);

export async function runEventsCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "events")) return USAGE_ERR;

  const common = parseCommonHostFlags(argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const followResult = takeBooleanFlag(common.rest, "--follow");
  const timeoutFlag = takeOptionFlag(followResult.remaining, "--timeout");
  let timeoutMs: number | null = null;
  if (timeoutFlag.value !== undefined) {
    const parsed = parseDuration(timeoutFlag.value);
    if (parsed.error) {
      io.stderr(`--timeout: ${parsed.error}\n`);
      return USAGE_ERR;
    }
    timeoutMs = parsed.ms;
  }
  const positional = timeoutFlag.remaining.filter((arg) => !arg.startsWith("--"));
  if (positional.length !== 1) {
    io.stderr("usage: antpath events <run-id> [--follow] [--timeout <dur>] [common flags]\n");
    return USAGE_ERR;
  }
  const runId = positional[0]!;

  const http = makeHttpClient(io, common.flags);

  if (!followResult.present) {
    try {
      const events = await operations.listRunEvents(http, runId);
      for (const event of events) {
        io.stdout(JSON.stringify(event) + "\n");
      }
      return SUCCESS;
    } catch (err) {
      return emitJsonError(io, "events_failed", (err as Error).message ?? "event fetch failed", { runId });
    }
  }

  const seen = new Set<string>();
  const deadline = timeoutMs === null ? Number.POSITIVE_INFINITY : Date.now() + timeoutMs;

  // Follow: poll the coordinator-backed /events endpoint until terminal.
  while (true) {
    let events;
    try {
      events = await operations.listRunEvents(http, runId);
    } catch (err) {
      io.stderr(`(transient) event poll failed: ${(err as Error).message}\n`);
      if (Date.now() >= deadline) return emitTimeout(io, runId, timeoutMs);
      await sleep(2000);
      continue;
    }
    for (const event of events) {
      if (!seen.has(event.id)) {
        seen.add(event.id);
        io.stdout(JSON.stringify(event) + "\n");
      }
    }

    try {
      const run = await operations.getRun(http, runId);
      if (TERMINAL_STATUSES.has(run.status)) {
        return SUCCESS;
      }
    } catch (err) {
      io.stderr(`(transient) status poll failed: ${(err as Error).message}\n`);
    }
    if (Date.now() >= deadline) return emitTimeout(io, runId, timeoutMs);
    await sleep(2000);
  }
}

// Emit the timeout JSON error (RUNTIME_ERR side effect) but return the
// dedicated TIMEOUT_ERR code so scripts can distinguish a follow that ran
// out of time from a transport failure.
function emitTimeout(io: CliIO, runId: string, timeoutMs: number | null): CliExitCode {
  emitJsonError(io, "events_follow_timeout", `timed out after ${timeoutMs}ms following run events`, { runId });
  return TIMEOUT_ERR;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
