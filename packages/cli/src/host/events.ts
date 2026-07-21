/**
 * `aex events <session-id> [--follow] [--timeout <dur>]`
 *
 * Without `--follow`: lists the session's events recorded so far and exits.
 *
 * With `--follow`: polls the session `/events` endpoint and prints new events
 * as NDJSON until the latest durable event is a run terminal.
 */
import { operations } from "@aexhq/contracts/internal";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  TIMEOUT_ERR,
  USAGE_ERR,
  emitApiError,
  emitJsonError,
  makeHttpClient,
  resolveCommonHostFlags,
  parseDuration,
  rejectUnknownFlags,
  refuseInsideManagedSession,
  takeBooleanFlag,
  takeOptionFlag
} from "./common.js";
import { pollingDelay } from "./command-primitives.js";

export async function executeEventsCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedSession(io, "events")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const followResult = takeBooleanFlag(common.rest, "--follow");
  const timeoutFlag = takeOptionFlag(followResult.remaining, "--timeout");
  if (timeoutFlag.error) { io.stderr(`${timeoutFlag.error}\n`); return USAGE_ERR; }
  let timeoutMs: number | null = null;
  if (timeoutFlag.value !== undefined) {
    const parsed = parseDuration(timeoutFlag.value);
    if (parsed.error) {
      io.stderr(`--timeout: ${parsed.error}\n`);
      return USAGE_ERR;
    }
    timeoutMs = parsed.ms;
  }
  const usage = "usage: aex events <session-id> [--follow] [--timeout <dur>] [common flags]";
  const unknown = rejectUnknownFlags(io, timeoutFlag.remaining, usage);
  if (unknown) return unknown;
  const positional = timeoutFlag.remaining;
  if (positional.length !== 1) {
    io.stderr(`${usage}\n`);
    return USAGE_ERR;
  }
  const sessionId = positional[0]!;

  const http = makeHttpClient(io, common.flags);

  if (!followResult.present) {
    try {
      const events = await operations.listSessionEvents(http, sessionId);
      for (const event of events) {
        io.stdout(JSON.stringify(event) + "\n");
      }
      return SUCCESS;
    } catch (err) {
      return emitApiError(io, "events_failed", err, { sessionId });
    }
  }

  const seen = new Set<string>();
  const deadline = timeoutMs === null ? Number.POSITIVE_INFINITY : Date.now() + timeoutMs;

  // Follow: poll durable events. HttpClient owns bounded transport retries, so
  // this loop never retries a failed application scenario.
  while (true) {
    let events;
    try {
      events = await operations.listSessionEvents(http, sessionId);
    } catch (err) {
      return emitApiError(io, "events_failed", err, { sessionId });
    }
    for (const event of events) {
      if (!seen.has(event.id)) {
        seen.add(event.id);
        io.stdout(JSON.stringify(event) + "\n");
      }
    }

    const latest = events.at(-1);
    if (latest?.type === "RUN_FINISHED" || latest?.type === "RUN_ERROR") return SUCCESS;
    if (Date.now() >= deadline) return emitTimeout(io, sessionId, timeoutMs);
    await pollingDelay(2000);
  }
}

// Emit the timeout JSON error (RUNTIME_ERR side effect) but return the
// dedicated TIMEOUT_ERR code so scripts can distinguish a follow that ran
// out of time from a transport failure.
function emitTimeout(io: CliIO, sessionId: string, timeoutMs: number | null): CliExitCode {
  emitJsonError(io, "events_follow_timeout", `timed out after ${timeoutMs}ms following session events`, { sessionId });
  return TIMEOUT_ERR;
}
