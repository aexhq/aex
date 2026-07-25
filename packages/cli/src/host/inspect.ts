/**
 * `aex inspect <session-id>` (DX3) — one-shot, checkpoint-consistent render of a
 * session's FULL timeline plus a header and a footer (jump-to-failure +
 * cost/usage). Built on the same coordinator envelope stream as `aex tail`
 * (replay from seq 0 through the committed RUN terminal, so the final
 * `getSession` is read-consistent).
 *
 * Human view: header → timeline → footer. `--json` emits one machine document
 * `{ session, events }`. Exit is 0 for a succeeded run, 1 otherwise, or 3 on timeout.
 */
import {
  isReplayableEvent,
  usageFromProviderUsage,
  type AexEvent,
  type AexStreamEvent,
  type Session
} from "@aexhq/contracts";
import { operations } from "@aexhq/contracts/internal";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
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

export async function executeInspectCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  const common = await prepareHostCommand(io, argv, { verb: "inspect", auth: "data" });
  if (!common.ok) return common.exit;

  // `--json` is consumed centrally by authenticated preparation (a global flag).
  const json = common.flags.json;
  const logsFlag = takeBooleanFlag(common.rest, "--logs");
  const filterFlag = collectRepeated(logsFlag.remaining, "--filter");
  if (filterFlag.error) {
    io.stderr(`${filterFlag.error}\n`);
    return USAGE_ERR;
  }
  const timeoutFlag = takeOptionFlag(filterFlag.remaining, "--timeout");
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

  const usage = "usage: aex inspect <session-id> [--json] [--filter <type|source>] [--logs] [--timeout <dur>] [common flags]";
  const unknown = rejectUnknownFlags(io, timeoutFlag.remaining, usage);
  if (unknown) return unknown;
  const positional = timeoutFlag.remaining;
  if (positional.length !== 1) {
    io.stderr(`${usage}\n`);
    return USAGE_ERR;
  }
  const sessionId = positional[0]!;

  const filters = parseFilters(filterFlag.values);
  if (filters.error) {
    io.stderr(`--filter: ${filters.error}\n`);
    return USAGE_ERR;
  }

  const debug = common.flags.debug ? (line: string) => io.stderr(`[aex] ${line}\n`) : undefined;
  const http = makeHttpClient(io, common.flags);

  // Header: read the record first so even an empty/early session prints something.
  let header: Session | null = null;
  try {
    header = await operations.getSession(http, sessionId);
  } catch (err) {
    return emitApiError(io, "inspect_failed", err, { sessionId });
  }
  if (!json) {
    const model = typeof header.model === "string" ? ` · ${header.model}` : "";
    const created = header.createdAt ? ` · ${header.createdAt}` : "";
    io.stdout(`session ${sessionId} · ${header.status}${model}${created}\n`);
  }

  const targetRunId = header.currentRun?.runId ?? header.lastRun?.runId;
  if (targetRunId === undefined) {
    if (json) io.stdout(JSON.stringify({ session: header, events: [] }) + "\n");
    return header.status === "idle" && header.acceptsMessages ? SUCCESS : RUNTIME_ERR;
  }

  if (!io.webSocketFactory) {
    io.stderr(
      JSON.stringify({
        error: "websocket_unavailable",
        message: "`aex inspect` needs a global WebSocket (Bun or Node >= 22). Upgrade Node or run with bun.",
        sessionId
      }) + "\n"
    );
    return USAGE_ERR;
  }

  const controller = new AbortController();
  let timedOut = false;
  const timer =
    timeoutMs === null
      ? null
      : setTimeout(() => {
          timedOut = true;
          controller.abort();
        }, timeoutMs);

  const collected: AexStreamEvent[] = [];
  let runErrorEvent: AexEvent | null = null;
  let terminalOutcome: string | undefined;
  try {
    const stream = openEnvelopeStream(io, http, sessionId, {
      from: 0,
      runId: targetRunId,
      signal: controller.signal,
      ...(debug ? { debug } : {})
    });
    for await (const e of stream) {
      if (isReplayableEvent(e) && (e.type === "RUN_FINISHED" || e.type === "RUN_ERROR")) {
        if (e.type === "RUN_ERROR") runErrorEvent = e;
        if (typeof e.data.outcome === "string") terminalOutcome = e.data.outcome;
      }
      if (filters.predicate && !filters.predicate(e)) continue;
      if (json) {
        if (logsFlag.present || e.channel !== "log") collected.push(e);
        continue;
      }
      const line = renderEnvelope(e, { logs: logsFlag.present });
      if (line !== null) io.stdout(line + "\n");
    }
  } catch (err) {
    if (timer) clearTimeout(timer);
    return emitApiError(io, "inspect_failed", err, { sessionId });
  }
  if (timer) clearTimeout(timer);

  if (timedOut) {
    emitJsonError(io, "inspect_timeout", `timed out after ${timeoutMs}ms inspecting session`, { sessionId });
    return TIMEOUT_ERR;
  }

  // The committed RUN terminal is the read-consistency barrier.
  let finalSession: Session = header;
  try {
    finalSession = await operations.getSession(http, sessionId);
  } catch {
    /* keep header */
  }

  if (json) {
    io.stdout(JSON.stringify({ session: finalSession, events: collected }) + "\n");
  } else {
    // Footer: jump-to-failure + cost/usage.
    if (runErrorEvent) {
      const d = runErrorEvent.data as Record<string, unknown>;
      const failureMessage = typeof d.failureMessage === "string" ? d.failureMessage : (runErrorEvent.message ?? "run error");
      const failureClass = typeof d.failureClass === "string" ? ` [${d.failureClass}]` : "";
      io.stdout(`\n✗ ${failureMessage}${failureClass}\n`);
    }
    const costUsd = finalSession.costUsd;
    // Token counts come from `costTelemetry.providerUsage` — the ONE server
    // source. This read used to be `finalSession.usage`, a top-level field the
    // data plane has never emitted, so the footer's `in=`/`out=`/`total=` were
    // unreachable in practice.
    const providerUsage = finalSession.costTelemetry?.providerUsage;
    const usage = providerUsage ? usageFromProviderUsage(providerUsage) : undefined;
    if (costUsd !== undefined || usage) {
      const parts: string[] = [`status=${finalSession.status}`];
      if (costUsd !== undefined) parts.push(`costUsd=${costUsd}`);
      if (usage?.inputTokens !== undefined) parts.push(`in=${usage.inputTokens}`);
      if (usage?.outputTokens !== undefined) parts.push(`out=${usage.outputTokens}`);
      if (usage?.totalTokens !== undefined) parts.push(`total=${usage.totalTokens}`);
      io.stdout(`\n— ${parts.join(" · ")}\n`);
    }
  }

  if (isSessionNonProgressing(finalSession.status) && terminalOutcome === "succeeded") return SUCCESS;
  return RUNTIME_ERR;
}
