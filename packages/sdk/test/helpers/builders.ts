/**
 * Typed test-data builders (B5): fixtures whose types GENUINELY satisfy the
 * public contracts, so tests need no `as never` / `as unknown as` erasure.
 * A builder used by 2+ packages belongs in `@aexhq/contracts/testing`; these
 * are SDK-shaped (SessionRunResult is an SDK type), so they stay package-local.
 */
import type { AexEvent, Session, SessionRun } from "@aexhq/contracts";
import { AEX_EVENT_SPECVERSION } from "@aexhq/contracts";
import type { SessionRunResult } from "../../src/index.js";

export function makeSession(patch: Partial<Session> = {}): Session {
  return {
    id: "sess_1",
    status: "idle",
    acceptsMessages: true,
    ...patch
  };
}

export function makeSessionRun(patch: Partial<SessionRun> = {}): SessionRun {
  return {
    sessionId: "sess_1",
    turnSeq: 1,
    runId: "run_1",
    phase: "finished",
    outcome: "succeeded",
    ...patch
  };
}

export function makeRunResult(patch: Partial<SessionRunResult> = {}): SessionRunResult {
  return {
    status: "succeeded",
    ok: true,
    costUsd: 0,
    usage: { inputTokens: 0, outputTokens: 0, totalTokens: 0 },
    sessionId: "sess_1",
    session: makeSession(),
    run: makeSessionRun(),
    text: "",
    events: [],
    files: [],
    messages: [],
    ...patch
  };
}

export function makeAexEvent<T extends string>(
  type: T,
  patch: Partial<Omit<AexEvent, "type">> = {}
): AexEvent & { readonly type: T } {
  const sequence = patch.sequence ?? 1;
  return {
    specversion: AEX_EVENT_SPECVERSION,
    id: `evt_${sequence}`,
    source: "runtime",
    subject: "sess_1",
    threadId: "sess_1",
    runId: "run_1",
    time: new Date(sequence).toISOString(),
    sequence,
    data: {},
    ...patch,
    type
  };
}
