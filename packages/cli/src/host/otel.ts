/** `aex otel <session-id> [--signal traces|logs] [--json]`. */
import { operations } from "@aexhq/contracts/internal";
import type {
  OtlpExportLogsServiceRequest,
  OtlpExportTraceServiceRequest,
  OtlpSignal
} from "@aexhq/contracts";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  SUCCESS,
  USAGE_ERR,
  emitApiError,
  makeHttpClient,
  prepareHostCommand,
  rejectUnknownFlags,
  takeOptionFlag
} from "./common.js";

export async function executeOtelCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  const common = await prepareHostCommand(io, argv, { verb: "otel", auth: "data" });
  if (!common.ok) return common.exit;
  const signalFlag = takeOptionFlag(common.rest, "--signal");
  if (signalFlag.error) {
    io.stderr(`${signalFlag.error}\n`);
    return USAGE_ERR;
  }
  const signal = signalFlag.value ?? "traces";
  if (signal !== "traces" && signal !== "logs") {
    io.stderr("--signal must be traces or logs\n");
    return USAGE_ERR;
  }
  const usage = "usage: aex otel <session-id> [--signal traces|logs] [--json] [common flags]";
  const unknown = rejectUnknownFlags(io, signalFlag.remaining, usage);
  if (unknown) return unknown;
  if (signalFlag.remaining.length !== 1) {
    io.stderr(`${usage}\n`);
    return USAGE_ERR;
  }
  const sessionId = signalFlag.remaining[0]!;
  const http = makeHttpClient(io, common.flags);

  try {
    const output = await collectOtlpPages(http, sessionId, signal);
    io.stdout(`${JSON.stringify(output, null, common.flags.json ? undefined : 2)}\n`);
    return SUCCESS;
  } catch (err) {
    return emitApiError(io, "otel_failed", err, { sessionId, signal });
  }
}

async function collectOtlpPages(
  http: Parameters<typeof operations.iterateSessionOtlpPages>[0],
  sessionId: string,
  signal: OtlpSignal
): Promise<OtlpExportTraceServiceRequest | OtlpExportLogsServiceRequest> {
  if (signal === "traces") {
    const resourceSpans: OtlpExportTraceServiceRequest["resourceSpans"][number][] = [];
    for await (const page of operations.iterateSessionOtlpPages(http, sessionId, "traces")) {
      if (!("resourceSpans" in page)) throw new TypeError("OTLP traces page has the wrong signal shape");
      resourceSpans.push(...page.resourceSpans);
    }
    return { resourceSpans };
  }
  const resourceLogs: OtlpExportLogsServiceRequest["resourceLogs"][number][] = [];
  for await (const page of operations.iterateSessionOtlpPages(http, sessionId, "logs")) {
    if (!("resourceLogs" in page)) throw new TypeError("OTLP logs page has the wrong signal shape");
    resourceLogs.push(...page.resourceLogs);
  }
  return { resourceLogs };
}
