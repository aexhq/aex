/**
 * `aex wait <session-id> [--timeout <dur>] [--interval <dur>]`
 *
 * Block until the session reaches a non-progressing lifecycle state, then
 * print the final `Session` record as JSON. Exits 0 only when it is ready for a
 * new message (`idle`), RUNTIME_ERR when it is held, errored, or removed, and
 * TIMEOUT_ERR when the `--timeout` deadline elapses first.
 *
 * Where `events --follow` streams the event log, `wait` is the quiet
 * "tell me when the session can stop progressing" verb with one final
 * line of JSON, script-friendly exit code.
 */
import { operations } from "@aexhq/contracts/internal";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  RUNTIME_ERR,
  TIMEOUT_ERR,
  USAGE_ERR,
  emitApiError,
  emitJsonError,
  isSessionNonProgressing,
  makeHttpClient,
  parseDuration,
  prepareHostCommand,
  rejectUnknownFlags,
  takeOptionFlag
} from "./common.js";
import { pollingDelay } from "./command-primitives.js";

const DEFAULT_INTERVAL_MS = 2_000;

export async function executeWaitCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  const common = await prepareHostCommand(io, argv, { verb: "wait", auth: "data" });
  if (!common.ok) return common.exit;

  const timeoutFlag = takeOptionFlag(common.rest, "--timeout");
  const intervalFlag = takeOptionFlag(timeoutFlag.remaining, "--interval");
  const optionError = timeoutFlag.error ?? intervalFlag.error;
  if (optionError) { io.stderr(`${optionError}\n`); return USAGE_ERR; }

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
    emitJsonError(io, "wait_timeout", `timed out after ${timeoutMs}ms waiting for a non-progressing session state`, { sessionId, ...extra });
    return TIMEOUT_ERR;
  };

  while (true) {
    let session;
    try {
      session = await operations.getSession(http, sessionId);
    } catch (err) {
      return emitApiError(io, "wait_failed", err, { sessionId });
    }

    if (isSessionNonProgressing(session.status)) {
      io.stdout(JSON.stringify(session) + "\n");
      return session.status === "idle" ? SUCCESS : RUNTIME_ERR;
    }

    if (Date.now() >= deadline) return timeout({ lastStatus: session.status });
    await pollingDelay(intervalMs);
  }
}
