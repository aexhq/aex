/**
 * `aex tail <session-id>` (DX3) — live, human-readable follow over the
 * coordinator WebSocket envelope stream (replay-from-cursor + tail +
 * exactly-once resume), NOT polling. `--json` is the raw-NDJSON escape hatch;
 * `--filter` narrows by AG-UI type/source; `RUN_ERROR` is surfaced as a
 * jump-to-failure line.
 *
 * stdout carries the event/JSON stream (clean for piping); all diagnostics go to
 * stderr. Exit: 0 success / 1 error (or transport give-up) / 3 timeout / 130 SIGINT.
 */
import { isReplayableEvent } from "@aexhq/contracts";
import { operations } from "@aexhq/contracts/internal";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  INTERRUPTED_ERR,
  RUNTIME_ERR,
  SUCCESS,
  TIMEOUT_ERR,
  USAGE_ERR,
  collectRepeated,
  emitApiError,
  emitJsonError,
  isSessionNonProgressing,
  makeHttpClient,
  parseDuration,
  prepareHostCommand,
  rejectUnknownFlags,
  takeBooleanFlag,
  takeOptionFlag
} from "./common.js";
import { openEnvelopeStream, parseFilters, renderEnvelope } from "./stream-render.js";

export async function executeTailCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  const common = await prepareHostCommand(io, argv, { verb: "tail", auth: "data" });
  if (!common.ok) return common.exit;

  // `--json` is consumed centrally by authenticated preparation (a global flag);
  // read the resolved value rather than re-parsing it here.
  const json = common.flags.json;
  const logsFlag = takeBooleanFlag(common.rest, "--logs");
  const filterFlag = collectRepeated(logsFlag.remaining, "--filter");
  if (filterFlag.error) {
    io.stderr(`${filterFlag.error}\n`);
    return USAGE_ERR;
  }
  const fromFlag = takeOptionFlag(filterFlag.remaining, "--from");
  if (fromFlag.error) { io.stderr(`${fromFlag.error}\n`); return USAGE_ERR; }
  let from = 0;
  if (fromFlag.value !== undefined) {
    const n = Number(fromFlag.value);
    if (!Number.isInteger(n) || n < 0) {
      io.stderr(`--from must be a non-negative integer sequence (got: ${fromFlag.value})\n`);
      return USAGE_ERR;
    }
    from = n;
  }
  const timeoutFlag = takeOptionFlag(fromFlag.remaining, "--timeout");
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

  const usage = "usage: aex tail <session-id> [--json] [--filter <type|source>] [--logs] [--from <seq>] [--timeout <dur>] [common flags]";
  const unknown = rejectUnknownFlags(io, timeoutFlag.remaining, usage);
  if (unknown) return unknown;
  const positional = timeoutFlag.remaining;
  if (positional.length !== 1) {
    io.stderr(`${usage}\n`);
    return USAGE_ERR;
  }
  const sessionId = positional[0]!;

  if (!io.webSocketFactory) {
    io.stderr(
      JSON.stringify({
        error: "websocket_unavailable",
        message: "`aex tail` needs a global WebSocket (Bun or Node >= 22). Upgrade Node or run with bun.",
        sessionId
      }) + "\n"
    );
    return USAGE_ERR;
  }

  const filters = parseFilters(filterFlag.values);
  if (filters.error) {
    io.stderr(`--filter: ${filters.error}\n`);
    return USAGE_ERR;
  }

  const debug = common.flags.debug ? (line: string) => io.stderr(`[aex] ${line}\n`) : undefined;
  const http = makeHttpClient(io, common.flags);
  let targetRunId: string | undefined;
  try {
    const session = await operations.getSession(http, sessionId);
    targetRunId = session.currentRun?.runId ?? session.lastRun?.runId;
  } catch (err) {
    return emitApiError(io, "tail_failed", err, { sessionId });
  }
  const controller = new AbortController();
  let interrupted = false;
  let timedOut = false;
  io.onSignal?.("SIGINT", () => {
    interrupted = true;
    controller.abort();
  });
  const timer =
    timeoutMs === null
      ? null
      : setTimeout(() => {
          timedOut = true;
          controller.abort();
        }, timeoutMs);

  let lastSeq = -1;
  let eventCount = 0;
  let runErrorLine: string | null = null;
  let terminalOutcome: string | undefined;
  const startMs = Date.now();
  try {
    const stream = openEnvelopeStream(io, http, sessionId, {
      from,
      ...(targetRunId ? { runId: targetRunId } : {}),
      signal: controller.signal,
      ...(debug ? { debug } : {})
    });
    for await (const e of stream) {
      if (isReplayableEvent(e)) {
        lastSeq = e.sequence;
        if (
          (e.type === "RUN_FINISHED" || e.type === "RUN_ERROR") &&
          typeof e.data.outcome === "string"
        ) {
          terminalOutcome = e.data.outcome;
        }
      }
      if (filters.predicate && !filters.predicate(e)) {
        // Still hide logs from the pretty stream consistently.
        continue;
      }
      if (json) {
        if (logsFlag.present || e.channel !== "log") {
          io.stdout(JSON.stringify(e) + "\n");
          eventCount++;
        }
      } else {
        const line = renderEnvelope(e, { logs: logsFlag.present });
        if (line !== null) {
          io.stdout(line + "\n");
          eventCount++;
        }
      }
      if (e.type === "RUN_ERROR") runErrorLine = renderEnvelope(e, { logs: true });
    }
  } catch (err) {
    if (timer) clearTimeout(timer);
    return emitApiError(io, "tail_failed", err, { sessionId, lastSeq });
  }
  if (timer) clearTimeout(timer);

  if (timedOut) {
    emitJsonError(io, "tail_timeout", `timed out after ${timeoutMs}ms following session events`, { sessionId, lastSeq });
    return TIMEOUT_ERR;
  }
  if (interrupted) {
    io.stderr(`(interrupted) tailed ${eventCount} event(s) up to seq ${lastSeq}\n`);
    return INTERRUPTED_ERR;
  }
  if (runErrorLine) io.stderr(runErrorLine + "\n");

  // Read the lifecycle record after the terminal event so the exit code also
  // reflects whether the thread is available, held, or recoverably errored.
  let finalStatus = "unknown";
  try {
    const session = await operations.getSession(http, sessionId);
    finalStatus = session.status;
  } catch (err) {
    emitApiError(io, "tail_failed", err, { sessionId, lastSeq }, { messagePrefix: "final status fetch failed: " });
  }
  if (debug) {
    debug(`tail done: events=${eventCount} lastSeq=${lastSeq} durationMs=${Date.now() - startMs} finalStatus=${finalStatus}`);
  }

  if (!isSessionNonProgressing(finalStatus)) {
    io.stderr(
      JSON.stringify({ error: "run_terminal_inconsistent", sessionId, lastSeq, status: finalStatus }) + "\n"
    );
    return RUNTIME_ERR;
  }
  if (terminalOutcome === "succeeded") return SUCCESS;
  if (terminalOutcome !== undefined) return RUNTIME_ERR;
  // Stream ended without a non-progressing record (e.g. transport give-up) — never let
  // a script mistake a silent give-up for a clean finish.
  io.stderr(
    JSON.stringify({
      error: "tail_ended_before_terminal",
      sessionId,
      lastSeq,
      hint: `aex status ${sessionId} | aex tail ${sessionId} --from ${lastSeq + 1}`
    }) + "\n"
  );
  return RUNTIME_ERR;
}
