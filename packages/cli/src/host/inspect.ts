/**
 * `aex inspect <run-id>` (DX3) — one-shot, settle-consistent render of a run's
 * FULL timeline plus a header and a footer (jump-to-failure + cost/usage). Built
 * on the same coordinator envelope stream as `aex tail` (replay from seq 0 with
 * the `aex.run.settled` barrier, so the final `getRun` is read-consistent).
 *
 * Human view: header → timeline → footer. `--json` emits one machine document
 * `{ run, events }`. Exit mirrors `wait`: 0 succeeded / 1 other terminal / 3 timeout.
 */
import { operations, type AexEvent, type Run } from "@aexhq/contracts";
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

export async function runInspectCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedRun(io, "inspect")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }

  const jsonFlag = takeBooleanFlag(common.rest, "--json");
  const logsFlag = takeBooleanFlag(jsonFlag.remaining, "--logs");
  const filterFlag = collectRepeated(logsFlag.remaining, "--filter");
  if (filterFlag.error) {
    io.stderr(`${filterFlag.error}\n`);
    return USAGE_ERR;
  }
  const timeoutFlag = takeOptionFlag(filterFlag.remaining, "--timeout");
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
    io.stderr("usage: aex inspect <run-id> [--json] [--filter <type|source>] [--logs] [--timeout <dur>] [common flags]\n");
    return USAGE_ERR;
  }
  const runId = positional[0]!;

  if (!io.webSocketFactory) {
    io.stderr(
      JSON.stringify({
        error: "websocket_unavailable",
        message: "`aex inspect` needs a global WebSocket (Bun or Node >= 22). Upgrade Node or run with bun.",
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

  // Header: read the record first so even an empty/early run prints something.
  let header: Run | null = null;
  try {
    header = await operations.getRun(http, runId);
  } catch (err) {
    const d = describeApiError(err);
    return emitJsonError(io, "inspect_failed", d.message, {
      runId,
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }
  if (!jsonFlag.present) {
    const model = typeof header.model === "string" ? ` · ${header.model}` : "";
    const created = header.createdAt ? ` · ${header.createdAt}` : "";
    io.stdout(`run ${runId} · ${header.status}${model}${created}\n`);
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

  const collected: AexEvent[] = [];
  let runErrorEvent: AexEvent | null = null;
  try {
    const stream = openEnvelopeStream(io, http, runId, {
      from: 0,
      settleConsistent: true,
      signal: controller.signal,
      ...(debug ? { debug } : {})
    });
    for await (const e of stream) {
      if (e.type === "RUN_ERROR") runErrorEvent = e;
      if (filters.predicate && !filters.predicate(e)) continue;
      if (jsonFlag.present) {
        if (logsFlag.present || e.channel !== "log") collected.push(e);
        continue;
      }
      const line = renderEnvelope(e, { logs: logsFlag.present });
      if (line !== null) io.stdout(line + "\n");
    }
  } catch (err) {
    if (timer) clearTimeout(timer);
    const d = describeApiError(err);
    return emitJsonError(io, "inspect_failed", d.message, {
      runId,
      ...(d.status !== undefined ? { status: d.status } : {}),
      ...(d.remedy ? { remedy: d.remedy } : {})
    });
  }
  if (timer) clearTimeout(timer);

  if (timedOut) {
    emitJsonError(io, "inspect_timeout", `timed out after ${timeoutMs}ms inspecting run`, { runId });
    return TIMEOUT_ERR;
  }

  // Settle-consistent stream ended ⇒ the record is read-consistent.
  let finalRun: Run = header;
  try {
    finalRun = await operations.getRun(http, runId);
  } catch {
    /* keep header */
  }

  if (jsonFlag.present) {
    io.stdout(JSON.stringify({ run: finalRun, events: collected }) + "\n");
  } else {
    // Footer: jump-to-failure + cost/usage.
    if (runErrorEvent) {
      const d = runErrorEvent.data as Record<string, unknown>;
      const failureMessage = typeof d.failureMessage === "string" ? d.failureMessage : (runErrorEvent.message ?? "run error");
      const failureClass = typeof d.failureClass === "string" ? ` [${d.failureClass}]` : "";
      io.stdout(`\n✗ ${failureMessage}${failureClass}\n`);
    }
    const costUsd = finalRun.costTelemetry?.billedCostUsd;
    const usage = finalRun.usage;
    if (costUsd !== undefined || usage) {
      const parts: string[] = [`status=${finalRun.status}`];
      if (costUsd !== undefined) parts.push(`costUsd=${costUsd}`);
      if (usage?.inputTokens !== undefined) parts.push(`in=${usage.inputTokens}`);
      if (usage?.outputTokens !== undefined) parts.push(`out=${usage.outputTokens}`);
      if (usage?.totalTokens !== undefined) parts.push(`total=${usage.totalTokens}`);
      io.stdout(`\n— ${parts.join(" · ")}\n`);
    }
  }

  if (finalRun.status === "succeeded") return SUCCESS;
  return RUNTIME_ERR;
}
