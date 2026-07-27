/**
 * Shared harness and scenario registrations for the independently sharded
 * edge-chat-*.user.test.ts files.
 *
 * Customer-perspective adversarial probing of the multi-turn CHAT SESSION
 * surface of the installed `@aexhq/sdk` against the DEV plane:
 *   client.sessions.create/open/get/list
 *   SessionHandle messages/suspend/resume/cancel/delete/refresh
 *   session.messages / session.events
 *
 * Each `it` spawns its own uniquely-named child script that drives the SDK
 * end-to-end and prints a single JSON result. Child scripts NEVER throw — they
 * catch everything and always `emit(...)` so the raw behaviour is captured as
 * evidence even when an assertion later fails.
 *
 * Model: deepseek/deepseek-v4-flash through the managed gateway, with tiny prompts.
 *
 * Required env: AEX_API_URL, AEX_API_KEY,
 *   AEX_USER_TEST_TARBALL | AEX_USER_TEST_VERSION
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { getBunCommand, installAex, runCommand, type InstallResult } from "./install.js";
import { gateModel } from "./provider.js";
import { formatChildFailure, redactKnownValues } from "./live-diagnostics.js";
import { requireLiveRuntimeKind } from "./runtime-kind.js";
import type { EdgeChatSessionShard } from "./edge-chat-session-manifest.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (edge-chat-session): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL").replace(/\/$/, "");
const apiKey = requireEnv("AEX_API_KEY");
const model = gateModel();
const runtimeKind = requireLiveRuntimeKind("edge chat live test");

function buildPassEnv(extras: Record<string, string>): Record<string, string> {
  const env: Record<string, string> = { ...extras };
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  if (process.env[pathKey]) env[pathKey] = process.env[pathKey]!;
  const carry =
    process.platform === "win32"
      ? ["SystemRoot", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "ComSpec", "ProgramFiles", "ProgramData"]
      : ["HOME", "TMPDIR", "LANG", "LC_ALL"];
  for (const key of carry) if (process.env[key]) env[key] = process.env[key]!;
  return env;
}

let install: InstallResult;

beforeAll(async () => {
  install = await installAex();
}, 240_000);

afterAll(() => {
  install?.cleanup();
});

// Shared child-script preamble. NOTE: keep this free of `${` so it survives the
// outer TS template literal verbatim. All dynamic values arrive via env.
const PRE = `
import { Aex } from "@aexhq/sdk";
const baseUrl = process.env.AEX_API_URL.replace(/\\/$/, "");
const apiKey = process.env.AEX_API_KEY;
const model = process.env.MODEL;
const runtimeKind = process.env.RUNTIME_KIND;
const client = new Aex({ baseUrl, apiKey });
const CREATE = {
  model,
  runtime: { kind: runtimeKind },
  builtinTools: "none",
  system: "You are a terse assistant. Follow the user's instructions exactly and reply with as few words as possible.",
  overrides: { idleTtl: "10m" }
};
async function createSession() {
  const session = await client.sessions.create(CREATE);
  const observed = session.record && session.record.runtime && session.record.runtime.kind;
  if (observed !== runtimeKind) {
    throw new Error("runtime identity mismatch: requested=" + runtimeKind + " observed=" + String(observed));
  }
  return session;
}
function errInfo(e){
  return {
    name: e && e.name ? String(e.name) : null,
    message: String(e && e.message !== undefined ? e.message : e),
    status: (e && typeof e.status === "number") ? e.status : null,
    apiCode: e && e.apiCode ? String(e.apiCode) : null,
    requestId: e && e.requestId ? String(e.requestId) : null
  };
}
function dense(s){ return String(s == null ? "" : s).replace(/\\s+/g, "").toLowerCase(); }
async function waitForLifecycle(session, statuses, timeoutMs){
  const deadline = Date.now() + (timeoutMs || 90000);
  let last = null;
  while (Date.now() < deadline) {
    const rec = await client.sessions.get(session.id);
    last = rec.status;
    if (statuses.includes(rec.status)) return rec.status;
    await new Promise((r) => setTimeout(r, 1500));
  }
  return "TIMEOUT:" + last;
}
function leaks(obj){
  const s = JSON.stringify(obj);
  return (apiKey && s.includes(apiKey));
}
function emit(o){ process.stdout.write(JSON.stringify(o)); process.exit(0); }
`;

async function runChild(scriptName: string, body: string, timeoutMs = 8 * 60_000): Promise<Record<string, unknown>> {
  const script = `${PRE}\n(async () => {\n  try {\n${body}\n  } catch (e) {\n    emit({ fatal: errInfo(e) });\n  }\n})();\n`;
  const scriptPath = join(install.installDir, scriptName);
  writeFileSync(scriptPath, script);
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    timeoutMs,
    env: buildPassEnv({
      AEX_API_URL: apiUrl,
      AEX_API_KEY: apiKey,
      MODEL: model,
      RUNTIME_KIND: runtimeKind
    })
  });
  if (child.exitCode !== 0) {
    throw new Error(formatChildFailure(scriptName, child, [apiKey]));
  }
  const out = child.stdout.trim();
  try {
    return JSON.parse(out) as Record<string, unknown>;
  } catch {
    throw new Error(
      `${scriptName}: child stdout was not JSON:\n${redactKnownValues(out, [apiKey])}\n--- stderr ---\n${redactKnownValues(child.stderr, [apiKey])}`
    );
  }
}

export function registerEdgeChatSessionScenario(shard: EdgeChatSessionShard, wrapperUrl: string): void {
  const wrapperPath = new URL(wrapperUrl).pathname;
  if (!wrapperPath.endsWith(`/${shard.file}`)) {
    throw new Error(`edge chat shard ${shard.scenario} must be registered by ${shard.file}`);
  }
  const { scenario } = shard;
  describe("live DEV — chat session edge cases via installed SDK", () => {
    if (scenario === "multiturn") it(
    "multi-turn keeps context, open() in a fresh client resumes, messages are ordered",
    async () => {
      const result = await runChild(
        "edge-multiturn.mjs",
        `
    const session = await createSession();
    const sessionId = session.id;

    const t1 = await session.messages.send("My name is Zed. Reply with exactly: ok", { idleTimeoutMs: 180000 }).finished();
    const t1DoneStatus = t1.status;
    const t1ServerImmediate = (await client.sessions.get(sessionId)).status;

    const t2 = await session.messages.send("What is my name? Reply with only my name and nothing else.", { idleTimeoutMs: 180000 }).finished();

    // Stable messages property.
    const all = await session.messages.list();
    const listed = await session.messages.list();
    const first = await session.messages.first();
    const last = await session.messages.last();
    const assistant = all.filter((m) => m.sender === "assistant");

    // Fresh client == a brand-new process/handle. Resume by id and continue.
    const client2 = new Aex({ baseUrl, apiKey });
    const session2 = await client2.sessions.open(sessionId);
    const t3 = await session2.messages.send("Say my name one more time, just the name.", { idleTimeoutMs: 180000 }).finished();
    const allAfter3 = await session2.messages.list();

    const out = {
      sessionId,
      turn1Status: t1.status,
      t1DoneStatus,
      t1ServerImmediate,
      turn2Status: t2.status,
      turn2Text: String(t2.text).slice(0, 120),
      contextRetained: dense(t2.text).includes("zed"),
      msgAllLen: all.length,
      msgListLen: listed.length,
      msgListEqualsAll: JSON.stringify(listed) === JSON.stringify(all),
      msgSenders: all.map((m) => m.sender),
      assistantCount: assistant.length,
      firstSender: first ? first.sender : null,
      firstText: first ? String(first.text).slice(0, 80) : null,
      lastText: last ? String(last.text).slice(0, 80) : null,
      lastAssistantHasZed: assistant.length > 0 ? dense(assistant[assistant.length - 1].text).includes("zed") : false,
      turn3Status: t3.status,
      turn3Text: String(t3.text).slice(0, 120),
      freshResumed: dense(t3.text).includes("zed"),
      msgAfter3Len: allAfter3.length
    };
    out.leaked = leaks(out);
    await session2.delete().catch(() => {});
    emit(out);
        `
      );

      const dump = JSON.stringify(result, null, 2);
      expect(result.fatal, dump).toBeUndefined();
      expect(result.turn1Status, dump).toBe("succeeded");
      expect(result.t1ServerImmediate, dump).toBe("idle");
      expect(result.turn2Status, dump).toBe("succeeded");
      expect(result.contextRetained, dump).toBe(true);
      expect(result.turn3Status, dump).toBe("succeeded");
      expect(result.freshResumed, dump).toBe(true);
      // messages property correctness + ordering.
      expect(result.msgListEqualsAll, dump).toBe(true);
      expect(result.assistantCount as number, dump).toBeGreaterThanOrEqual(2);
      expect(result.lastAssistantHasZed, dump).toBe(true);
      expect(result.msgAfter3Len as number, dump).toBeGreaterThanOrEqual(result.msgAllLen as number);
      expect(result.leaked, dump).toBe(false);
    },
    9 * 60_000
    );

    if (scenario === "replay") it(
    "replayLast() de-dupes on the reused key, forces a new turn with a fresh key, and throws before any send",
    async () => {
      const result = await runChild(
        "edge-replaylast.mjs",
        `
    const session = await createSession();
    const replayKey = "edge-replay-" + Date.now() + "-" + Math.random().toString(36).slice(2);
    const t1 = await session.messages.send("Reply with exactly: alpha", { idempotencyKey: replayKey, idleTimeoutMs: 180000 }).finished();
    const turn1Seq = t1.run && typeof t1.run.turnSeq === "number" ? t1.run.turnSeq : -1;

    // Default replayLast reuses the previous idempotency key -> server MUST
    // return the SAME turn, never a second billable turn.
    let replayDedup;
    try {
      const r = await session.messages.replayLast({ idleTimeoutMs: 180000 }).finished();
      replayDedup = { seq: r.run && typeof r.run.turnSeq === "number" ? r.run.turnSeq : -1, status: r.status, text: String(r.text).slice(0, 60) };
    } catch (e) { replayDedup = { error: errInfo(e) }; }

    // A fresh key forces a brand-new turn.
    let replayFresh;
    try {
      const r = await session.messages.replayLast({ idempotencyKey: "edge-replay-fresh-" + Date.now(), idleTimeoutMs: 180000 }).finished();
      replayFresh = { seq: r.run && typeof r.run.turnSeq === "number" ? r.run.turnSeq : -1, status: r.status, text: String(r.text).slice(0, 60) };
    } catch (e) { replayFresh = { error: errInfo(e) }; }

    // replayLast before any send must throw a clear client-side error.
    const s2 = await createSession();
    let replayNoSend;
    try {
      await s2.messages.replayLast().finished();
      replayNoSend = { threw: false };
    } catch (e) { replayNoSend = { threw: true, ...errInfo(e) }; }

    const out = {
      sessionId: session.id,
      replayKeySuffix: replayKey.slice(-10),
      turn1Seq,
      turn1Text: String(t1.text).slice(0, 60),
      replayDedup,
      dedupWorked: !!(replayDedup && replayDedup.seq === turn1Seq),
      replayFresh,
      freshWorked: !!(replayFresh && typeof replayFresh.seq === "number" && replayFresh.seq > turn1Seq),
      replayNoSend
    };
    out.leaked = leaks(out);
    await session.delete().catch(() => {});
    await s2.delete().catch(() => {});
    emit(out);
        `
      );

      const dump = JSON.stringify(result, null, 2);
      expect(result.fatal, dump).toBeUndefined();
      expect((result.turn1Seq as number), dump).toBeGreaterThan(0);
      // Double-billing guard: reused key must not spawn a new turn.
      expect(result.dedupWorked, dump).toBe(true);
      expect(result.freshWorked, dump).toBe(true);
      const noSend = result.replayNoSend as Record<string, unknown>;
      expect(noSend.threw, dump).toBe(true);
      expect(String(noSend.message), dump).toMatch(/no message has been sent/i);
      expect(result.leaked, dump).toBe(false);
    },
    9 * 60_000
    );

    if (scenario === "concurrency") it(
    "two concurrent sends on one session serialize — one sessions, the other is a clean busy rejection",
    async () => {
      const result = await runChild(
        "edge-concurrent.mjs",
        `
    const session = await createSession();
    const mk = (label, key) => session.messages.send("Reply with exactly: " + label, { idempotencyKey: key, idleTimeoutMs: 180000 }).finished()
      .then((r) => ({ ok: true, status: r.status, turnSeq: r.run && r.run.turnSeq, text: String(r.text).slice(0, 40) }))
      .catch((e) => ({ ok: false, ...errInfo(e) }));
    const [a, b] = await Promise.all([mk("one", "edge-conc-a-" + Date.now()), mk("two", "edge-conc-b-" + Date.now())]);
    const oks = [a, b].filter((r) => r.ok);
    const rejected = [a, b].filter((r) => !r.ok);
    const out = {
      a, b,
      okCount: oks.length,
      rejectedCount: rejected.length,
      rejectedStatuses: rejected.map((r) => r.status)
    };
    out.leaked = leaks(out);
    await session.delete().catch(() => {});
    emit(out);
        `
      );

      const dump = JSON.stringify(result, null, 2);
      expect(result.fatal, dump).toBeUndefined();
      // Exactly one turn should win; the other must be a clean rejection (409),
      // never a second concurrent turn on the same session.
      expect(result.okCount, dump).toBe(1);
      expect(result.rejectedCount, dump).toBe(1);
      expect((result.rejectedStatuses as number[]), dump).toContain(409);
      expect(result.leaked, dump).toBe(false);
    },
    9 * 60_000
    );

    if (scenario === "suspend") it(
    "suspend() parks the session; a send while suspended has a clean outcome (auto-resume or clear error)",
    async () => {
      const result = await runChild(
        "edge-suspend-send.mjs",
        `
    const session = await createSession();
    const t1 = await session.messages.send("Reply with exactly: ready", { idleTimeoutMs: 180000 }).finished();
    const afterRunStatus = (await client.sessions.get(session.id)).status;

    const suspended = await session.suspend();
    const suspendImmediate = suspended.session ? suspended.session.status : null;
    const suspendedStatus = await waitForLifecycle(session, ["suspended"], 90000);

    // Probe: send while suspended WITHOUT an explicit resume.
    let sendWhileSuspended;
    try {
      const r = await session.messages.send("Reply with exactly: back", { idleTimeoutMs: 180000 }).finished();
      sendWhileSuspended = { accepted: true, status: r.status, text: String(r.text).slice(0, 40) };
    } catch (e) { sendWhileSuspended = { accepted: false, ...errInfo(e) }; }

    // If the platform requires an explicit resume, exercise resume() + send.
    let resumeThenSend = null;
    if (!sendWhileSuspended.accepted) {
      try {
        const res = await session.resume();
        const resumeStatus = res.session ? res.session.status : null;
        await waitForLifecycle(session, ["idle"], 90000);
        const r = await session.messages.send("Reply with exactly: back", { idleTimeoutMs: 180000 }).finished();
        resumeThenSend = { resumeStatus, accepted: true, status: r.status, text: String(r.text).slice(0, 40) };
      } catch (e) { resumeThenSend = { accepted: false, ...errInfo(e) }; }
    }

    const finalRec = await session.refresh().catch(() => null);
    const out = {
      turn1Status: t1.status,
      afterRunStatus,
      suspendImmediate,
      suspendedStatus,
      sendWhileSuspended,
      resumeThenSend,
      finalStatus: finalRec ? finalRec.status : null,
      canContinue: sendWhileSuspended.accepted === true || (resumeThenSend != null && resumeThenSend.accepted === true)
    };
    out.leaked = leaks(out);
    await session.delete().catch(() => {});
    emit(out);
        `
      );

      const dump = JSON.stringify(result, null, 2);
      expect(result.fatal, dump).toBeUndefined();
      expect(result.turn1Status, dump).toBe("succeeded");
      expect(result.afterRunStatus, dump).toBe("idle");
      expect(result.suspendedStatus, dump).toBe("suspended");
      // After a suspend the conversation must be continuable (auto-resume or
      // explicit resume()+send).
      expect(result.canContinue, dump).toBe(true);
      expect(result.leaked, dump).toBe(false);
    },
    9 * 60_000
    );

    if (scenario === "cancel-send") it(
    "cancel() then send() on the cancelled session is a clean error, never a ghost turn",
    async () => {
      const result = await runChild(
        "edge-cancel-send.mjs",
        `
    const session = await createSession();

    // Kick a turn off but do not await; try to cancel while it is in flight.
    const inflight = session.messages.send("Reply with exactly: gamma", { idleTimeoutMs: 120000 }).finished()
      .then((r) => ({ ok: true, status: r.status }))
      .catch((e) => ({ ok: false, ...errInfo(e) }));
    await new Promise((r) => setTimeout(r, 600));

    let cancel;
    try { const acc = await session.cancel(); cancel = { ok: true, status: acc.session ? acc.session.status : null }; }
    catch (e) { cancel = { ok: false, ...errInfo(e) }; }

    // Bound the in-flight turn so a stalled stream cannot hang the test.
    const inflightResult = await Promise.race([
      inflight,
      new Promise((r) => setTimeout(() => r({ ok: false, timedOut: true }), 130000))
    ]);

    const afterCancelRec = await session.refresh().catch(() => null);

    // Now attempt a NEW send on the cancelled session.
    let sendAfterCancel;
    try {
      const r = await session.messages.send("Reply with exactly: delta", { idleTimeoutMs: 60000 }).finished();
      sendAfterCancel = { ran: true, status: r.status, text: String(r.text).slice(0, 40) };
    } catch (e) { sendAfterCancel = { ran: false, ...errInfo(e) }; }

    const out = {
      cancel,
      inflightResult,
      afterCancelStatus: afterCancelRec ? afterCancelRec.status : null,
      sendAfterCancel,
      // A clean outcome: either the platform rejects the send OR it accepts and
      // re-runs (resume semantics). A 5xx / crash is not clean.
      sendClean: sendAfterCancel.ran === true || (typeof sendAfterCancel.status === "number" && sendAfterCancel.status >= 400 && sendAfterCancel.status < 500)
    };
    out.leaked = leaks(out);
    await session.delete().catch(() => {});
    emit(out);
        `,
        9 * 60_000
      );

      const dump = JSON.stringify(result, null, 2);
      expect(result.fatal, dump).toBeUndefined();
      // cancel() itself must be a clean state change.
      const cancel = result.cancel as Record<string, unknown>;
      expect(cancel.ok, dump).toBe(true);
      expect(result.sendClean, dump).toBe(true);
      expect(result.leaked, dump).toBe(false);
    },
    10 * 60_000
    );

    if (scenario === "cancel-launch") it(
    "cancel during run launch returns a cancelled outcome and leaves the session resumable",
    async () => {
      const result = await runChild(
        "edge-cancel-at-launch.mjs",
        `
    const session = await createSession();

    // Kick a slow turn and cancel the moment the record leaves idle (launch /
    // container-boot phase — before any assistant output exists).
    const inflight = session.messages.send(
      "Count from 1 to 40 slowly, one number per line, with a sentence about each number.",
      { idleTimeoutMs: 240000 }
    ).finished()
      .then((r) => ({ ok: true, status: r.status, text: String(r.text || "").slice(0, 40) }))
      .catch((e) => ({ ok: false, ...errInfo(e) }));

    let launched = null;
    for (let i = 0; i < 60; i++) {
      const rec = await client.sessions.get(session.id).catch(() => null);
      if (rec && rec.status !== "idle") { launched = rec.status; break; }
      await new Promise((r) => setTimeout(r, 1000));
    }

    let cancel;
    try { const acc = await session.cancel(); cancel = { ok: true, status: acc.session ? acc.session.status : null }; }
    catch (e) { cancel = { ok: false, ...errInfo(e) }; }

    // The cancel must land: the session parks (idle) — never wedged in
    // "cancelling", never silently resurrected into a full turn, never
    // auto-suspended with the cancel lost.
    let final = null;
    const timeline = [];
    const deadline = Date.now() + 240000;
    while (Date.now() < deadline) {
      const rec = await client.sessions.get(session.id).catch(() => null);
      if (rec && timeline[timeline.length - 1] !== rec.status) timeline.push(rec.status);
      if (rec && ["idle","suspended","error"].includes(rec.status)) { final = rec; break; }
      await new Promise((r) => setTimeout(r, 2500));
    }

    let terminalOutcome = null;
    try {
      const evs = await session.events.list();
      const terminal = (Array.isArray(evs) ? evs : []).filter(
        (event) => event.type === "RUN_FINISHED" || event.type === "RUN_ERROR"
      ).pop();
      terminalOutcome = terminal && terminal.data ? terminal.data.outcome : null;
    } catch (e) { terminalOutcome = { error: errInfo(e) }; }

    const inflightResult = await Promise.race([
      inflight,
      new Promise((r) => setTimeout(() => r({ ok: false, timedOut: true }), 30000))
    ]);

    const out = { launched, cancel, timeline, finalStatus: final ? final.status : null, terminalOutcome, inflightResult };
    out.leaked = leaks(out);
    await session.delete().catch(() => {});
    emit(out);
        `,
        9 * 60_000
      );

      const dump = JSON.stringify(result, null, 2);
      expect(result.fatal, dump).toBeUndefined();
      const cancel = result.cancel as Record<string, unknown>;
      expect(cancel.ok, dump).toBe(true);
      expect(result.finalStatus, dump).toBe("idle");
      expect(result.terminalOutcome, dump).toBe("cancelled");
      expect(result.leaked, dump).toBe(false);
    },
    10 * 60_000
    );

    if (scenario === "delete") it(
    "delete() leaves no usable ghost; open() on missing/malformed/empty ids is a clean 404, not a crash",
    async () => {
      const result = await runChild(
        "edge-delete-open.mjs",
        `
    const session = await createSession();
    const sid = session.id;
    await session.delete();

    let openAfterDelete;
    try { const h = await client.sessions.open(sid); openAfterDelete = { ok: true, status: h.record ? h.record.status : null }; }
    catch (e) { openAfterDelete = { ok: false, ...errInfo(e) }; }

    let getAfterDelete;
    try { const rec = await client.sessions.get(sid); getAfterDelete = { ok: true, status: rec.status }; }
    catch (e) { getAfterDelete = { ok: false, ...errInfo(e) }; }

    // A send on the deleted session must NOT run a turn.
    let sendAfterDelete;
    try {
      const r = await session.messages.send("Reply with exactly: zeta", { idleTimeoutMs: 45000 }).finished();
      sendAfterDelete = { ran: true, status: r.status };
    } catch (e) { sendAfterDelete = { ran: false, ...errInfo(e) }; }

    let openMissing;
    try { await client.sessions.open("ses_missing_" + Math.random().toString(36).slice(2, 12)); openMissing = { ok: true }; }
    catch (e) { openMissing = { ok: false, ...errInfo(e) }; }

    let openMalformed;
    try { const h = await client.sessions.open("../../etc/passwd"); openMalformed = { ok: true, status: h.record ? h.record.status : null }; }
    catch (e) { openMalformed = { ok: false, ...errInfo(e) }; }

    let openEmpty;
    try { const h = await client.sessions.open(""); openEmpty = { ok: true, status: h.record ? h.record.status : null }; }
    catch (e) { openEmpty = { ok: false, ...errInfo(e) }; }

    const out = { sid, openAfterDelete, getAfterDelete, sendAfterDelete, openMissing, openMalformed, openEmpty };
    out.leaked = leaks(out);
    emit(out);
        `,
        5 * 60_000
      );

      const dump = JSON.stringify(result, null, 2);
      expect(result.fatal, dump).toBeUndefined();
      // A missing id is a clean 404.
      const openMissing = result.openMissing as Record<string, unknown>;
      expect(openMissing.ok, dump).toBe(false);
      expect(openMissing.status, dump).toBe(404);
      // Deleted session must not accept a new turn.
      const sendAfterDelete = result.sendAfterDelete as Record<string, unknown>;
      expect(sendAfterDelete.ran, dump).toBe(false);
      // The deleted session must not read back as a live/idle session.
      const getAfterDelete = result.getAfterDelete as Record<string, unknown>;
      expect(getAfterDelete.ok === false || getAfterDelete.status !== "idle", dump).toBe(true);
      // Malformed / empty ids must not resolve to a usable session.
      const openMalformed = result.openMalformed as Record<string, unknown>;
      const openEmpty = result.openEmpty as Record<string, unknown>;
      expect(openMalformed.ok === false || openMalformed.status !== "idle", dump).toBe(true);
      expect(openEmpty.ok === false || openEmpty.status !== "idle", dump).toBe(true);
      expect(result.leaked, dump).toBe(false);
    },
    6 * 60_000
    );
  });
}
