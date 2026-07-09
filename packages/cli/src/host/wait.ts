/**
 * `aex wait <session-id> [--timeout <dur>] [--interval <dur>]`
 *
 * Block until the session parks — reaches `idle`/`suspended`/`error` or a
 * terminal session status (the host-side mirror of the SDK's `session.wait()`),
 * then print the final `Session` record as JSON. Exits 0 when the session
 * parked cleanly (`idle`/`suspended`), RUNTIME_ERR on a non-clean park
 * (`error`/`failed`/…), and TIMEOUT_ERR when the `--timeout` deadline elapsed
 * first.
 *
 * Where `events --follow` streams the event log, `wait` is the quiet
 * "tell me when it's done and what the outcome was" verb — one final
 * line of JSON, script-friendly exit code.
 */
import { operations } from "@aexhq/contracts";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  RUNTIME_ERR,
  TIMEOUT_ERR,
  USAGE_ERR,
  emitJsonError,
  isSessionOk,
  isSessionParked,
  makeHttpClient,
  resolveCommonHostFlags,
  parseDuration,
  rejectUnknownFlags,
  refuseInsideManagedSession,
  takeOptionFlag
} from "./common.js";

const DEFAULT_INTERVAL_MS = 2_000;

export async function executeWaitCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedSession(io, "wait")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
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

  const usage = "usage: aex wait <session-id> [--timeout <dur>] [--interval <dur>] [common flags]";
  const unknown = rejectUnknownFlags(io, intervalFlag.remaining, usage);
  if (unknown) return unknown;
  const positional = intervalFlag.remaining;
  if (positional.length !== 1) {
    io.stderr(`${usage}\n`);
    return USAGE_ERR;
  }
  const sessionId = positional[0]!;

  const http = makeHttpClient(io, common.flags);
  const deadline = timeoutMs === null ? Number.POSITIVE_INFINITY : Date.now() + timeoutMs;

  // Emit the timeout JSON error body (via emitJsonError's side effect)
  // but return the dedicated TIMEOUT_ERR code rather than its RUNTIME_ERR.
  const timeout = (extra: Record<string, unknown>): CliExitCode => {
    emitJsonError(io, "wait_timeout", `timed out after ${timeoutMs}ms waiting for session to park`, { sessionId, ...extra });
    return TIMEOUT_ERR;
  };

  while (true) {
    let session;
    try {
      session = await operations.getSession(http, sessionId);
    } catch (err) {
      // Transient read failures are non-fatal until the deadline — a slow
      // BFF or a brief network blip shouldn't abort a multi-minute wait.
      io.stderr(`(transient) status poll failed: ${(err as Error).message}\n`);
      if (Date.now() >= deadline) return timeout({});
      await sleep(intervalMs);
      continue;
    }

    if (isSessionParked(session.status)) {
      io.stdout(JSON.stringify(session) + "\n");
      return isSessionOk(session.status) ? SUCCESS : RUNTIME_ERR;
    }

    if (Date.now() >= deadline) return timeout({ lastStatus: session.status });
    await sleep(intervalMs);
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
