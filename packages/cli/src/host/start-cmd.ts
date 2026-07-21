/**
 * `aex start` — one-shot over the session API. The exported command is an
 * order-preserving orchestrator over start-owned parse, config, attachment,
 * and submission stages; shared contracts still own asset and session creation.
 */
import { isReplayableEvent, type HttpClient } from "@aexhq/contracts";
import { operations } from "@aexhq/contracts/internal";
import type { CliIO } from "../internal.js";
import {
  type CliExitCode,
  INTERRUPTED_ERR,
  RUNTIME_ERR,
  SUCCESS,
  TIMEOUT_ERR,
  USAGE_ERR,
  emitApiError,
  emitJsonError,
  isSessionNonProgressing,
  makeHttpClient,
  refuseInsideManagedSession,
  resolveCommonHostFlags
} from "./common.js";
import { parseStartArguments } from "./start-arguments.js";
import { buildStartAttachments } from "./start-attachments.js";
import { resolveStartConfig } from "./start-config.js";
import { buildStartSubmission } from "./start-submission.js";
import { submitCliRun } from "./start-submit.js";
import { openEnvelopeStream } from "./stream-render.js";

type AcceptedStart = Awaited<ReturnType<typeof submitCliRun>>;

export async function executeStartCmd(io: CliIO, argv: readonly string[]): Promise<CliExitCode> {
  if (await refuseInsideManagedSession(io, "start")) return USAGE_ERR;

  const common = await resolveCommonHostFlags(io, argv);
  if (!common.ok) {
    io.stderr(`${common.reason}\n`);
    return USAGE_ERR;
  }
  const parsed = parseStartArguments(common.rest);
  if (!parsed.ok) {
    io.stderr(`${parsed.error}\n`);
    return USAGE_ERR;
  }
  const resolved = await resolveStartConfig(io, parsed.value);
  if (!resolved.ok) {
    io.stderr(`${resolved.error}\n`);
    return USAGE_ERR;
  }
  let attachments;
  try {
    attachments = await buildStartAttachments(io, parsed.value);
  } catch (err) {
    io.stderr(`failed to attach asset: ${(err as Error).message}\n`);
    return USAGE_ERR;
  }
  const options = buildStartSubmission(parsed.value, resolved.value, attachments);
  const http = makeHttpClient(io, common.flags);

  if (parsed.value.follow && !io.webSocketFactory) {
    io.stderr(
      JSON.stringify({
        error: "websocket_unavailable",
        message: "`aex start --follow` needs a global WebSocket (Bun or Node >= 22). Upgrade Node or run with bun."
      }) + "\n"
    );
    return USAGE_ERR;
  }

  let accepted: AcceptedStart;
  try {
    accepted = await submitCliRun(http, io.fetchImpl, options);
  } catch (err) {
    return emitApiError(io, "session_failed", err);
  }
  io.stdout(JSON.stringify(accepted.session) + "\n");
  if (!parsed.value.follow) return SUCCESS;
  return followAcceptedStart(io, http, accepted, parsed.value.followTimeoutMs, common.flags.debug);
}

async function followAcceptedStart(
  io: CliIO,
  http: HttpClient,
  accepted: AcceptedStart,
  followTimeoutMs: number | null,
  debug: boolean
): Promise<CliExitCode> {
  const session = accepted.session;
  const controller = new AbortController();
  let timedOut = false;
  let interrupted = false;
  io.onSignal?.("SIGINT", () => {
    interrupted = true;
    controller.abort();
  });
  const timer =
    followTimeoutMs === null
      ? null
      : setTimeout(() => {
          timedOut = true;
          controller.abort();
        }, followTimeoutMs);
  let lastSeq = -1;
  let terminalOutcome: string | undefined;
  try {
    const stream = openEnvelopeStream(io, http, session.id, {
      runId: accepted.run.runId,
      signal: controller.signal,
      ...(debug ? { debug: (line: string) => io.stderr(`[aex] ${line}\n`) } : {})
    });
    for await (const event of stream) {
      if (isReplayableEvent(event)) {
        lastSeq = event.sequence;
        if (
          event.runId === accepted.run.runId &&
          (event.type === "RUN_FINISHED" || event.type === "RUN_ERROR") &&
          typeof event.data.outcome === "string"
        ) {
          terminalOutcome = event.data.outcome;
        }
      }
      io.stdout(JSON.stringify(event) + "\n");
    }
  } catch (err) {
    if (timer) clearTimeout(timer);
    if (timedOut) {
      emitJsonError(io, "session_follow_timeout", `timed out after ${followTimeoutMs}ms following session`, {
        sessionId: session.id,
        lastSeq,
        ...followTimeoutContext(session, accepted.run),
        hint: `aex status ${session.id} | aex events ${session.id} | aex download ${session.id}`
      });
      return TIMEOUT_ERR;
    }
    if (interrupted) {
      io.stderr(`(interrupted) followed up to seq ${lastSeq}\n`);
      return INTERRUPTED_ERR;
    }
    io.stderr(`(transient) event stream failed: ${(err as Error).message}\n`);
  }
  if (timer) clearTimeout(timer);

  if (timedOut) {
    emitJsonError(io, "session_follow_timeout", `timed out after ${followTimeoutMs}ms following session`, {
      sessionId: session.id,
      lastSeq,
      ...followTimeoutContext(session, accepted.run),
      hint: `aex status ${session.id} | aex events ${session.id} | aex download ${session.id}`
    });
    return TIMEOUT_ERR;
  }
  if (interrupted) {
    io.stderr(`(interrupted) followed up to seq ${lastSeq}\n`);
    return INTERRUPTED_ERR;
  }

  try {
    const final = await operations.getSession(http, session.id);
    io.stdout(JSON.stringify(final) + "\n");
    if (!isSessionNonProgressing(final.status)) {
      io.stderr(
        JSON.stringify({
          error: "run_terminal_inconsistent",
          sessionId: session.id,
          status: final.status,
          hint: `aex status ${session.id} | aex events ${session.id} | aex download ${session.id}`
        }) + "\n"
      );
      return RUNTIME_ERR;
    }
    if (terminalOutcome === "succeeded") return SUCCESS;
    io.stderr(
      JSON.stringify({
        error: "session_not_ok",
        sessionId: session.id,
        status: final.status,
        ...(terminalOutcome ? { outcome: terminalOutcome } : {}),
        hint: `aex status ${session.id} | aex events ${session.id} | aex download ${session.id}`
      }) + "\n"
    );
    return RUNTIME_ERR;
  } catch (err) {
    return emitApiError(io, "session_failed", err, { sessionId: session.id }, { messagePrefix: "final status fetch failed: " });
  }
}

function followTimeoutContext(
  session: { readonly status?: unknown },
  run: { readonly turnSeq?: unknown }
): Record<string, string | number> {
  return {
    ...(typeof session.status === "string" ? { sessionStatus: session.status } : {}),
    ...(typeof run.turnSeq === "number" ? { turnSeq: run.turnSeq } : {})
  };
}
