/**
 * `aex events <session-id> [--follow] [--timeout <dur>]`
 *
 * Without `--follow`: lists the session's events recorded so far and exits.
 *
 * With `--follow`: polls the session `/events` endpoint and prints new events
 * as NDJSON until the session parks.
 */
import { operations } from "@aexhq/contracts";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  TIMEOUT_ERR,
  USAGE_ERR,
  describeApiError,
  emitJsonError,
  isSessionParked,
  makeHttpClient,
  resolveCommonHostFlags,
  parseDuration,
  refuseInsideManagedRun,
  takeBooleanFlag,
  takeOptionFlag
} from "./common.js";

export async function runEventsCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "events")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
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
    io.stderr("usage: aex events <session-id> [--follow] [--timeout <dur>] [common flags]\n");
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
      const d = describeApiError(err);
      return emitJsonError(io, "events_failed", d.message, {
        sessionId,
        ...(d.status !== undefined ? { status: d.status } : {}),
        ...(d.remedy ? { remedy: d.remedy } : {})
      });
    }
  }

  const seen = new Set<string>();
  const deadline = timeoutMs === null ? Number.POSITIVE_INFINITY : Date.now() + timeoutMs;

  // Follow: poll the session /events endpoint until the session parks.
  while (true) {
    let events;
    try {
      events = await operations.listSessionEvents(http, sessionId);
    } catch (err) {
      io.stderr(`(transient) event poll failed: ${(err as Error).message}\n`);
      if (Date.now() >= deadline) return emitTimeout(io, sessionId, timeoutMs);
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
      const session = await operations.getSession(http, sessionId);
      if (isSessionParked(session.status)) {
        return SUCCESS;
      }
    } catch (err) {
      io.stderr(`(transient) status poll failed: ${(err as Error).message}\n`);
    }
    if (Date.now() >= deadline) return emitTimeout(io, sessionId, timeoutMs);
    await sleep(2000);
  }
}

// Emit the timeout JSON error (RUNTIME_ERR side effect) but return the
// dedicated TIMEOUT_ERR code so scripts can distinguish a follow that ran
// out of time from a transport failure.
function emitTimeout(io: CliIO, sessionId: string, timeoutMs: number | null): CliExitCode {
  emitJsonError(io, "events_follow_timeout", `timed out after ${timeoutMs}ms following session events`, { sessionId });
  return TIMEOUT_ERR;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
