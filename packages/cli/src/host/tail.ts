/**
 * `aex tail <run-id>` (DX3) — live, human-readable follow over the coordinator
 * WebSocket envelope stream (replay-from-cursor + tail + exactly-once resume),
 * NOT polling. `--json` is the raw-NDJSON escape hatch; `--filter` narrows by
 * AG-UI type/source; `RUN_ERROR` is surfaced as a jump-to-failure line.
 *
 * stdout carries the event/JSON stream (clean for piping); all diagnostics go to
 * stderr. Exit: 0 succeeded / 1 other terminal (or transport give-up) / 3 timeout.
 */
import { operations, TERMINAL_RUN_STATUSES, type AexEvent } from "@aexhq/contracts";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  RUNTIME_ERR,
  SUCCESS,
  TIMEOUT_ERR,
  USAGE_ERR,
  collectRepeated,
  describeApiError,
  emitJsonError,
  makeHttpClient,
  parseDuration,
  refuseInsideManagedRun,
  resolveCommonHostFlags,
  takeBooleanFlag,
  takeOptionFlag
} from "./common.js";
import { openEnvelopeStream, parseFilters, renderEnvelope } from "./stream-render.js";

const TERMINAL_STATUSES = new Set<string>(TERMINAL_RUN_STATUSES);

export async function runTailCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "tail")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }

  const jsonFlag = takeBooleanFlag(common.rest, "--json");
  const logsFlag = takeBooleanFlag(jsonFlag.remaining, "--logs");
  const settleFlag = takeBooleanFlag(logsFlag.remaining, "--settle");
  const filterFlag = collectRepeated(settleFlag.remaining, "--filter");
  if (filterFlag.error) {
    io.stderr(`${filterFlag.error}\n`);
    return USAGE_ERR;
  }
  const fromFlag = takeOptionFlag(filterFlag.remaining, "--from");
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
  let timeoutMs: number | null = null;
  if (timeoutFlag.value !== undefined) {
    const parsed = parseDuration(timeoutFlag.value);
    if (parsed.error) {
      io.stderr(`--timeout: ${parsed.error}\n`);
      return USAGE_ERR;
    }
    timeoutMs = parsed.ms;
  }

  const positional = timeoutFlag.remaining.filter((a) => !a.startsWith("--"));
  if (positional.length !== 1) {
    io.stderr("usage: aex tail <run-id> [--json] [--filter <type|source>] [--logs] [--from <seq>] [--settle] [--timeout <dur>] [common flags]\n");
    return USAGE_ERR;
  }
  const runId = positional[0]!;

  if (!io.webSocketFactory) {
    io.stderr(
      JSON.stringify({
        error: "websocket_unavailable",
        message: "`aex tail` needs a global WebSocket (Bun or Node >= 22). Upgrade Node or run with bun.",
        runId
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
  const startMs = Date.now();
  try {
    const stream = openEnvelopeStream(io, http, runId, {
      from,
      ...(settleFlag.present ? { settleConsistent: true } : {}),
      signal: controller.signal,
      ...(debug ? { debug } : {})
    });
    for await (const e of stream) {
      lastSeq = e.sequence;
      if (filters.predicate && !filters.predicate(e)) {
        // Still hide logs from the pretty stream consistently.
        continue;
      }
      if (jsonFlag.present) {
        if (logsFlag.present || (e as AexEvent).channel !== "log") {
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
    const d = describeApiError(err);
    return emitJsonError(io, "tail_failed", d.message, {
      runId,
      lastSeq,
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }
  if (timer) clearTimeout(timer);

  if (timedOut) {
    emitJsonError(io, "tail_timeout", `timed out after ${timeoutMs}ms following run events`, { runId, lastSeq });
    return TIMEOUT_ERR;
  }
  if (interrupted) {
    io.stderr(`(interrupted) tailed ${eventCount} event(s) up to seq ${lastSeq}\n`);
    return SUCCESS;
  }
  if (runErrorLine) io.stderr(runErrorLine + "\n");

  // The terminal EVENT precedes the authoritative run RECORD; read it back so
  // the exit code reflects the real outcome (not just "a terminal frame seen").
  let finalStatus = "unknown";
  try {
    const run = await operations.getRun(http, runId);
    finalStatus = run.status;
  } catch (err) {
    io.stderr(`final status fetch failed: ${(err as Error).message}\n`);
  }
  if (debug) {
    debug(`tail done: events=${eventCount} lastSeq=${lastSeq} durationMs=${Date.now() - startMs} finalStatus=${finalStatus}`);
  }

  if (finalStatus === "succeeded") return SUCCESS;
  if (TERMINAL_STATUSES.has(finalStatus)) return RUNTIME_ERR;
  // Stream ended without a terminal record (e.g. transport give-up) — never let
  // a script mistake a silent give-up for a clean finish.
  io.stderr(
    JSON.stringify({
      error: "tail_ended_before_terminal",
      runId,
      lastSeq,
      hint: `aex status ${runId} | aex tail ${runId} --from ${lastSeq + 1}`
    }) + "\n"
  );
  return RUNTIME_ERR;
}
